//! Editing a workbook, with undo and redo.
//!
//! Every edit is an [`Op`]. Edits to cells record the cells before and
//! after; edits that move or rename things (rows, columns, sheets) record
//! the whole workbook before and after. Undo memory is capped.

use std::collections::BTreeMap;

use crate::calc::Key;
use crate::formula::adjust::{
    invalidate_sheet, move_block, offset_references, rename_sheet, shift_index, shift_range, shift_references, Shift,
};
use crate::input::{parse_input, Input};
use crate::model::{Cell, ColInfo, DefinedName, Formula, RowInfo, Sheet, SheetState, Workbook};
use crate::refs::{CellRef, Range, MAX_COLS, MAX_ROWS};
use crate::style::{CellStyle, ColorSpec, Edge, Fill, Underline};
use crate::value::{compare, Value};

/// Why an edit was refused, worded for a person.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("{0}")]
pub struct EditError(pub String);

fn refuse<T>(message: impl Into<String>) -> Result<T, EditError> {
    Err(EditError(message.into()))
}

/// What to clear.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ClearWhat {
    All,
    Contents,
    Formats,
}

/// Which borders to set.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BorderKind {
    All,
    Outline,
    Inside,
    None,
    Top,
    Bottom,
    Left,
    Right,
}

/// A change to the formatting of cells: only the fields that are set are
/// applied.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct StylePatch {
    pub bold: Option<bool>,
    pub italic: Option<bool>,
    pub underline: Option<Underline>,
    pub strike: Option<bool>,
    pub font_name: Option<String>,
    /// Hundredths of a point.
    pub font_size: Option<u32>,
    pub font_color: Option<Option<ColorSpec>>,
    pub fill: Option<Option<ColorSpec>>,
    pub h_align: Option<String>,
    pub v_align: Option<String>,
    pub wrap: Option<bool>,
    pub indent: Option<u8>,
    pub num_fmt: Option<String>,
    pub border: Option<(BorderKind, String, Option<ColorSpec>)>,
    /// Add (or remove, when negative) decimal places shown.
    pub decimals_delta: Option<i32>,
}

/// How a sort orders one column.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SortKey {
    /// Column on the sheet (not relative to the range).
    pub col: u32,
    pub ascending: bool,
}

/// What pasting brings over.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PasteWhat {
    All,
    Values,
    Formats,
    Formulas,
}

/// One edit.
#[derive(Clone, Debug, PartialEq)]
pub enum Op {
    /// Type into a cell, as a person would (`=SUM(A1:A3)`, `12%`, `13/10/2026`).
    SetCell { sheet: usize, at: CellRef, input: String },
    /// Type a block of entries starting at a cell (pasted text).
    SetCells { sheet: usize, at: CellRef, rows: Vec<Vec<String>> },
    Clear { sheet: usize, ranges: Vec<Range>, what: ClearWhat },
    Format { sheet: usize, ranges: Vec<Range>, patch: StylePatch },
    InsertRows { sheet: usize, at: u32, count: u32 },
    DeleteRows { sheet: usize, at: u32, count: u32 },
    InsertCols { sheet: usize, at: u32, count: u32 },
    DeleteCols { sheet: usize, at: u32, count: u32 },
    /// Width in characters; None for the default.
    ColWidth { sheet: usize, first: u32, last: u32, width: Option<f64> },
    /// Height in points; None for the default.
    RowHeight { sheet: usize, first: u32, last: u32, height: Option<f64> },
    HideRows { sheet: usize, first: u32, last: u32, hidden: bool },
    HideCols { sheet: usize, first: u32, last: u32, hidden: bool },
    Merge { sheet: usize, range: Range },
    Unmerge { sheet: usize, range: Range },
    /// Freeze the rows above and columns left of a cell (None unfreezes).
    Freeze { sheet: usize, at: Option<CellRef> },
    AddSheet { name: Option<String>, at: Option<usize> },
    RenameSheet { sheet: usize, name: String },
    DeleteSheet { sheet: usize },
    MoveSheet { sheet: usize, to: usize },
    DuplicateSheet { sheet: usize },
    HideSheet { sheet: usize, hidden: bool },
    SetActive { sheet: usize },
    /// Fill the rest of `range` from its first row (down) or column (right).
    Fill { sheet: usize, range: Range, down: bool },
    /// Copy (or move, with `cut`) a block to another place.
    Paste { from_sheet: usize, from: Range, to_sheet: usize, to: CellRef, what: PasteWhat, cut: bool },
    Sort { sheet: usize, range: Range, keys: Vec<SortKey>, header: bool },
    /// Replace text in constant cells (not formulas). `sheet` None for all.
    Replace { sheet: Option<usize>, find: String, replace: String, match_case: bool, whole: bool },
    DefineName { name: String, sheet: Option<usize>, refers_to: String },
    DeleteName { name: String, sheet: Option<usize> },
}

/// A sheet's layout (everything but its cells).
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Layout {
    cols: BTreeMap<u32, ColInfo>,
    rows: BTreeMap<u32, RowInfo>,
    merges: Vec<Range>,
    freeze: Option<CellRef>,
}

impl Layout {
    fn of(s: &Sheet) -> Layout {
        Layout {
            cols: s.cols.clone(),
            rows: s.rows.clone(),
            merges: s.merges.clone(),
            freeze: s.freeze,
        }
    }

    fn apply(&self, s: &mut Sheet) {
        s.cols = self.cols.clone();
        s.rows = self.rows.clone();
        s.merges = self.merges.clone();
        s.freeze = self.freeze;
    }
}

#[derive(Clone, Debug)]
pub(crate) struct Snapshot {
    sheets: Vec<Sheet>,
    names: Vec<DefinedName>,
    active: usize,
}

impl Snapshot {
    fn of(wb: &Workbook) -> Snapshot {
        Snapshot {
            sheets: wb.sheets.clone(),
            names: wb.names.clone(),
            active: wb.active,
        }
    }

    /// Trade places with the workbook: it gets the snapshot's state and the
    /// snapshot keeps the state it had, so undo and redo each swap once
    /// and the history holds one copy of the workbook, not two.
    fn swap(&mut self, wb: &mut Workbook) {
        std::mem::swap(&mut wb.sheets, &mut self.sheets);
        std::mem::swap(&mut wb.names, &mut self.names);
        std::mem::swap(&mut wb.active, &mut self.active);
        for s in &mut wb.sheets {
            s.modified = true;
            s.invalidate_used();
        }
        wb.active = wb.active.min(wb.sheets.len().saturating_sub(1));
        wb.structure_changed = true;
        wb.calc.invalidate();
    }

    fn weight(&self) -> usize {
        self.sheets.iter().map(|s| s.cell_count() + 64).sum()
    }
}

/// The workbook-level state that hiding, moving and naming change: no
/// cells are copied for them.
#[derive(Clone, Debug)]
pub(crate) struct Meta {
    names: Vec<DefinedName>,
    /// Each sheet's state, in the sheet order of the side this records.
    states: Vec<SheetState>,
    active: usize,
    /// A sheet moved from the first index to the second.
    moved: Option<(usize, usize)>,
}

impl Meta {
    fn of(wb: &Workbook, moved: Option<(usize, usize)>) -> Meta {
        Meta {
            names: wb.names.clone(),
            states: wb.sheets.iter().map(|s| s.state).collect(),
            active: wb.active,
            moved,
        }
    }

