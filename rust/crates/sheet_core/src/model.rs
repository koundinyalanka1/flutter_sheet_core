//! The workbook model: sheets of sparse cells, styles, defined names.

use std::collections::BTreeMap;
use std::sync::Arc;

use crate::formula::Expr;
use crate::numfmt::FormatOptions;
use crate::refs::{CellRef, Range};
use crate::style::StyleTable;
use crate::value::Value;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ReadLimits {
    pub max_cells: usize,
    pub max_shared_strings: usize,
    pub max_string_bytes: usize,
    pub max_xml_bytes: usize,
}

impl Default for ReadLimits {
    /// The limits for this device: [`ReadLimits::set_cell_budget`]'s, or
    /// the base limits when the app has not set one.
    fn default() -> Self {
        match CELL_BUDGET.load(std::sync::atomic::Ordering::Relaxed) {
            0 => BASE_LIMITS,
            cells => Self::phone(cells),
        }
    }
}

const BASE_LIMITS: ReadLimits = ReadLimits {
    max_cells: 2_000_000,
    max_shared_strings: 500_000,
    max_string_bytes: 32 << 20,
    max_xml_bytes: 64 << 20,
};

/// The cell budget the app chose from the device's memory; 0 when unset.
static CELL_BUDGET: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);

impl ReadLimits {
    /// Set the number of cells a workbook may hold on this device (the app
    /// passes 2 to 4 million by RAM). Clamped as [`ReadLimits::phone`]
    /// does; returns the budget now in force.
    pub fn set_cell_budget(cells: usize) -> usize {
        let cells = Self::phone(cells).max_cells;
        CELL_BUDGET.store(cells, std::sync::atomic::Ordering::Relaxed);
        cells
    }

    pub fn phone(max_cells: usize) -> Self {
        let max_cells = max_cells.clamp(1, if cfg!(target_pointer_width = "32") { 2_000_000 } else { 4_000_000 });
        Self {
            max_cells,
            max_shared_strings: max_cells.min(1_000_000),
            max_string_bytes: if max_cells > 2_000_000 { 64 << 20 } else { 32 << 20 },
            ..BASE_LIMITS
        }
    }

    pub(crate) fn package(self) -> doc_opc::PackageLimits {
        doc_opc::PackageLimits {
            max_xml_part: self.max_xml_bytes as u64,
            max_part: self.max_xml_bytes as u64,
            max_total: (self.max_xml_bytes as u64).saturating_mul(4),
            ..doc_opc::PackageLimits::default()
        }
    }
}

/// A formula in a cell.
#[derive(Clone, Debug, PartialEq)]
pub struct Formula {
    /// The formula as written, without the leading `=`, in Excel's
    /// (English, comma-separated) syntax.
    pub text: String,
    /// The parsed formula. None when it could not be parsed: the text and
    /// the saved value are kept, and the cell is never recalculated.
    pub expr: Option<Arc<Expr>>,
    /// For an array (Ctrl+Shift+Enter) formula, the range it fills.
    pub array: Option<Range>,
    /// A formula element this engine does not interpret (a what-if data
    /// table), written back exactly as read.
    pub raw_xml: Option<Arc<str>>,
    pub ods_text: Option<Arc<str>>,
}

impl Formula {
    /// Parse formula text (without `=`).
    pub fn new(text: &str) -> Formula {
        match crate::formula::parse(text) {
            Ok(expr) => Formula {
                text: crate::formula::unparse(&expr),
                expr: Some(Arc::new(expr)),
                array: None,
                raw_xml: None,
                ods_text: None,
            },
            Err(_) => Formula {
                text: text.to_owned(),
                expr: None,
                array: None,
                raw_xml: None,
                ods_text: None,
            },
        }
    }

    pub fn from_expr(expr: Expr) -> Formula {
        Formula {
            text: crate::formula::unparse(&expr),
            expr: Some(Arc::new(expr)),
            array: None,
            raw_xml: None,
            ods_text: None,
        }
    }
}

/// Rarely used per-cell data, kept out of line.
#[derive(Clone, Debug, PartialEq, Default)]
pub struct CellExtra {
    /// Rich text as an XLSX file stored it (the raw `<si>` contents), and
    /// its shared-string index, kept while the cell is unchanged.
    pub rich: Option<(Arc<str>, u32)>,
    /// The exact text a CSV field had, kept while the cell is unchanged so
    /// that `007` or `1.50` are saved as they were.
    pub source_text: Option<Arc<str>>,
    /// A cell that is part of an array formula's range (but not its first
    /// cell): its value comes from that formula.
    pub array_member: Option<CellRef>,
    /// What an ODS cell had that is not modelled (validations, comments).
    pub ods: Option<Box<crate::ods::OdsCellExtra>>,
    pub xlsx_attrs: Vec<(String, String)>,
    pub xlsx_children: Option<Arc<str>>,
}