    /// Trade places with the workbook, as [`Snapshot::swap`] does.
    fn swap(&mut self, wb: &mut Workbook, undo: bool) {
        let states: Vec<SheetState> = wb.sheets.iter().map(|s| s.state).collect();
        if let Some((from, to)) = self.moved {
            let (a, b) = if undo { (to, from) } else { (from, to) };
            if a < wb.sheets.len() && b < wb.sheets.len() {
                let s = wb.sheets.remove(a);
                wb.sheets.insert(b, s);
            }
        }
        for (s, state) in wb.sheets.iter_mut().zip(&self.states) {
            s.state = *state;
        }
        self.states = states;
        std::mem::swap(&mut wb.names, &mut self.names);
        std::mem::swap(&mut wb.active, &mut self.active);
        wb.active = wb.active.min(wb.sheets.len().saturating_sub(1));
        wb.structure_changed = true;
        wb.calc.invalidate();
    }

    fn weight(&self) -> usize {
        self.names.len() + self.states.len() + 1
    }
}

type CellChange = (CellRef, Option<Cell>, Option<Cell>);

#[derive(Clone, Debug)]
pub(crate) enum Change {
    Cells {
        sheet: usize,
        cells: Vec<CellChange>,
        layout: Option<(Layout, Layout)>,
    },
    /// The whole workbook on the other side of the change (before it while
    /// on the undo stack, after it while on the redo stack).
    Workbook { other: Snapshot },
    /// Names, sheet states, the active sheet and a sheet move, likewise.
    Meta(Meta),
}

impl Change {
    fn weight(&self) -> usize {
        match self {
            Change::Cells { cells, .. } => cells.len() + 1,
            Change::Workbook { other } => other.weight(),
            Change::Meta(meta) => meta.weight(),
        }
    }
}

/// Undo and redo stacks.
#[derive(Clone, Debug, Default)]
pub(crate) struct History {
    undo: Vec<Change>,
    redo: Vec<Change>,
}

/// The most undo history kept, in cells (a structural change counts every
/// cell of the workbook once; hiding, moving and naming count no cells).
const UNDO_BUDGET: usize = 4_000_000;
const UNDO_STEPS: usize = 200;

impl History {
    fn push(&mut self, change: Change) {
        self.redo.clear();
        self.undo.push(change);
        let mut total: usize = self.undo.iter().map(Change::weight).sum();
        while (total > UNDO_BUDGET || self.undo.len() > UNDO_STEPS) && self.undo.len() > 1 {
            total -= self.undo.remove(0).weight();
        }
    }
}

/// What an edit did, for the screen to refresh.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct EditResult {
    /// Cells whose value or look changed (including recalculated ones).
    pub changed: Vec<Key>,
    /// Rows, columns or sheets moved: refresh everything.
    pub structure: bool,
    /// Circular references found while recalculating.
    pub circular: Vec<Key>,
}

/// Records the cells an edit touches, before and after.
struct Recorder {
    sheet: usize,
    before: BTreeMap<CellRef, Option<Cell>>,
}

impl Recorder {
    fn new(sheet: usize) -> Self {
        Recorder {
            sheet,
            before: BTreeMap::new(),
        }
    }

    fn touch(&mut self, wb: &Workbook, at: CellRef) {
        self.before
            .entry(at)
            .or_insert_with(|| wb.sheets[self.sheet].cell(at).cloned());
    }

    fn finish(self, wb: &Workbook) -> Vec<CellChange> {
        self.before
            .into_iter()
            .map(|(at, before)| {
                let after = wb.sheets[self.sheet].cell(at).cloned();
                (at, before, after)
            })
            .filter(|(_, b, a)| b != a)
            .collect()
    }
}

impl Workbook {
    fn check_sheet(&self, sheet: usize) -> Result<(), EditError> {
        if sheet < self.sheets.len() {
            Ok(())
        } else {
            refuse("That sheet does not exist.")
        }
    }

    pub fn can_undo(&self) -> bool {
        !self.history.undo.is_empty()
    }

    pub fn can_redo(&self) -> bool {
        !self.history.redo.is_empty()
    }

    /// Apply an edit.
    pub fn apply(&mut self, op: Op) -> Result<EditResult, EditError> {
        let structural = matches!(
            op,
            Op::InsertRows { .. }
                | Op::DeleteRows { .. }
                | Op::InsertCols { .. }
                | Op::DeleteCols { .. }
                | Op::AddSheet { .. }
                | Op::RenameSheet { .. }
                | Op::DeleteSheet { .. }
                | Op::MoveSheet { .. }
                | Op::DuplicateSheet { .. }
                | Op::HideSheet { .. }
                | Op::DefineName { .. }
                | Op::DeleteName { .. }
                | Op::Replace { sheet: None, .. }
                | Op::Paste { cut: true, .. }
        ) || matches!(&op, Op::Paste { from_sheet, to_sheet, .. } if from_sheet != to_sheet);
        if let Op::SetActive { sheet } = op {
            self.check_sheet(sheet)?;
            self.active = sheet;
            return Ok(EditResult::default());
        }
        let meta_only = match &op {
            Op::HideSheet { .. } | Op::DefineName { .. } | Op::DeleteName { .. } => Some(None),
            Op::MoveSheet { sheet, to } => Some(Some((*sheet, (*to).min(self.sheets.len().saturating_sub(1))))),
            _ => None,
        };
        if let Some(moved) = meta_only {
            // These check everything before changing anything.
            let before = Meta::of(self, moved);
            self.apply_structural(op)?;
            self.history.push(Change::Meta(before));
            self.calc.invalidate();
            self.structure_changed = true;
            self.dirty = true;
            let changed = self.recalc_all();
            return Ok(EditResult {
                changed,
                structure: true,
                circular: self.calc.circular.clone(),
            });
        }
        if structural {
            let mut before = Snapshot::of(self);
            let result = self.apply_structural(op);
            match result {
                Ok(()) => {
                    self.history.push(Change::Workbook { other: before });
                    self.calc.invalidate();
                    self.structure_changed = true;
                    self.dirty = true;
                    let changed = self.recalc_all();
                    Ok(EditResult {
                        changed,
                        structure: true,
                        circular: self.calc.circular.clone(),
                    })
                }
                Err(e) => {
                    before.swap(self);
                    Err(e)
                }
            }
        } else {
            let sheet = op_sheet(&op);
            self.check_sheet(sheet)?;
            let layout_before = Layout::of(&self.sheets[sheet]);
            let mut rec = Recorder::new(sheet);
            if let Err(e) = self.apply_cells(op, &mut rec) {
                // Put back whatever was touched.
                for (at, before) in rec.before {
                    match before {
                        Some(c) => self.sheets[sheet].set(at, c),
                        None => {
                            self.sheets[sheet].remove(at);
                        }
                    }
                }
                layout_before.apply(&mut self.sheets[sheet]);
                return Err(e);
            }
            let cells = rec.finish(self);
            let layout_after = Layout::of(&self.sheets[sheet]);
            let layout = (layout_before != layout_after).then_some((layout_before, layout_after));
            if cells.is_empty() && layout.is_none() {
                return Ok(EditResult::default());
            }
            let keys: Vec<Key> = cells.iter().map(|(at, _, _)| (sheet, *at)).collect();
            for (at, _, after) in &cells {
                if after.as_ref().is_some_and(|c| c.formula.is_some()) {
                    self.index_formula(sheet, *at);
                }
            }
            self.history.push(Change::Cells { sheet, cells, layout });
            self.sheets[sheet].modified = true;
            self.dirty = true;
            let mut changed = keys.clone();
            changed.extend(self.recalc_from(&keys));
            Ok(EditResult {
                changed,
                structure: false,
                circular: self.calc.circular.clone(),
            })
        }
    }

    pub fn undo(&mut self) -> Option<EditResult> {
        let mut change = self.history.undo.pop()?;
        let result = self.revert(&mut change, true);
        self.history.redo.push(change);
        Some(result)
    }

    pub fn redo(&mut self) -> Option<EditResult> {
        let mut change = self.history.redo.pop()?;
        let result = self.revert(&mut change, false);
        self.history.undo.push(change);
        Some(result)
    }

    fn revert(&mut self, change: &mut Change, undo: bool) -> EditResult {
        self.dirty = true;
        match change {
            Change::Cells { sheet, cells, layout } => {
                let s = &mut self.sheets[*sheet];
                for (at, before, after) in cells.iter() {
                    let target = if undo { before } else { after };
                    match target {
                        Some(c) => s.set(*at, c.clone()),
                        None => {
                            s.remove(*at);
                        }
                    }
                }
                if let Some((b, a)) = layout {
                    if undo { b } else { a }.apply(s);
                }
                s.modified = true;
                let keys: Vec<Key> = cells.iter().map(|(at, _, _)| (*sheet, *at)).collect();
                for (at, _, _) in cells {
                    if self.sheets[*sheet].cell(*at).is_some_and(|c| c.formula.is_some()) {
                        self.index_formula(*sheet, *at);
                    }
                }
                let mut changed = keys.clone();
                changed.extend(self.recalc_from(&keys));
                EditResult {
                    changed,
                    structure: layout.is_some(),
                    circular: self.calc.circular.clone(),
                }
            }
            Change::Workbook { other } => {
                other.swap(self);
                EditResult {
                    changed: Vec::new(),
                    structure: true,
                    circular: Vec::new(),
                }
            }
            Change::Meta(meta) => {
                meta.swap(self, undo);
                EditResult {
                    changed: Vec::new(),
                    structure: true,
                    circular: Vec::new(),
                }
            }
        }
    }

    // ----- cell edits

    fn apply_cells(&mut self, op: Op, rec: &mut Recorder) -> Result<(), EditError> {
        match op {
            Op::SetCell { sheet, at, input } => self.set_input(sheet, at, &input, rec),
            Op::SetCells { sheet, at, rows } => {
                for (i, row) in rows.iter().enumerate() {
                    for (j, input) in row.iter().enumerate() {
                        let Some(cell) = at.offset(i as i64, j as i64) else {
                            return refuse("The pasted cells do not fit on the sheet.");
                        };
                        self.set_input(sheet, cell, input, rec)?;
                    }
                }
                Ok(())
            }
            Op::Clear { sheet, ranges, what } => {
                for range in ranges {
                    let cells: Vec<CellRef> = self.sheets[sheet].cells_in(clip(range)).map(|(at, _)| at).collect();
                    for at in cells {
                        if self.blocks_array_edit(sheet, at, range) {
                            return refuse("You can't change part of an array formula.");
                        }
                        rec.touch(self, at);
                        let s = &mut self.sheets[sheet];
                        match what {
                            ClearWhat::All => {
                                s.remove(at);
                            }
                            ClearWhat::Contents => {
                                if let Some(c) = s.cell(at).cloned() {
                                    s.set(at, Cell { style: c.style, ..Cell::default() });
                                }
                            }
                            ClearWhat::Formats => {
                                if let Some(c) = s.cell_mut(at) {
                                    c.style = 0;
                                    if c.is_blank() {
                                        s.remove(at);
                                    }
                                }
                            }
                        }
                    }
                }
                Ok(())
            }
            Op::Format { sheet, ranges, patch } => self.format(sheet, &ranges, &patch, rec),
            Op::ColWidth { sheet, first, last, width } => {
                let s = &mut self.sheets[sheet];
                for c in first..=last.min(MAX_COLS - 1) {
                    let info = s.cols.entry(c).or_default();
                    info.width = width;
                    info.custom_width = width.is_some();
                    if *info == ColInfo::default() {
                        s.cols.remove(&c);
                    }
                }
                Ok(())
            }
            Op::RowHeight { sheet, first, last, height } => {
                let s = &mut self.sheets[sheet];
                for r in first..=last.min(MAX_ROWS - 1) {
                    let info = s.rows.entry(r).or_default();
                    info.height = height;
                    info.custom_height = height.is_some();
                    if *info == RowInfo::default() {
                        s.rows.remove(&r);
                    }
                }
                Ok(())
            }
            Op::HideRows { sheet, first, last, hidden } => {
                let s = &mut self.sheets[sheet];
                for r in first..=last.min(MAX_ROWS - 1) {
                    let info = s.rows.entry(r).or_default();
                    info.hidden = hidden;
                    if *info == RowInfo::default() {
                        s.rows.remove(&r);
                    }
                }
                Ok(())
            }
            Op::HideCols { sheet, first, last, hidden } => {
                let s = &mut self.sheets[sheet];
                for c in first..=last.min(MAX_COLS - 1) {
                    let info = s.cols.entry(c).or_default();
                    info.hidden = hidden;
                    if *info == ColInfo::default() {
                        s.cols.remove(&c);
                    }
                }
                Ok(())
            }
            Op::Merge { sheet, range } => {
                if range.cell_count() < 2 {
                    return Ok(());
                }
                // Merging keeps only the top-left value, as Excel does.
                let cells: Vec<CellRef> = self.sheets[sheet].cells_in(range).map(|(at, _)| at).collect();
                for at in cells {
                    if at != range.start {
                        rec.touch(self, at);
                        let s = &mut self.sheets[sheet];
                        if let Some(c) = s.cell(at).cloned() {
                            s.set(at, Cell { style: c.style, ..Cell::default() });
                        }
                    }
                }
                let s = &mut self.sheets[sheet];
                s.merges.retain(|m| !m.intersects(range));
                s.merges.push(range);
                Ok(())
            }
            Op::Unmerge { sheet, range } => {
                self.sheets[sheet].merges.retain(|m| !m.intersects(range));
                Ok(())
            }
            Op::Freeze { sheet, at } => {
                self.sheets[sheet].freeze = at.filter(|c| c.row > 0 || c.col > 0);
                Ok(())
            }
            Op::Fill { sheet, range, down } => {
                let (count, step_r, step_c) = if down { (range.rows(), 1i64, 0i64) } else { (range.cols(), 0, 1) };
                let source = if down {
                    Range::new(range.start, CellRef::new(range.start.row, range.end.col))
                } else {
                    Range::new(range.start, CellRef::new(range.end.row, range.start.col))
                };
                for k in 1..count as i64 {
                    let to = CellRef::new((range.start.row as i64 + k * step_r) as u32, (range.start.col as i64 + k * step_c) as u32);
                    self.copy_block(sheet, source, sheet, to, PasteWhat::All, rec)?;
                }
                Ok(())
            }
            Op::Paste { from_sheet, from, to, what, .. } => {
                // Same sheet, not a cut (other cases are structural).
                let block = self.read_block(from_sheet, from);
                self.write_block(from_sheet, from, &block, from_sheet, to, what, rec)
            }
            Op::Sort { sheet, range, keys, header } => self.sort(sheet, range, &keys, header, rec),
            Op::Replace { sheet: Some(sheet), find, replace, match_case, whole } => {
                self.replace_in(sheet, &find, &replace, match_case, whole, Some(rec)).map(|_| ())
            }
            other => refuse(format!("{other:?} is not a cell edit")),
        }
    }