/// One cell.
#[derive(Clone, Debug, PartialEq, Default)]
pub struct Cell {
    pub value: Value,
    pub formula: Option<Box<Formula>>,
    /// Index into the workbook's [`StyleTable`].
    pub style: u32,
    pub extra: Option<Box<CellExtra>>,
}

impl Cell {
    pub fn value(value: Value) -> Cell {
        Cell {
            value,
            ..Cell::default()
        }
    }

    /// Whether the cell holds nothing worth keeping (no value, formula or
    /// formatting).
    pub fn is_blank(&self) -> bool {
        self.value.is_empty() && self.formula.is_none() && self.style == 0 && self.extra.is_none()
    }

    pub fn array_member(&self) -> Option<CellRef> {
        self.extra.as_ref().and_then(|e| e.array_member)
    }

    /// Forget file-specific representations after the value changed.
    pub fn clear_source(&mut self) {
        if let Some(extra) = &mut self.extra {
            extra.rich = None;
            extra.source_text = None;
            if extra.array_member.is_none() && extra.ods.is_none() && extra.xlsx_attrs.is_empty() && extra.xlsx_children.is_none() {
                self.extra = None;
            }
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum SheetState {
    #[default]
    Visible,
    Hidden,
    VeryHidden,
}

impl SheetState {
    pub fn name(self) -> &'static str {
        match self {
            SheetState::Visible => "visible",
            SheetState::Hidden => "hidden",
            SheetState::VeryHidden => "veryHidden",
        }
    }
}

/// A column's width and visibility.
#[derive(Clone, Debug, PartialEq, Default)]
pub struct ColInfo {
    /// Width in characters of the default font's digit (Excel's unit).
    pub width: Option<f64>,
    pub hidden: bool,
    pub style: Option<u32>,
    pub custom_width: bool,
    /// Attributes this engine does not model (outline levels...), kept.
    pub extra: Vec<(String, String)>,
}

/// A row's height and visibility.
#[derive(Clone, Debug, PartialEq, Default)]
pub struct RowInfo {
    /// Height in points.
    pub height: Option<f64>,
    pub hidden: bool,
    pub custom_height: bool,
    pub style: Option<u32>,
    pub extra: Vec<(String, String)>,
}

/// A worksheet.
#[derive(Clone, Debug, Default)]
pub struct Sheet {
    pub name: String,
    pub state: SheetState,
    /// Cells by row, then column (both 0-based).
    pub cells: BTreeMap<u32, BTreeMap<u32, Cell>>,
    pub cols: BTreeMap<u32, ColInfo>,
    pub rows: BTreeMap<u32, RowInfo>,
    pub merges: Vec<Range>,
    /// The first cell below and right of frozen panes.
    pub freeze: Option<CellRef>,
    /// Default column width in characters.
    pub default_col_width: f64,
    /// Default row height in points.
    pub default_row_height: f64,
    pub show_grid: bool,
    /// The sheet must be written out again (cells, rows, columns, merges
    /// or panes changed); otherwise its part is copied as it was.
    pub modified: bool,
    /// What the file format needs to write this sheet back in place.
    pub xlsx: Option<Box<crate::xlsx::SheetPart>>,
    pub ods: Option<Box<crate::ods::TablePart>>,
    used: std::cell::Cell<Option<Option<Range>>>,
}

impl Sheet {
    pub fn new(name: impl Into<String>) -> Sheet {
        Sheet {
            name: name.into(),
            default_col_width: 8.43,
            default_row_height: 15.0,
            show_grid: true,
            ..Sheet::default()
        }
    }

    pub fn cell(&self, at: CellRef) -> Option<&Cell> {
        self.cells.get(&at.row)?.get(&at.col)
    }

    pub fn cell_mut(&mut self, at: CellRef) -> Option<&mut Cell> {
        self.cells.get_mut(&at.row)?.get_mut(&at.col)
    }

    pub fn value(&self, at: CellRef) -> Value {
        self.cell(at).map(|c| c.value.clone()).unwrap_or_default()
    }

    /// Store a cell (a blank cell removes it).
    pub fn set(&mut self, at: CellRef, cell: Cell) {
        self.invalidate_used();
        if cell.is_blank() {
            self.remove(at);
            return;
        }
        self.cells.entry(at.row).or_default().insert(at.col, cell);
    }

    pub fn remove(&mut self, at: CellRef) -> Option<Cell> {
        self.invalidate_used();
        let row = self.cells.get_mut(&at.row)?;
        let old = row.remove(&at.col);
        if row.is_empty() {
            self.cells.remove(&at.row);
        }
        old
    }

    /// The cells in a range, row by row.
    pub fn cells_in(&self, range: Range) -> impl Iterator<Item = (CellRef, &Cell)> {
        self.cells.range(range.start.row..=range.end.row).flat_map(move |(&r, row)| {
            row.range(range.start.col..=range.end.col).map(move |(&c, cell)| (CellRef::new(r, c), cell))
        })
    }

    pub fn invalidate_used(&self) {
        self.used.set(None);
    }

    /// The smallest range holding every cell with content.
    pub fn used_range(&self) -> Option<Range> {
        if let Some(cached) = self.used.get() {
            return cached;
        }
        let first_row = self.cells.keys().next().copied();
        let last_row = self.cells.keys().next_back().copied();
        let result = match (first_row, last_row) {
            (Some(r0), Some(r1)) => {
                let mut c0 = u32::MAX;
                let mut c1 = 0;
                for row in self.cells.values() {
                    if let (Some(a), Some(b)) = (row.keys().next(), row.keys().next_back()) {
                        c0 = c0.min(*a);
                        c1 = c1.max(*b);
                    }
                }
                (c0 != u32::MAX).then(|| Range::new(CellRef::new(r0, c0), CellRef::new(r1, c1)))
            }
            _ => None,
        };
        self.used.set(Some(result));
        result
    }

    pub fn cell_count(&self) -> usize {
        self.cells.values().map(|r| r.len()).sum()
    }

    pub fn col_width(&self, col: u32) -> f64 {
        self.cols.get(&col).and_then(|c| c.width).unwrap_or(self.default_col_width)
    }

    pub fn row_height(&self, row: u32) -> f64 {
        self.rows.get(&row).and_then(|r| r.height).unwrap_or(self.default_row_height)
    }

    pub fn col_hidden(&self, col: u32) -> bool {
        self.cols.get(&col).is_some_and(|c| c.hidden)
    }

    pub fn row_hidden(&self, row: u32) -> bool {
        self.rows.get(&row).is_some_and(|r| r.hidden)
    }

    /// The merged range a cell belongs to.
    pub fn merge_at(&self, at: CellRef) -> Option<Range> {
        self.merges.iter().copied().find(|m| m.contains(at))
    }
}

/// A defined name (`TaxRate`, `_xlnm.Print_Area`).
#[derive(Clone, Debug, PartialEq)]
pub struct DefinedName {
    pub name: String,
    /// The sheet a local name belongs to.
    pub sheet: Option<usize>,
    /// What it refers to, without `=`.
    pub text: String,
    pub expr: Option<Arc<Expr>>,
    pub hidden: bool,
    /// Other attributes (comment, function...), kept.
    pub extra: Vec<(String, String)>,
}

impl DefinedName {
    pub fn new(name: &str, sheet: Option<usize>, text: &str) -> DefinedName {
        let expr = crate::formula::parse(text).ok().map(Arc::new);
        DefinedName {
            name: name.to_owned(),
            sheet,
            text: text.to_owned(),
            expr,
            hidden: false,
            extra: Vec::new(),
        }
    }
}

/// Where a workbook came from, and what saving it back needs.
#[derive(Debug, Default)]
pub enum Source {
    #[default]
    New,
    Xlsx(Box<crate::xlsx::Package>),
    Ods(Box<crate::ods::Package>),
    Csv(crate::csv::CsvFormat),
}

/// File formats a workbook can be saved as.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Format {
    Xlsx,
    /// `.xlsm`: an XLSX package with macros (kept, never run).
    Xlsm,
    Ods,
    Csv,
    Tsv,
}

impl Format {
    pub fn from_name(name: &str) -> Option<Format> {
        Some(match name.to_ascii_lowercase().trim_start_matches('.') {
            "xlsx" | "xltx" => Format::Xlsx,
            "xlsm" | "xltm" => Format::Xlsm,
            "ods" | "ots" => Format::Ods,
            "csv" | "txt" => Format::Csv,
            "tsv" | "tab" => Format::Tsv,
            _ => return None,
        })
    }