    /// Whether editing `at` would change part of an array formula without
    /// all of it being in `scope`.
    fn blocks_array_edit(&self, sheet: usize, at: CellRef, scope: Range) -> bool {
        let s = &self.sheets[sheet];
        let anchor = match s.cell(at) {
            Some(c) if c.formula.as_ref().is_some_and(|f| f.array.is_some()) => Some(at),
            Some(c) => c.array_member(),
            None => None,
        };
        let Some(anchor) = anchor else { return false };
        let Some(range) = s.cell(anchor).and_then(|c| c.formula.as_ref()).and_then(|f| f.array) else {
            return false;
        };
        !(scope.contains(range.start) && scope.contains(range.end))
    }

    /// Type into a cell.
    fn set_input(&mut self, sheet: usize, at: CellRef, text: &str, rec: &mut Recorder) -> Result<(), EditError> {
        if self.blocks_array_edit(sheet, at, Range::cell(at)) {
            let anchor_is_single = self.sheets[sheet]
                .cell(at)
                .and_then(|c| c.formula.as_ref())
                .and_then(|f| f.array)
                .is_some_and(|r| r.cell_count() == 1);
            if !anchor_is_single {
                return refuse("You can't change part of an array formula.");
            }
        }
        rec.touch(self, at);
        let options = self.format_options();
        let current = self.sheets[sheet].cell(at).cloned().unwrap_or_default();
        let style_id = current.style;
        let style = self.styles.get(style_id).clone();
        let text_format = crate::numfmt::NumberFormat::cached(&style.num_fmt).is_text();
        let input = if text_format && !text.starts_with('=') {
            if text.is_empty() {
                Input::Empty
            } else {
                Input::Text(text.to_owned())
            }
        } else {
            parse_input(text, &options)
        };
        let mut cell = Cell {
            style: style_id,
            ..Cell::default()
        };
        match input {
            Input::Empty => {}
            Input::Formula(f) => {
                let parsed = crate::formula::parse(&f).map_err(|e| EditError(format!("There's a problem with this formula: {e}")))?;
                cell.formula = Some(Box::new(Formula::from_expr(parsed)));
                // Shown until recalculation fills it in.
                cell.value = current.value.clone();
            }
            Input::Number(n, hint) => {
                cell.value = Value::Number(n);
                if let Some(hint) = hint {
                    let fmt = crate::numfmt::NumberFormat::cached(&style.num_fmt);
                    let keep = !fmt.is_general() && (fmt.is_date() == hint.contains(['d', 'y', 'h']));
                    if !keep {
                        let mut s = style.clone();
                        s.num_fmt = hint;
                        cell.style = self.styles.intern(s);
                        self.styles_changed = true;
                    }
                }
            }
            Input::Text(t) => cell.value = Value::text(t),
            Input::Bool(b) => cell.value = Value::Bool(b),
            Input::Error(e) => cell.value = Value::Error(e),
        }
        self.sheets[sheet].set(at, cell);
        Ok(())
    }

    fn format(&mut self, sheet: usize, ranges: &[Range], patch: &StylePatch, rec: &mut Recorder) -> Result<(), EditError> {
        let mut cache: BTreeMap<u32, u32> = BTreeMap::new();
        for &range in ranges {
            let range = clip(range);
            if range.cell_count() > 2_000_000 {
                return refuse("Select fewer cells to format.");
            }
            // Whole columns or rows format the cells in use, and the
            // column or row itself for cells typed later.
            let whole_cols = range.rows() == MAX_ROWS;
            let whole_rows = range.cols() == MAX_COLS;
            let cells: Vec<CellRef> = if whole_cols || whole_rows {
                self.sheets[sheet].cells_in(range).map(|(at, _)| at).collect()
            } else {
                range.iter().collect()
            };
            for at in cells {
                rec.touch(self, at);
                let old = self.sheets[sheet].cell(at).map_or(0, |c| c.style);
                let edge_patch = patch.border.as_ref().map(|(kind, style, color)| edges_for(*kind, range, at, style, color));
                let new = match (&edge_patch, cache.get(&old)) {
                    (None, Some(&id)) => id,
                    _ => {
                        let mut style = self.styles.get(old).clone();
                        apply_patch(&mut style, patch, edge_patch.as_ref());
                        let id = self.styles.intern(style);
                        if edge_patch.is_none() {
                            cache.insert(old, id);
                        }
                        id
                    }
                };
                if new != old {
                    let s = &mut self.sheets[sheet];
                    let mut cell = s.cell(at).cloned().unwrap_or_default();
                    cell.style = new;
                    s.set(at, cell);
                }
            }
            if whole_cols || whole_rows {
                let s = &mut self.sheets[sheet];
                let base_style = |id: Option<u32>, styles: &mut crate::style::StyleTable| {
                    let mut st = styles.get(id.unwrap_or(0)).clone();
                    apply_patch(&mut st, patch, None);
                    styles.intern(st)
                };
                if whole_cols {
                    for c in range.start.col..=range.end.col {
                        let info = s.cols.entry(c).or_default();
                        info.style = Some(base_style(info.style, &mut self.styles));
                    }
                } else {
                    for r in range.start.row..=range.end.row.min(range.start.row + 100_000) {
                        let info = s.rows.entry(r).or_default();
                        info.style = Some(base_style(info.style, &mut self.styles));
                    }
                }
            }
        }
        if self.styles.len() > self.styles.original {
            self.styles_changed = true;
        }
        Ok(())
    }

    /// The cells of a block, relative to its top-left.
    fn read_block(&self, sheet: usize, from: Range) -> Vec<(u32, u32, Cell)> {
        self.sheets[sheet]
            .cells_in(from)
            .map(|(at, c)| (at.row - from.start.row, at.col - from.start.col, c.clone()))
            .collect()
    }

    /// Write a copied block at `to`, moving relative references in its
    /// formulas by the distance copied.
    #[allow(clippy::too_many_arguments)]
    fn write_block(
        &mut self,
        from_sheet: usize,
        from: Range,
        block: &[(u32, u32, Cell)],
        to_sheet: usize,
        to: CellRef,
        what: PasteWhat,
        rec: &mut Recorder,
    ) -> Result<(), EditError> {
        let Some(end) = to.offset(from.rows() as i64 - 1, from.cols() as i64 - 1) else {
            return refuse("The pasted cells do not fit on the sheet.");
        };
        let target = Range::new(to, end);
        let dr = to.row as i64 - from.start.row as i64;
        let dc = to.col as i64 - from.start.col as i64;
        // Clear the target first (cells the block leaves empty).
        let existing: Vec<CellRef> = self.sheets[to_sheet].cells_in(target).map(|(at, _)| at).collect();
        for at in existing {
            if self.blocks_array_edit(to_sheet, at, target) {
                return refuse("You can't change part of an array formula.");
            }
            rec.touch(self, at);
            let s = &mut self.sheets[to_sheet];
            match what {
                PasteWhat::All => {
                    s.remove(at);
                }
                PasteWhat::Values | PasteWhat::Formulas => {
                    if let Some(c) = s.cell(at).cloned() {
                        s.set(at, Cell { style: c.style, ..Cell::default() });
                    }
                }
                PasteWhat::Formats => {
                    if let Some(c) = s.cell_mut(at) {
                        c.style = 0;
                    }
                }
            }
        }
        let _ = from_sheet;
        for (r, c, cell) in block {
            let at = CellRef::new(to.row + r, to.col + c);
            rec.touch(self, at);
            let mut new = self.sheets[to_sheet].cell(at).cloned().unwrap_or_default();
            match what {
                PasteWhat::All | PasteWhat::Formulas => {
                    new.value = cell.value.clone();
                    new.formula = cell.formula.as_ref().map(|f| {
                        let mut f = f.clone();
                        if let Some(expr) = &f.expr {
                            let mut e = (**expr).clone();
                            if offset_references(&mut e, dr, dc) {
                                let array = f.array;
                                *f = Formula { array, ..Formula::from_expr(e) };
                            }
                        }
                        f.array = f.array.map(|a| Range::new(at, a.end.offset(dr, dc).unwrap_or(at)));
                        f
                    });
                    new.extra = cell.extra.clone().map(|mut e| {
                        e.array_member = e.array_member.and_then(|m| m.offset(dr, dc));
                        e
                    });
                    if what == PasteWhat::All {
                        new.style = cell.style;
                    }
                }
                PasteWhat::Values => {
                    new.value = cell.value.clone();
                    new.formula = None;
                    new.extra = None;
                }
                PasteWhat::Formats => new.style = cell.style,
            }
            self.sheets[to_sheet].set(at, new);
        }
        Ok(())
    }

    fn copy_block(&mut self, from_sheet: usize, from: Range, to_sheet: usize, to: CellRef, what: PasteWhat, rec: &mut Recorder) -> Result<(), EditError> {
        let block = self.read_block(from_sheet, from);
        self.write_block(from_sheet, from, &block, to_sheet, to, what, rec)
    }

    fn sort(&mut self, sheet: usize, range: Range, keys: &[SortKey], header: bool, rec: &mut Recorder) -> Result<(), EditError> {
        let mut range = range;
        if range.rows() == MAX_ROWS || range.cols() == MAX_COLS {
            let Some(used) = self.sheets[sheet].used_range() else { return Ok(()) };
            match range.intersection(used) {
                Some(r) => range = r,
                None => return Ok(()),
            }
        }
        if header {
            if range.rows() < 2 {
                return Ok(());
            }
            range.start.row += 1;
        }
        if keys.is_empty() || keys.iter().any(|k| k.col < range.start.col || k.col > range.end.col) {
            return refuse("Sort by a column inside the selection.");
        }
        if self.sheets[sheet].merges.iter().any(|m| m.intersects(range)) {
            return refuse("To sort, unmerge the cells first.");
        }
        let rows: Vec<u32> = (range.start.row..=range.end.row).collect();
        let s = &self.sheets[sheet];
        let mut order: Vec<u32> = rows.clone();
        order.sort_by(|a, b| {
            for k in keys {
                let va = s.value(CellRef::new(*a, k.col));
                let vb = s.value(CellRef::new(*b, k.col));
                // Blanks always go last.
                let ord = match (va.is_empty(), vb.is_empty()) {
                    (true, true) => std::cmp::Ordering::Equal,
                    (true, false) => return std::cmp::Ordering::Greater,
                    (false, true) => return std::cmp::Ordering::Less,
                    _ => compare(&va, &vb),
                };
                let ord = if k.ascending { ord } else { ord.reverse() };
                if ord != std::cmp::Ordering::Equal {
                    return ord;
                }
            }
            std::cmp::Ordering::Equal
        });
        if order == rows {
            return Ok(());
        }
        let width = Range::new(CellRef::new(0, range.start.col), CellRef::new(0, range.end.col));
        let blocks: Vec<Vec<(u32, u32, Cell)>> = order
            .iter()
            .map(|r| {
                let from = Range::new(CellRef::new(*r, width.start.col), CellRef::new(*r, width.end.col));
                self.read_block(sheet, from)
            })
            .collect();
        for (i, block) in blocks.iter().enumerate() {
            let src_row = order[i];
            let dst_row = range.start.row + i as u32;
            let from = Range::new(CellRef::new(src_row, width.start.col), CellRef::new(src_row, width.end.col));
            self.write_block(sheet, from, block, sheet, CellRef::new(dst_row, width.start.col), PasteWhat::All, rec)?;
        }
        Ok(())
    }

    /// Replace text in constant cells. Returns the number replaced.
    fn replace_in(
        &mut self,
        sheet: usize,
        find: &str,
        replace: &str,
        match_case: bool,
        whole: bool,
        mut rec: Option<&mut Recorder>,
    ) -> Result<usize, EditError> {
        if find.is_empty() {
            return Ok(0);
        }
        let mut targets = Vec::new();
        for (&r, row) in &self.sheets[sheet].cells {
            for (&c, cell) in row {
                if cell.formula.is_some() {
                    continue;
                }
                let Value::Text(t) = &cell.value else { continue };
                if let Some(new) = replace_text(t, find, replace, match_case, whole) {
                    targets.push((CellRef::new(r, c), new));
                }
            }
        }
        let n = targets.len();
        for (at, new) in targets {
            if let Some(r) = rec.as_deref_mut() {
                r.touch(self, at);
            }
            let s = &mut self.sheets[sheet];
            if let Some(cell) = s.cell_mut(at) {
                cell.value = Value::text(new);
                cell.clear_source();
            }
            s.modified = true;
        }
        Ok(n)
    }

    // ----- structural edits