    pub fn name(self) -> &'static str {
        match self {
            Format::Xlsx => "xlsx",
            Format::Xlsm => "xlsm",
            Format::Ods => "ods",
            Format::Csv => "csv",
            Format::Tsv => "tsv",
        }
    }
}

/// Formats a workbook is read from but not written back: saving writes
/// [`Imported::save_format`] to a new file instead.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Imported {
    /// Excel 97-2003 and WPS Spreadsheets (`.xls`, `.et`).
    Xls,
    /// A flat OpenDocument spreadsheet (`.fods`, one XML file).
    Fods,
}

impl Imported {
    pub fn name(self) -> &'static str {
        match self {
            Imported::Xls => "xls",
            Imported::Fods => "fods",
        }
    }

    /// The format such a workbook is saved as.
    pub fn save_format(self) -> Format {
        match self {
            Imported::Xls => Format::Xlsx,
            Imported::Fods => Format::Ods,
        }
    }
}

/// A whole workbook.
#[derive(Debug)]
pub struct Workbook {
    pub limits: ReadLimits,
    pub sheets: Vec<Sheet>,
    pub styles: StyleTable,
    pub names: Vec<DefinedName>,
    pub date1904: bool,
    /// The sheet shown first.
    pub active: usize,
    pub source: Source,
    /// The format the workbook was read from (None for a new one).
    pub format: Option<Format>,
    /// The format of the file it was opened from, when that is not written
    /// back (an `.xls`, a flat `.fods`).
    pub imported: Option<Imported>,
    /// The locale's date order, for typed dates and criteria.
    pub date_order: crate::numfmt::DateOrder,
    /// Things the reader could not keep or understand, worded for people.
    pub warnings: Vec<String>,
    /// Sheets were added, removed, renamed, reordered or hidden, or names
    /// changed: the workbook part must be written again.
    pub structure_changed: bool,
    /// Values were recalculated here; ask Excel to recalculate on open.
    pub recalculated: bool,
    /// The styles part must be written again.
    pub styles_changed: bool,
    pub(crate) calc: crate::calc::CalcState,
    pub(crate) history: crate::edit::History,
    /// Edits since the last save.
    pub dirty: bool,
}