    fn apply_structural(&mut self, op: Op) -> Result<(), EditError> {
        match op {
            Op::InsertRows { sheet, at, count } => self.shift(sheet, Shift::InsertRows { at, count }),
            Op::DeleteRows { sheet, at, count } => self.shift(sheet, Shift::DeleteRows { at, count }),
            Op::InsertCols { sheet, at, count } => self.shift(sheet, Shift::InsertCols { at, count }),
            Op::DeleteCols { sheet, at, count } => self.shift(sheet, Shift::DeleteCols { at, count }),
            Op::AddSheet { name, at } => {
                let name = match name {
                    Some(n) => {
                        if !Workbook::valid_sheet_name(&n) {
                            return refuse("Sheet names can't be blank, longer than 31 characters, or use : \\ / ? * [ ].");
                        }
                        if self.sheet_index(&n).is_some() {
                            return refuse("A sheet with that name already exists.");
                        }
                        n
                    }
                    None => self.unused_sheet_name("Sheet"),
                };
                let at = at.unwrap_or(self.sheets.len()).min(self.sheets.len());
                let mut sheet = Sheet::new(name);
                sheet.modified = true;
                self.sheets.insert(at, sheet);
                for n in &mut self.names {
                    if let Some(s) = &mut n.sheet {
                        if *s >= at {
                            *s += 1;
                        }
                    }
                }
                if self.active >= at && self.sheets.len() > 1 {
                    self.active += 1;
                }
                self.active = at;
                Ok(())
            }
            Op::RenameSheet { sheet, name } => {
                self.check_sheet(sheet)?;
                let name = name.trim().to_owned();
                if !Workbook::valid_sheet_name(&name) {
                    return refuse("Sheet names can't be blank, longer than 31 characters, or use : \\ / ? * [ ].");
                }
                if let Some(i) = self.sheet_index(&name) {
                    if i != sheet {
                        return refuse("A sheet with that name already exists.");
                    }
                }
                let old = std::mem::replace(&mut self.sheets[sheet].name, name.clone());
                self.rewrite_formulas(|e, _| rename_sheet(e, &old, &name));
                for n in &mut self.names {
                    if let Some(expr) = &n.expr {
                        let mut e = (**expr).clone();
                        if rename_sheet(&mut e, &old, &name) {
                            *n = DefinedName { extra: n.extra.clone(), hidden: n.hidden, ..DefinedName::new(&n.name, n.sheet, &crate::formula::unparse(&e)) };
                        }
                    }
                }
                Ok(())
            }
            Op::DeleteSheet { sheet } => {
                self.check_sheet(sheet)?;
                let visible = self.sheets.iter().filter(|s| s.state == SheetState::Visible).count();
                if visible <= 1 && self.sheets[sheet].state == SheetState::Visible {
                    return refuse("A workbook must keep at least one visible sheet.");
                }
                let removed = self.sheets.remove(sheet);
                self.rewrite_formulas(|e, _| invalidate_sheet(e, &removed.name));
                self.names.retain(|n| n.sheet != Some(sheet));
                for n in &mut self.names {
                    if let Some(s) = &mut n.sheet {
                        if *s > sheet {
                            *s -= 1;
                        }
                    }
                    if let Some(expr) = &n.expr {
                        let mut e = (**expr).clone();
                        if invalidate_sheet(&mut e, &removed.name) {
                            *n = DefinedName { extra: n.extra.clone(), hidden: n.hidden, ..DefinedName::new(&n.name, n.sheet, &crate::formula::unparse(&e)) };
                        }
                    }
                }
                if self.active >= self.sheets.len() || self.active > sheet {
                    self.active = self.active.saturating_sub(1);
                }
                Ok(())
            }
            Op::MoveSheet { sheet, to } => {
                self.check_sheet(sheet)?;
                let to = to.min(self.sheets.len() - 1);
                if to == sheet {
                    return Ok(());
                }
                let s = self.sheets.remove(sheet);
                self.sheets.insert(to, s);
                let map = |i: usize| -> usize {
                    if i == sheet {
                        to
                    } else if sheet < to && i > sheet && i <= to {
                        i - 1
                    } else if sheet > to && i >= to && i < sheet {
                        i + 1
                    } else {
                        i
                    }
                };
                for n in &mut self.names {
                    n.sheet = n.sheet.map(map);
                }
                self.active = map(self.active);
                Ok(())
            }
            Op::DuplicateSheet { sheet } => {
                self.check_sheet(sheet)?;
                let mut copy = self.sheets[sheet].clone();
                let stem = format!("{} (", copy.name.chars().take(26).collect::<String>());
                copy.name = (2..)
                    .map(|i| format!("{stem}{i})"))
                    .find(|n| self.sheet_index(n).is_none())
                    .unwrap_or_else(|| self.unused_sheet_name("Sheet"));
                copy.modified = true;
                copy.state = SheetState::Visible;
                if let Some(part) = &mut copy.xlsx {
                    part.detach();
                }
                copy.ods = None;
                let at = sheet + 1;
                self.sheets.insert(at, copy);
                for n in &mut self.names {
                    if let Some(s) = &mut n.sheet {
                        if *s >= at {
                            *s += 1;
                        }
                    }
                }
                self.active = at;
                Ok(())
            }
            Op::HideSheet { sheet, hidden } => {
                self.check_sheet(sheet)?;
                if hidden {
                    let visible = self.sheets.iter().filter(|s| s.state == SheetState::Visible).count();
                    if visible <= 1 && self.sheets[sheet].state == SheetState::Visible {
                        return refuse("A workbook must keep at least one visible sheet.");
                    }
                    self.sheets[sheet].state = SheetState::Hidden;
                    if self.active == sheet {
                        self.active = self.sheets.iter().position(|s| s.state == SheetState::Visible).unwrap_or(0);
                    }
                } else {
                    self.sheets[sheet].state = SheetState::Visible;
                }
                Ok(())
            }
            Op::Paste { from_sheet, from, to_sheet, to, what, cut } => {
                self.check_sheet(from_sheet)?;
                self.check_sheet(to_sheet)?;
                let block = self.read_block(from_sheet, from);
                let mut rec = Recorder::new(to_sheet);
                if cut {
                    // Moving cells keeps their formulas as they were and
                    // points other formulas at the new place.
                    for (r, c, _) in &block {
                        self.sheets[from_sheet].remove(CellRef::new(from.start.row + r, from.start.col + c));
                    }
                    let target_name = self.sheets[to_sheet].name.clone();
                    let source_name = self.sheets[from_sheet].name.clone();
                    let Some(end) = to.offset(from.rows() as i64 - 1, from.cols() as i64 - 1) else {
                        return refuse("The pasted cells do not fit on the sheet.");
                    };
                    let existing: Vec<CellRef> = self.sheets[to_sheet].cells_in(Range::new(to, end)).map(|(a, _)| a).collect();
                    for at in existing {
                        self.sheets[to_sheet].remove(at);
                    }
                    for (r, c, cell) in block {
                        let mut cell = cell;
                        if from_sheet != to_sheet {
                            // Unqualified references now live on another sheet.
                            if let Some(f) = &mut cell.formula {
                                if let Some(expr) = &f.expr {
                                    let mut e = (**expr).clone();
                                    qualify_references(&mut e, &source_name);
                                    **f = Formula { array: f.array, ..Formula::from_expr(e) };
                                }
                            }
                        }
                        let dr = to.row as i64 - from.start.row as i64;
                        let dc = to.col as i64 - from.start.col as i64;
                        if let Some(f) = &mut cell.formula {
                            f.array = f.array.and_then(|a| Some(Range::new(a.start.offset(dr, dc)?, a.end.offset(dr, dc)?)));
                        }
                        self.sheets[to_sheet].set(CellRef::new(to.row + r, to.col + c), cell);
                    }
                    let _ = target_name;
                    self.rewrite_formulas(|e, formula_sheet| {
                        if from_sheet == to_sheet {
                            move_block(e, formula_sheet, &source_name, from, to)
                        } else {
                            false
                        }
                    });
                    self.sheets[from_sheet].modified = true;
                    Ok(())
                } else {
                    self.write_block(from_sheet, from, &block, to_sheet, to, what, &mut rec)
                }
            }
            Op::Replace { sheet: None, find, replace, match_case, whole } => {
                for s in 0..self.sheets.len() {
                    self.replace_in(s, &find, &replace, match_case, whole, None)?;
                }
                Ok(())
            }
            Op::DefineName { name, sheet, refers_to } => {
                let valid = !name.is_empty()
                    && name.chars().next().is_some_and(|c| c.is_alphabetic() || c == '_' || c == '\\')
                    && name.chars().all(|c| c.is_alphanumeric() || c == '_' || c == '.' || c == '\\')
                    && CellRef::parse_a1(&name).is_none();
                if !valid {
                    return refuse("Names start with a letter and use only letters, numbers, _ and periods.");
                }
                let text = refers_to.trim_start_matches('=');
                if crate::formula::parse(text).is_err() {
                    return refuse("That is not a valid reference or formula.");
                }
                self.names.retain(|n| !(n.sheet == sheet && n.name.eq_ignore_ascii_case(&name)));
                self.names.push(DefinedName::new(&name, sheet, text));
                Ok(())
            }
            Op::DeleteName { name, sheet } => {
                self.names.retain(|n| !(n.sheet == sheet && n.name.eq_ignore_ascii_case(&name)));
                Ok(())
            }
            other => refuse(format!("{other:?} is not a structural edit")),
        }
    }

    /// Rewrite every formula with `f(expr, formula's sheet name)`, marking
    /// changed sheets as modified.
    fn rewrite_formulas(&mut self, mut f: impl FnMut(&mut crate::formula::Expr, &str) -> bool) {
        for sheet in &mut self.sheets {
            let name = sheet.name.clone();
            let mut any = false;
            for row in sheet.cells.values_mut() {
                for cell in row.values_mut() {
                    if let Some(formula) = &mut cell.formula {
                        if let Some(expr) = &formula.expr {
                            let mut e = (**expr).clone();
                            if f(&mut e, &name) {
                                let array = formula.array;
                                **formula = Formula { array, ..Formula::from_expr(e) };
                                any = true;
                            }
                        }
                    }
                }
            }
            if any {
                sheet.modified = true;
            }
        }
    }

    /// Insert or delete rows or columns on one sheet.
    fn shift(&mut self, sheet: usize, shift: Shift) -> Result<(), EditError> {
        self.check_sheet(sheet)?;
        let (rows, insert, at, count) = match shift {
            Shift::InsertRows { at, count } => (true, true, at, count),
            Shift::DeleteRows { at, count } => (true, false, at, count),
            Shift::InsertCols { at, count } => (false, true, at, count),
            Shift::DeleteCols { at, count } => (false, false, at, count),
        };
        let max = if rows { MAX_ROWS } else { MAX_COLS };
        if count == 0 || at >= max {
            return Ok(());
        }
        let count = count.min(max - at);
        {
            let s = &self.sheets[sheet];
            if insert {
                if let Some(used) = s.used_range() {
                    let last = if rows { used.end.row } else { used.end.col };
                    if last as u64 + count as u64 >= max as u64 && last >= at {
                        return refuse("There isn't room to insert: it would push cells off the end of the sheet.");
                    }
                }
            }
            // Array formulas cannot be cut in half.
            for (_, row) in &s.cells {
                for cell in row.values() {
                    if let Some(range) = cell.formula.as_ref().and_then(|f| f.array) {
                        let (lo, hi) = if rows { (range.start.row, range.end.row) } else { (range.start.col, range.end.col) };
                        let cut = if insert { at > lo && at <= hi } else { at <= hi && at + count > lo && !(at <= lo && at + count > hi) };
                        if cut && range.cell_count() > 1 {
                            return refuse("You can't change part of an array formula.");
                        }
                    }
                }
            }
        }
        let target_name = self.sheets[sheet].name.clone();
        // Move the cells.
        let s = &mut self.sheets[sheet];
        let old = std::mem::take(&mut s.cells);
        for (r, row) in old {
            for (c, cell) in row {
                let (idx, other) = if rows { (r, c) } else { (c, r) };
                let Some(n) = shift_index(idx, insert, at, count, max) else { continue };
                if !insert && idx >= at && idx < at + count {
                    continue;
                }
                let new = if rows { CellRef::new(n, other) } else { CellRef::new(other, n) };
                let mut cell = cell;
                if let Some(extra) = &mut cell.extra {
                    if let Some(m) = extra.array_member {
                        extra.array_member = shift_range(Range::cell(m), shift).map(|r| r.start);
                    }
                }
                s.cells.entry(new.row).or_default().insert(new.col, cell);
            }
        }
        s.invalidate_used();
        // Rows and columns, merges.
        if rows {
            s.rows = shift_map(std::mem::take(&mut s.rows), insert, at, count, max);
        } else {
            s.cols = shift_map(std::mem::take(&mut s.cols), insert, at, count, max);
        }
        s.merges = s.merges.iter().filter_map(|m| shift_range(*m, shift)).filter(|m| m.cell_count() > 1).collect();
        if let Some(part) = &mut s.xlsx {
            part.shift(shift);
        }
        s.modified = true;
        // References everywhere.
        self.rewrite_formulas(|e, formula_sheet| shift_references(e, formula_sheet, &target_name, shift));
        for sh in &mut self.sheets {
            for row in sh.cells.values_mut() {
                for cell in row.values_mut() {
                    if let Some(f) = &mut cell.formula {
                        if let Some(a) = f.array {
                            if sh.name == target_name {
                                f.array = shift_range(a, shift);
                            }
                        }
                    }
                }
            }
        }
        for n in &mut self.names {
            if let Some(expr) = &n.expr {
                let mut e = (**expr).clone();
                // A name's unqualified references are taken as on its own
                // sheet (or on none for global names).
                let own = n.sheet.and_then(|i| self.sheets.get(i)).map_or("", |s| s.name.as_str()).to_owned();
                if shift_references(&mut e, &own, &target_name, shift) {
                    *n = DefinedName { extra: n.extra.clone(), hidden: n.hidden, ..DefinedName::new(&n.name, n.sheet, &crate::formula::unparse(&e)) };
                }
            }
        }
        Ok(())
    }
}

fn op_sheet(op: &Op) -> usize {
    match op {
        Op::SetCell { sheet, .. }
        | Op::SetCells { sheet, .. }
        | Op::Clear { sheet, .. }
        | Op::Format { sheet, .. }
        | Op::ColWidth { sheet, .. }
        | Op::RowHeight { sheet, .. }
        | Op::HideRows { sheet, .. }
        | Op::HideCols { sheet, .. }
        | Op::Merge { sheet, .. }
        | Op::Unmerge { sheet, .. }
        | Op::Freeze { sheet, .. }
        | Op::Fill { sheet, .. }
        | Op::Sort { sheet, .. } => *sheet,
        Op::Replace { sheet: Some(sheet), .. } => *sheet,
        Op::Paste { to_sheet, .. } => *to_sheet,
        _ => 0,
    }
}

/// Whole rows and columns stop at the sheet's edge.
fn clip(range: Range) -> Range {
    Range::new(
        range.start,
        CellRef::new(range.end.row.min(MAX_ROWS - 1), range.end.col.min(MAX_COLS - 1)),
    )
}

fn shift_map<T>(map: BTreeMap<u32, T>, insert: bool, at: u32, count: u32, max: u32) -> BTreeMap<u32, T> {
    map.into_iter()
        .filter(|(i, _)| insert || !(*i >= at && *i < at + count))
        .filter_map(|(i, v)| shift_index(i, insert, at, count, max).map(|n| (n, v)))
        .collect()
}

/// Give a formula's own-sheet references an explicit sheet (before the
/// formula moves to another sheet).
fn qualify_references(expr: &mut crate::formula::Expr, sheet: &str) {
    expr.walk_mut(&mut |e| {
        if let crate::formula::Expr::Ref(r) = e {
            if r.sheet.is_none() {
                r.sheet = Some(sheet.to_owned());
            }
        }
    });
}