impl Default for Workbook {
    fn default() -> Self {
        Workbook {
            limits: ReadLimits::default(),
            sheets: Vec::new(),
            styles: StyleTable::default(),
            names: Vec::new(),
            date1904: false,
            active: 0,
            source: Source::New,
            format: None,
            imported: None,
            date_order: Default::default(),
            warnings: Vec::new(),
            structure_changed: false,
            recalculated: false,
            styles_changed: false,
            calc: Default::default(),
            history: Default::default(),
            dirty: false,
        }
    }
}

impl Workbook {
    /// A new workbook with one empty sheet.
    pub fn new() -> Workbook {
        Workbook {
            sheets: vec![Sheet::new("Sheet1")],
            ..Workbook::default()
        }
    }

    /// Whether saving writes the file it was opened from back in its own
    /// format (true for a new workbook).
    pub fn writes_back(&self) -> bool {
        self.imported.is_none()
    }

    /// The format saving writes: the workbook's own when it is written
    /// back, otherwise the modern equivalent of what it was read from;
    /// XLSX for a new workbook.
    pub fn save_format(&self) -> Format {
        match self.imported {
            Some(imported) => imported.save_format(),
            None => self.format.unwrap_or(Format::Xlsx),
        }
    }

    pub fn format_options(&self) -> FormatOptions {
        FormatOptions {
            date_order: self.date_order,
            date1904: self.date1904,
        }
    }

    pub fn sheet_index(&self, name: &str) -> Option<usize> {
        self.sheets.iter().position(|s| s.name.eq_ignore_ascii_case(name))
    }

    /// A sheet name not in use: `Sheet2`, `Sheet3`...
    pub fn unused_sheet_name(&self, stem: &str) -> String {
        (1..)
            .map(|i| if i == 1 && stem != "Sheet" { stem.to_owned() } else { format!("{stem}{i}") })
            .find(|n| self.sheet_index(n).is_none())
            .unwrap_or_else(|| stem.to_owned())
    }

    /// The value of a defined name as an expression, from a sheet's view.
    pub fn name_expr(&self, name: &str, sheet: usize) -> Option<Arc<Expr>> {
        let local = self
            .names
            .iter()
            .find(|n| n.sheet == Some(sheet) && n.name.eq_ignore_ascii_case(name));
        let global = || self.names.iter().find(|n| n.sheet.is_none() && n.name.eq_ignore_ascii_case(name));
        local.or_else(global).and_then(|n| n.expr.clone())
    }

    /// Whether a name is valid for a sheet: 1-31 characters, none of
    /// `: \ / ? * [ ]`, not starting or ending with an apostrophe.
    pub fn valid_sheet_name(name: &str) -> bool {
        let n = name.chars().count();
        (1..=31).contains(&n)
            && !name.contains([':', '\\', '/', '?', '*', '[', ']'])
            && !name.starts_with('\'')
            && !name.ends_with('\'')
            && !name.eq_ignore_ascii_case("history")
    }
}