/// The border edges a cell gets from a border preset over a range.
fn edges_for(kind: BorderKind, range: Range, at: CellRef, style: &str, color: &Option<ColorSpec>) -> [(bool, Option<Edge>); 4] {
    let edge = || {
        Some(Edge {
            style: style.to_owned(),
            color: color.clone(),
        })
    };
    let top = at.row == range.start.row;
    let bottom = at.row == range.end.row;
    let left = at.col == range.start.col;
    let right = at.col == range.end.col;
    // (set?, value) for left, right, top, bottom.
    let on = |cond: bool| if cond { (true, edge()) } else { (false, None) };
    match kind {
        BorderKind::All => [(true, edge()), (true, edge()), (true, edge()), (true, edge())],
        BorderKind::None => [(true, None), (true, None), (true, None), (true, None)],
        BorderKind::Outline => [on(left), on(right), on(top), on(bottom)],
        BorderKind::Inside => [on(!left), on(!right), on(!top), on(!bottom)],
        BorderKind::Top => [(false, None), (false, None), on(top), (false, None)],
        BorderKind::Bottom => [(false, None), (false, None), (false, None), on(bottom)],
        BorderKind::Left => [on(left), (false, None), (false, None), (false, None)],
        BorderKind::Right => [(false, None), on(right), (false, None), (false, None)],
    }
}

fn apply_patch(style: &mut CellStyle, p: &StylePatch, edges: Option<&[(bool, Option<Edge>); 4]>) {
    if let Some(b) = p.bold {
        style.font.bold = b;
    }
    if let Some(i) = p.italic {
        style.font.italic = i;
    }
    if let Some(u) = p.underline {
        style.font.underline = u;
    }
    if let Some(s) = p.strike {
        style.font.strike = s;
    }
    if let Some(n) = &p.font_name {
        style.font.name = n.clone();
        style.font.scheme = None;
    }
    if let Some(sz) = p.font_size {
        style.font.size = sz.clamp(100, 40_900);
    }
    if let Some(c) = &p.font_color {
        style.font.color = c.clone();
    }
    if let Some(f) = &p.fill {
        style.fill = match f {
            Some(c) => Fill::solid(c.clone()),
            None => Fill::none(),
        };
    }
    if let Some(h) = &p.h_align {
        style.align.horizontal = if h == "general" { String::new() } else { h.clone() };
    }
    if let Some(v) = &p.v_align {
        style.align.vertical = if v == "bottom" { String::new() } else { v.clone() };
    }
    if let Some(w) = p.wrap {
        style.align.wrap = w;
    }
    if let Some(i) = p.indent {
        style.align.indent = i.min(250);
    }
    if let Some(f) = &p.num_fmt {
        style.num_fmt = f.clone();
    }
    if let Some(delta) = p.decimals_delta {
        style.num_fmt = change_decimals(&style.num_fmt, delta);
    }
    if let Some(edges) = edges {
        let b = &mut style.border;
        let slots = [&mut b.left, &mut b.right, &mut b.top, &mut b.bottom];
        for (slot, (set, value)) in slots.into_iter().zip(edges.iter()) {
            if *set {
                *slot = value.clone();
            }
        }
    }
}

/// A number format with one decimal place more or fewer.
pub fn change_decimals(code: &str, delta: i32) -> String {
    let fmt = crate::numfmt::NumberFormat::cached(code);
    if fmt.is_date() || fmt.is_text() {
        return code.to_owned();
    }
    let current = fmt.decimals() as i32;
    let target = (current + delta).clamp(0, 15) as usize;
    if fmt.is_general() {
        return if target == 0 { "0".into() } else { format!("0.{}", "0".repeat(target)) };
    }
    // Rewrite the digits after the first decimal point in every section.
    code.split(';')
        .map(|section| {
            let mut out = String::new();
            let mut chars = section.chars().peekable();
            let mut done = false;
            let mut in_quote = false;
            let mut in_bracket = false;
            while let Some(c) = chars.next() {
                match c {
                    '"' => in_quote = !in_quote,
                    '[' if !in_quote => in_bracket = true,
                    ']' if !in_quote => in_bracket = false,
                    _ => {}
                }
                if !done && !in_quote && !in_bracket && c == '.' {
                    while chars.peek().is_some_and(|d| matches!(d, '0' | '#' | '?')) {
                        chars.next();
                    }
                    if target > 0 {
                        out.push('.');
                        out.push_str(&"0".repeat(target));
                    }
                    done = true;
                    continue;
                }
                if !done && !in_quote && !in_bracket && c == '0' && chars.peek().is_none_or(|d| !matches!(d, '0' | '#' | ',' | '.')) {
                    out.push('0');
                    if target > 0 {
                        out.push('.');
                        out.push_str(&"0".repeat(target));
                    }
                    done = true;
                    continue;
                }
                out.push(c);
            }
            out
        })
        .collect::<Vec<_>>()
        .join(";")
}

fn replace_text(text: &str, find: &str, replace: &str, match_case: bool, whole: bool) -> Option<String> {
    if whole {
        let hit = if match_case { text == find } else { text.to_lowercase() == find.to_lowercase() };
        return hit.then(|| replace.to_owned());
    }
    if match_case {
        return text.contains(find).then(|| text.replace(find, replace));
    }
    let lower = text.to_lowercase();
    let needle = find.to_lowercase();
    if !lower.contains(&needle) || lower.len() != text.len() {
        return (lower.contains(&needle)).then(|| lower.replace(&needle, replace));
    }
    let mut out = String::new();
    let mut i = 0;
    while let Some(p) = lower[i..].find(&needle) {
        out.push_str(&text[i..i + p]);
        out.push_str(replace);
        i += p + needle.len();
    }
    out.push_str(&text[i..]);
    Some(out)
}

/// A matching cell for find.
#[derive(Clone, Debug, PartialEq)]
pub struct Found {
    pub sheet: usize,
    pub cell: CellRef,
}

impl Workbook {
    /// Find cells whose shown text (or formula) contains `text`.
    pub fn find(&self, text: &str, sheet: Option<usize>, match_case: bool, whole: bool, in_formulas: bool, limit: usize) -> Vec<Found> {
        let mut out = Vec::new();
        if text.is_empty() {
            return out;
        }
        let needle = if match_case { text.to_owned() } else { text.to_lowercase() };
        let sheets: Vec<usize> = match sheet {
            Some(s) => vec![s],
            None => (0..self.sheets.len()).collect(),
        };
        for s in sheets {
            let Some(sh) = self.sheets.get(s) else { continue };
            for (&r, row) in &sh.cells {
                for (&c, cell) in row {
                    let mut hay = match (&cell.formula, in_formulas) {
                        (Some(f), true) => format!("={}", f.text),
                        _ => crate::display::shown_text(self, cell),
                    };
                    if !match_case {
                        hay = hay.to_lowercase();
                    }
                    let hit = if whole { hay == needle } else { hay.contains(&needle) };
                    if hit {
                        out.push(Found { sheet: s, cell: CellRef::new(r, c) });
                        if out.len() >= limit {
                            return out;
                        }
                    }
                }
            }
        }
        out
    }
}
