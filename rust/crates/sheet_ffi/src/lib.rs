//! The FFI layer of the spreadsheet engine, in Rust, for `dart:ffi`.
//!
//! Conventions are those of `doc_ffi`: UTF-8 strings, caller-owned results
//! freed with [`sheet_free_string`], negative or NULL results on failure with
//! the reason in [`sheet_last_error`]. Rows and columns are 0-based.
//! Structured data crosses as JSON; edits are JSON operations (see
//! [`sheet_apply`]).

use std::ffi::c_char;

use doc_ffi::{call_i64, call_str, codes, last_error_ptr, opt_str_arg, read_file, str_arg, write_file_atomically, FfiError, Registry};
use serde::Deserialize;
use serde_json::{json, Value as Json};
use sheet_core::edit::{BorderKind, ClearWhat, EditError, EditResult, Op, PasteWhat, SortKey, StylePatch};
use sheet_core::numfmt::{DateOrder, FormatOptions};
use sheet_core::style::{ColorSpec, Underline};
use sheet_core::{CellRef, Format, Range, SheetError, Workbook};

pub use sheet_core;

/// Files larger than this are refused rather than read into memory.
const MAX_FILE_BYTES: u64 = 256 << 20;

static BOOKS: Registry<Workbook> = Registry::new();

fn sheet_error(e: SheetError) -> FfiError {
    match e {
        SheetError::Encrypted => FfiError::new(codes::ENCRYPTED, e.to_string()),
        SheetError::Corrupt(m) => FfiError::corrupt(m),
        SheetError::Unsupported(m) => FfiError::unsupported(m),
        SheetError::TooLarge(m) => FfiError::new(codes::TOO_LARGE, m),
    }
}

fn edit_error(e: EditError) -> FfiError {
    FfiError::invalid(e.0)
}

fn json_err(e: serde_json::Error) -> FfiError {
    FfiError::invalid(format!("bad request: {e}"))
}

fn to_json(v: &Json) -> Result<String, FfiError> {
    serde_json::to_string(v).map_err(|e| FfiError::new(codes::ERROR, e.to_string()))
}

#[derive(Deserialize, Default)]
#[serde(rename_all = "camelCase", default)]
struct Options {
    /// `dmy`, `mdy` or `ymd`.
    date_order: Option<String>,
    utc_offset_minutes: Option<i32>,
}

fn date_order(o: &Options) -> DateOrder {
    match o.date_order.as_deref() {
        Some("mdy") => DateOrder::Mdy,
        Some("ymd") => DateOrder::Ymd,
        _ => DateOrder::Dmy,
    }
}

fn options(json: Option<&str>) -> Result<Options, FfiError> {
    match json {
        Some(j) => serde_json::from_str(j).map_err(json_err),
        None => Ok(Options::default()),
    }
}

/// Library version (static; do not free).
#[no_mangle]
pub extern "C" fn sheet_core_version() -> *const c_char {
    concat!(env!("CARGO_PKG_VERSION"), "\0").as_ptr() as *const c_char
}

/// This thread's last error, `"CODE: message"` (borrowed; do not free).
#[no_mangle]
pub extern "C" fn sheet_last_error() -> *const c_char {
    last_error_ptr()
}

/// Free a string returned by this library.
///
/// # Safety
/// `ptr` must be NULL or a string from this library, not yet freed.
#[no_mangle]
pub unsafe extern "C" fn sheet_free_string(ptr: *mut c_char) {
    doc_ffi::free_c_string(ptr)
}

/// Open a workbook (XLSX, XLSM, ODS, CSV, TSV). `name` gives the file name
/// for its extension (the path may not have one, e.g. a content URI copy);
/// NULL uses `path`. `options_json`: `{"dateOrder":"dmy","utcOffsetMinutes":330}`.
/// Returns a handle, or -1.
///
/// # Safety
/// String arguments must be NULL or valid NUL-terminated UTF-8.
#[no_mangle]
pub unsafe extern "C" fn sheet_open(path: *const c_char, name: *const c_char, options_json: *const c_char) -> i64 {
    call_i64(|| {
        let path = str_arg(path, "path")?;
        let name = opt_str_arg(name, "name")?.unwrap_or(path);
        let opts = options(opt_str_arg(options_json, "options")?)?;
        let bytes = read_file(path, MAX_FILE_BYTES)?;
        let mut wb = Workbook::open(bytes, name, date_order(&opts)).map_err(sheet_error)?;
        if let Some(m) = opts.utc_offset_minutes {
            wb.set_utc_offset(m);
        }
        Ok(BOOKS.insert(wb))
    })
}

/// A new workbook with one empty sheet. Returns a handle.
///
/// # Safety
/// `options_json` must be NULL or valid NUL-terminated UTF-8.
#[no_mangle]
pub unsafe extern "C" fn sheet_new(options_json: *const c_char) -> i64 {
    call_i64(|| {
        let opts = options(opt_str_arg(options_json, "options")?)?;
        let mut wb = Workbook::blank(date_order(&opts));
        if let Some(m) = opts.utc_offset_minutes {
            wb.set_utc_offset(m);
        }
        Ok(BOOKS.insert(wb))
    })
}

/// Close a workbook. Returns 0, or -1 for an unknown handle.
#[no_mangle]
pub extern "C" fn sheet_close(handle: i64) -> i64 {
    call_i64(|| {
        if BOOKS.remove(handle) {
            Ok(0)
        } else {
            Err(FfiError::new(codes::BAD_HANDLE, format!("no open workbook has handle {handle}")))
        }
    })
}

fn range_json(r: Range) -> Json {
    json!([r.start.row, r.start.col, r.end.row, r.end.col])
}

fn info(wb: &Workbook) -> Json {
    let sheets: Vec<Json> = wb
        .sheets
        .iter()
        .map(|s| {
            json!({
                "name": s.name,
                "state": s.state.name(),
                "used": s.used_range().map(range_json),
                "freeze": s.freeze.map(|f| json!([f.row, f.col])),
                "showGrid": s.show_grid,
                "defaultColWidth": s.default_col_width,
                "defaultRowHeight": s.default_row_height,
                "chart": s.xlsx.as_ref().is_some_and(|p| p.chart),
            })
        })
        .collect();
    json!({
        "format": wb.format.map(|f| f.name()),
        "sheets": sheets,
        "active": wb.active,
        "modified": wb.dirty,
        "canUndo": wb.can_undo(),
        "canRedo": wb.can_redo(),
        "warnings": wb.warnings,
        "circular": wb.circular_references().iter().map(|(s, c)| json!([s, c.row, c.col])).collect::<Vec<_>>(),
        "names": wb.names.iter().filter(|n| !n.hidden && !n.name.starts_with("_xlnm")).map(|n| json!({"name": n.name, "sheet": n.sheet, "refersTo": n.text})).collect::<Vec<_>>(),
        "date1904": wb.date1904,
    })
}

/// The workbook: format, sheets (name, state, used range, frozen panes),
/// active sheet, whether it has unsaved edits, undo/redo availability,
/// warnings and circular references.
#[no_mangle]
pub extern "C" fn sheet_info_json(handle: i64) -> *mut c_char {
    call_str(|| BOOKS.with(handle, |wb| to_json(&info(wb))))
}

fn check_sheet(wb: &Workbook, sheet: i64) -> Result<usize, FfiError> {
    usize::try_from(sheet)
        .ok()
        .filter(|s| *s < wb.sheets.len())
        .ok_or_else(|| FfiError::invalid(format!("no sheet {sheet}")))
}

/// The cells in a window of a sheet, for drawing: `{"cells": [[row, col,
/// text, kind, style, color, formula]], "merges": [[r0,c0,r1,c1]], "cols":
/// [[col, width, hidden]], "rows": [[row, height, hidden]]}`. `kind` is `n`
/// number, `s` text, `b` boolean, `e` error. `color` is the number format's
/// colour as `#rrggbb` or null. Columns and rows are listed only when they
/// differ from the defaults.
#[no_mangle]
pub extern "C" fn sheet_cells_json(handle: i64, sheet: i64, r0: i64, c0: i64, r1: i64, c1: i64) -> *mut c_char {
    call_str(|| {
        BOOKS.with(handle, |wb| {
            let s = check_sheet(wb, sheet)?;
            let clamp_r = |v: i64| v.clamp(0, sheet_core::refs::MAX_ROWS as i64 - 1) as u32;
            let clamp_c = |v: i64| v.clamp(0, sheet_core::refs::MAX_COLS as i64 - 1) as u32;
            let window = Range::new(CellRef::new(clamp_r(r0), clamp_c(c0)), CellRef::new(clamp_r(r1), clamp_c(c1)));
            let sh = &wb.sheets[s];
            let mut cells = Vec::new();
            for (at, cell) in sh.cells_in(window) {
                if cell.value.is_empty() && cell.formula.is_none() && cell.style == 0 {
                    continue;
                }
                let shown = sheet_core::display::shown(wb, cell);
                cells.push(json!([
                    at.row,
                    at.col,
                    shown.text,
                    shown.kind,
                    cell.style,
                    shown.color.map(|c| format!("#{c:06x}")),
                    cell.formula.is_some()
                ]));
            }
            // Text that spills into empty neighbours to the left needs its
            // source cell even when it starts outside the window.
            let merges: Vec<Json> = sh.merges.iter().filter(|m| m.intersects(window)).map(|m| range_json(*m)).collect();
            let cols: Vec<Json> = sh
                .cols
                .range(window.start.col..=window.end.col)
                .filter(|(_, i)| i.width.is_some() || i.hidden)
                .map(|(c, i)| json!([c, i.width.unwrap_or(sh.default_col_width), i.hidden]))
                .collect();
            let rows: Vec<Json> = sh
                .rows
                .range(window.start.row..=window.end.row)
                .filter(|(_, i)| i.height.is_some() || i.hidden)
                .map(|(r, i)| json!([r, i.height.unwrap_or(sh.default_row_height), i.hidden]))
                .collect();
            to_json(&json!({"cells": cells, "merges": merges, "cols": cols, "rows": rows}))
        })
    })
}

/// Column widths (characters) and row heights (points) of a whole sheet,
/// where they differ from the defaults: `{"cols": [[c, w, hidden]], "rows":
/// [[r, h, hidden]]}`. For laying out the grid.
#[no_mangle]
pub extern "C" fn sheet_layout_json(handle: i64, sheet: i64) -> *mut c_char {
    call_str(|| {
        BOOKS.with(handle, |wb| {
            let s = check_sheet(wb, sheet)?;
            let sh = &wb.sheets[s];
            let cols: Vec<Json> = sh
                .cols
                .iter()
                .filter(|(_, i)| i.width.is_some() || i.hidden)
                .map(|(c, i)| json!([c, i.width.unwrap_or(sh.default_col_width), i.hidden]))
                .collect();
            let rows: Vec<Json> = sh
                .rows
                .iter()
                .filter(|(_, i)| i.height.is_some() || i.hidden)
                .map(|(r, i)| json!([r, i.height.unwrap_or(sh.default_row_height), i.hidden]))
                .collect();
            to_json(&json!({"cols": cols, "rows": rows, "merges": sh.merges.iter().map(|m| range_json(*m)).collect::<Vec<_>>()}))
        })
    })
}

fn hex(c: Option<u32>) -> Json {
    c.map_or(Json::Null, |c| Json::String(format!("#{c:06x}")))
}

/// Resolved styles by id. `ids_json` is a JSON array of style ids.
///
/// # Safety
/// `ids_json` must be a valid NUL-terminated UTF-8 string.
#[no_mangle]
pub unsafe extern "C" fn sheet_styles_json(handle: i64, ids_json: *const c_char) -> *mut c_char {
    call_str(|| {
        let ids: Vec<u32> = serde_json::from_str(str_arg(ids_json, "ids")?).map_err(json_err)?;
        BOOKS.with(handle, |wb| {
            let theme = &wb.styles.theme;
            let list: Vec<Json> = ids
                .iter()
                .map(|&id| {
                    let s = wb.styles.get(id);
                    let edge = |e: &Option<sheet_core::style::Edge>| {
                        e.as_ref().map(|e| json!([e.style, hex(e.color.as_ref().and_then(|c| c.resolve(theme)).or(Some(0)))]))
                    };
                    json!({
                        "id": id,
                        "font": s.font.name,
                        "size": s.font.size as f64 / 100.0,
                        "bold": s.font.bold,
                        "italic": s.font.italic,
                        "underline": s.font.underline.name(),
                        "strike": s.font.strike,
                        "color": hex(s.font.color.as_ref().and_then(|c| c.resolve(theme))),
                        "fill": hex(s.fill.paint(theme)),
                        "hAlign": s.align.horizontal,
                        "vAlign": s.align.vertical,
                        "wrap": s.align.wrap,
                        "indent": s.align.indent,
                        "rotation": s.align.rotation,
                        "numFmt": s.num_fmt,
                        "border": {
                            "l": edge(&s.border.left),
                            "r": edge(&s.border.right),
                            "t": edge(&s.border.top),
                            "b": edge(&s.border.bottom),
                        },
                    })
                })
                .collect();
            to_json(&Json::Array(list))
        })
    })
}

/// One cell for the formula bar: `{"edit": "=SUM(A1:A3)", "text": shown,
/// "kind", "style", "formula": bool, "array": bool, "numFmt"}`.
#[no_mangle]
pub extern "C" fn sheet_cell_json(handle: i64, sheet: i64, row: i64, col: i64) -> *mut c_char {
    call_str(|| {
        BOOKS.with(handle, |wb| {
            let s = check_sheet(wb, sheet)?;
            let at = CellRef::new(row.max(0) as u32, col.max(0) as u32);
            let cell = wb.sheets[s].cell(at).cloned().unwrap_or_default();
            let shown = sheet_core::display::shown(wb, &cell);
            let array = cell.formula.as_ref().is_some_and(|f| f.array.is_some_and(|a| a.cell_count() > 1)) || cell.array_member().is_some();
            to_json(&json!({
                "edit": sheet_core::display::edit_text(wb, &cell),
                "text": shown.text,
                "kind": shown.kind,
                "style": cell.style,
                "formula": cell.formula.is_some(),
                "array": array,
                "numFmt": wb.styles.get(cell.style).num_fmt,
            }))
        })
    })
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct BorderJson {
    kind: String,
    #[serde(default)]
    style: Option<String>,
    #[serde(default)]
    color: Option<String>,
}

#[derive(Deserialize, Default)]
#[serde(rename_all = "camelCase", default)]
struct PatchJson {
    bold: Option<bool>,
    italic: Option<bool>,
    underline: Option<String>,
    strike: Option<bool>,
    font: Option<String>,
    size: Option<f64>,
    /// `#rrggbb`, or `""` for automatic.
    color: Option<String>,
    /// `#rrggbb`, or `""` for none.
    fill: Option<String>,
    h_align: Option<String>,
    v_align: Option<String>,
    wrap: Option<bool>,
    indent: Option<u8>,
    num_fmt: Option<String>,
    border: Option<BorderJson>,
    decimals: Option<i32>,
}

fn color_opt(v: &str) -> Option<ColorSpec> {
    if v.is_empty() {
        None
    } else {
        ColorSpec::parse_hex(v)
    }
}

impl PatchJson {
    fn into_patch(self) -> StylePatch {
        StylePatch {
            bold: self.bold,
            italic: self.italic,
            underline: self.underline.map(|u| if u == "none" { Underline::None } else { Underline::parse(&u) }),
            strike: self.strike,
            font_name: self.font,
            font_size: self.size.map(|s| (s * 100.0).round() as u32),
            font_color: self.color.map(|c| color_opt(&c)),
            fill: self.fill.map(|c| color_opt(&c)),
            h_align: self.h_align,
            v_align: self.v_align,
            wrap: self.wrap,
            indent: self.indent,
            num_fmt: self.num_fmt,
            border: self.border.map(|b| {
                let kind = match b.kind.as_str() {
                    "outline" => BorderKind::Outline,
                    "inside" => BorderKind::Inside,
                    "none" => BorderKind::None,
                    "top" => BorderKind::Top,
                    "bottom" => BorderKind::Bottom,
                    "left" => BorderKind::Left,
                    "right" => BorderKind::Right,
                    _ => BorderKind::All,
                };
                (kind, b.style.unwrap_or_else(|| "thin".into()), b.color.as_deref().and_then(color_opt))
            }),
            decimals_delta: self.decimals,
        }
    }
}

#[derive(Deserialize)]
struct SortKeyJson {
    col: u32,
    #[serde(default = "yes")]
    asc: bool,
}

fn yes() -> bool {
    true
}

type RangeJson = [u32; 4];

fn range(r: RangeJson) -> Range {
    Range::new(CellRef::new(r[0], r[1]), CellRef::new(r[2], r[3]))
}

#[derive(Deserialize)]
#[serde(tag = "op", rename_all = "camelCase", rename_all_fields = "camelCase")]
enum OpJson {
    Set { sheet: usize, row: u32, col: u32, input: String },
    SetBlock { sheet: usize, row: u32, col: u32, rows: Vec<Vec<String>> },
    Clear { sheet: usize, ranges: Vec<RangeJson>, #[serde(default)] what: Option<String> },
    Format { sheet: usize, ranges: Vec<RangeJson>, patch: PatchJson },
    InsertRows { sheet: usize, at: u32, count: u32 },
    DeleteRows { sheet: usize, at: u32, count: u32 },
    InsertCols { sheet: usize, at: u32, count: u32 },
    DeleteCols { sheet: usize, at: u32, count: u32 },
    ColWidth { sheet: usize, first: u32, last: u32, width: Option<f64> },
    RowHeight { sheet: usize, first: u32, last: u32, height: Option<f64> },
    HideRows { sheet: usize, first: u32, last: u32, hidden: bool },
    HideCols { sheet: usize, first: u32, last: u32, hidden: bool },
    Merge { sheet: usize, range: RangeJson },
    Unmerge { sheet: usize, range: RangeJson },
    Freeze { sheet: usize, row: u32, col: u32 },
    AddSheet { name: Option<String>, at: Option<usize> },
    RenameSheet { sheet: usize, name: String },
    DeleteSheet { sheet: usize },
    MoveSheet { sheet: usize, to: usize },
    DuplicateSheet { sheet: usize },
    HideSheet { sheet: usize, hidden: bool },
    SetActive { sheet: usize },
    Fill { sheet: usize, range: RangeJson, #[serde(default = "yes")] down: bool },
    Paste {
        from_sheet: usize,
        from: RangeJson,
        to_sheet: usize,
        row: u32,
        col: u32,
        #[serde(default)]
        what: Option<String>,
        #[serde(default)]
        cut: bool,
    },
    Sort { sheet: usize, range: RangeJson, keys: Vec<SortKeyJson>, #[serde(default)] header: bool },
    Replace {
        sheet: Option<usize>,
        find: String,
        replace: String,
        #[serde(default)]
        match_case: bool,
        #[serde(default)]
        whole: bool,
    },
    DefineName { name: String, sheet: Option<usize>, refers_to: String },
    DeleteName { name: String, sheet: Option<usize> },
}

impl OpJson {
    fn into_op(self) -> Op {
        match self {
            OpJson::Set { sheet, row, col, input } => Op::SetCell { sheet, at: CellRef::new(row, col), input },
            OpJson::SetBlock { sheet, row, col, rows } => Op::SetCells { sheet, at: CellRef::new(row, col), rows },
            OpJson::Clear { sheet, ranges, what } => Op::Clear {
                sheet,
                ranges: ranges.into_iter().map(range).collect(),
                what: match what.as_deref() {
                    Some("contents") => ClearWhat::Contents,
                    Some("formats") => ClearWhat::Formats,
                    _ => ClearWhat::All,
                },
            },
            OpJson::Format { sheet, ranges, patch } => Op::Format {
                sheet,
                ranges: ranges.into_iter().map(range).collect(),
                patch: patch.into_patch(),
            },
            OpJson::InsertRows { sheet, at, count } => Op::InsertRows { sheet, at, count },
            OpJson::DeleteRows { sheet, at, count } => Op::DeleteRows { sheet, at, count },
            OpJson::InsertCols { sheet, at, count } => Op::InsertCols { sheet, at, count },
            OpJson::DeleteCols { sheet, at, count } => Op::DeleteCols { sheet, at, count },
            OpJson::ColWidth { sheet, first, last, width } => Op::ColWidth { sheet, first, last, width },
            OpJson::RowHeight { sheet, first, last, height } => Op::RowHeight { sheet, first, last, height },
            OpJson::HideRows { sheet, first, last, hidden } => Op::HideRows { sheet, first, last, hidden },
            OpJson::HideCols { sheet, first, last, hidden } => Op::HideCols { sheet, first, last, hidden },
            OpJson::Merge { sheet, range: r } => Op::Merge { sheet, range: range(r) },
            OpJson::Unmerge { sheet, range: r } => Op::Unmerge { sheet, range: range(r) },
            OpJson::Freeze { sheet, row, col } => Op::Freeze {
                sheet,
                at: (row > 0 || col > 0).then(|| CellRef::new(row, col)),
            },
            OpJson::AddSheet { name, at } => Op::AddSheet { name, at },
            OpJson::RenameSheet { sheet, name } => Op::RenameSheet { sheet, name },
            OpJson::DeleteSheet { sheet } => Op::DeleteSheet { sheet },
            OpJson::MoveSheet { sheet, to } => Op::MoveSheet { sheet, to },
            OpJson::DuplicateSheet { sheet } => Op::DuplicateSheet { sheet },
            OpJson::HideSheet { sheet, hidden } => Op::HideSheet { sheet, hidden },
            OpJson::SetActive { sheet } => Op::SetActive { sheet },
            OpJson::Fill { sheet, range: r, down } => Op::Fill { sheet, range: range(r), down },
            OpJson::Paste { from_sheet, from, to_sheet, row, col, what, cut } => Op::Paste {
                from_sheet,
                from: range(from),
                to_sheet,
                to: CellRef::new(row, col),
                what: match what.as_deref() {
                    Some("values") => PasteWhat::Values,
                    Some("formats") => PasteWhat::Formats,
                    Some("formulas") => PasteWhat::Formulas,
                    _ => PasteWhat::All,
                },
                cut,
            },
            OpJson::Sort { sheet, range: r, keys, header } => Op::Sort {
                sheet,
                range: range(r),
                keys: keys.into_iter().map(|k| SortKey { col: k.col, ascending: k.asc }).collect(),
                header,
            },
            OpJson::Replace { sheet, find, replace, match_case, whole } => Op::Replace { sheet, find, replace, match_case, whole },
            OpJson::DefineName { name, sheet, refers_to } => Op::DefineName { name, sheet, refers_to },
            OpJson::DeleteName { name, sheet } => Op::DeleteName { name, sheet },
        }
    }
}

/// How many changed cells an edit reports; beyond that the screen should
/// refresh everything.
const CHANGED_LIMIT: usize = 5000;

fn result_json(r: &EditResult, wb: &Workbook) -> Json {
    let many = r.changed.len() > CHANGED_LIMIT;
    json!({
        "changed": if many { Vec::new() } else { r.changed.iter().map(|(s, c)| json!([s, c.row, c.col])).collect::<Vec<_>>() },
        "all": many || r.structure,
        "structure": r.structure,
        "circular": r.circular.iter().map(|(s, c)| json!([s, c.row, c.col])).collect::<Vec<_>>(),
        "canUndo": wb.can_undo(),
        "canRedo": wb.can_redo(),
    })
}

/// Apply an edit, given as JSON: `{"op": "set", "sheet": 0, "row": 0, "col":
/// 0, "input": "=SUM(A2:A9)"}`. Operations: `set`, `setBlock`, `clear`,
/// `format`, `insertRows`, `deleteRows`, `insertCols`, `deleteCols`,
/// `colWidth`, `rowHeight`, `hideRows`, `hideCols`, `merge`, `unmerge`,
/// `freeze`, `addSheet`, `renameSheet`, `deleteSheet`, `moveSheet`,
/// `duplicateSheet`, `hideSheet`, `setActive`, `fill`, `paste`, `sort`,
/// `replace`, `defineName`, `deleteName`. Returns `{"changed": [[sheet, row,
/// col]], "all": bool, "structure": bool, "circular": [...], "canUndo",
/// "canRedo"}`. A refused edit (a bad formula, part of an array) fails
/// with `INVALID_ARGUMENT` and a message for the person.
///
/// # Safety
/// `op_json` must be a valid NUL-terminated UTF-8 string.
#[no_mangle]
pub unsafe extern "C" fn sheet_apply(handle: i64, op_json: *const c_char) -> *mut c_char {
    call_str(|| {
        let op: OpJson = serde_json::from_str(str_arg(op_json, "operation")?).map_err(json_err)?;
        BOOKS.with(handle, |wb| {
            let r = wb.apply(op.into_op()).map_err(edit_error)?;
            to_json(&result_json(&r, wb))
        })
    })
}

/// Undo the last edit. Returns the same shape as [`sheet_apply`], or
/// `null` when there is nothing to undo.
#[no_mangle]
pub extern "C" fn sheet_undo(handle: i64) -> *mut c_char {
    call_str(|| BOOKS.with(handle, |wb| match wb.undo() {
        Some(r) => to_json(&result_json(&r, wb)),
        None => Ok("null".into()),
    }))
}

/// Redo the last undone edit.
#[no_mangle]
pub extern "C" fn sheet_redo(handle: i64) -> *mut c_char {
    call_str(|| BOOKS.with(handle, |wb| match wb.redo() {
        Some(r) => to_json(&result_json(&r, wb)),
        None => Ok("null".into()),
    }))
}

fn format_arg(name: &str) -> Result<Format, FfiError> {
    Format::from_name(name).ok_or_else(|| FfiError::unsupported(format!("can't save as '{name}'")))
}

fn save_warnings(wb: &Workbook, format: Format) -> Vec<String> {
    let mut w = Vec::new();
    if matches!(format, Format::Csv | Format::Tsv) {
        if wb.sheets.len() > 1 {
            w.push(format!("Only the sheet \"{}\" was saved; CSV files hold one sheet.", wb.sheets[wb.active].name));
        }
        let formulas = wb.sheets[wb.active].cells.values().flat_map(|r| r.values()).any(|c| c.formula.is_some());
        if formulas {
            w.push("Formulas were saved as their values; CSV files can't hold formulas.".into());
        }
    }
    w
}

/// Save to `path` in `format` (`xlsx`, `xlsm`, `ods`, `csv`, `tsv`)
/// atomically. Later saves build on the written file. Returns `{"warnings":
/// [...]}` (for example, what a CSV file cannot hold).
///
/// # Safety
/// String arguments must be valid NUL-terminated UTF-8.
#[no_mangle]
pub unsafe extern "C" fn sheet_save(handle: i64, path: *const c_char, format: *const c_char) -> *mut c_char {
    call_str(|| {
        let path = str_arg(path, "path")?;
        let format = format_arg(str_arg(format, "format")?)?;
        BOOKS.with(handle, |wb| {
            let warnings = save_warnings(wb, format);
            let bytes = wb.to_bytes(format).map_err(sheet_error)?;
            write_file_atomically(path, &bytes)?;
            wb.saved(format, bytes).map_err(sheet_error)?;
            to_json(&json!({ "warnings": warnings }))
        })
    })
}

/// Write a copy to `path` in `format` without changing what the workbook
/// is saved as (sharing, exporting CSV). Returns `{"warnings": [...]}`.
///
/// # Safety
/// String arguments must be valid NUL-terminated UTF-8.
#[no_mangle]
pub unsafe extern "C" fn sheet_write_copy(handle: i64, path: *const c_char, format: *const c_char) -> *mut c_char {
    call_str(|| {
        let path = str_arg(path, "path")?;
        let format = format_arg(str_arg(format, "format")?)?;
        BOOKS.with(handle, |wb| {
            let warnings = save_warnings(wb, format);
            let bytes = wb.to_bytes(format).map_err(sheet_error)?;
            write_file_atomically(path, &bytes)?;
            to_json(&json!({ "warnings": warnings }))
        })
    })
}

#[derive(Deserialize, Default)]
#[serde(rename_all = "camelCase", default)]
struct PdfOptionsJson {
    font_dirs: Vec<String>,
    /// Sheet indexes; empty for every visible sheet.
    sheets: Vec<usize>,
    gridlines: Option<bool>,
    title: Option<String>,
    /// Page width and height in points (portrait).
    page: Option<(f32, f32)>,
}

/// Print the workbook (or some sheets) to a PDF at `path`. `options_json`:
/// `{"fontDirs": [...], "sheets": [0], "gridlines": true, "title": "...",
/// "page": [595, 842]}`. Returns `{"pages"}`.
///
/// # Safety
/// String arguments must be NULL or valid NUL-terminated UTF-8.
#[no_mangle]
pub unsafe extern "C" fn sheet_export_pdf(handle: i64, path: *const c_char, options_json: *const c_char) -> *mut c_char {
    call_str(|| {
        let path = str_arg(path, "path")?;
        let o: PdfOptionsJson = match opt_str_arg(options_json, "options")? {
            Some(j) => serde_json::from_str(j).map_err(|e| FfiError::invalid(format!("bad options: {e}")))?,
            None => PdfOptionsJson::default(),
        };
        let book = doc_font::FontBook::shared();
        let mut book = book.lock().unwrap_or_else(|e| e.into_inner());
        for d in &o.font_dirs {
            book.add_dir(std::path::Path::new(d));
        }
        BOOKS.with(handle, |wb| {
            let mut opts = sheet_core::pdf::PdfOptions { sheets: o.sheets.clone(), title: o.title.clone(), ..Default::default() };
            if let Some(g) = o.gridlines {
                opts.gridlines = g;
            }
            if let Some(p) = o.page {
                opts.page = p;
            }
            let bytes = sheet_core::pdf::export(wb, &mut book, &opts);
            let pages = String::from_utf8_lossy(&bytes).matches("/Type /Page ").count();
            write_file_atomically(path, &bytes)?;
            to_json(&json!({"pages": pages}))
        })
    })
}

/// Find cells whose shown text contains `query`. `flags`: 1 match case,
/// 2 whole cell, 4 look in formulas. `sheet` -1 searches every sheet.
/// Returns `[[sheet, row, col]]`.
///
/// # Safety
/// `query` must be a valid NUL-terminated UTF-8 string.
#[no_mangle]
pub unsafe extern "C" fn sheet_find_json(handle: i64, query: *const c_char, flags: i32, sheet: i64, limit: i64) -> *mut c_char {
    call_str(|| {
        let query = str_arg(query, "query")?;
        BOOKS.with(handle, |wb| {
            let sheet = if sheet < 0 { None } else { Some(check_sheet(wb, sheet)?) };
            let found = wb.find(query, sheet, flags & 1 != 0, flags & 2 != 0, flags & 4 != 0, limit.clamp(1, 100_000) as usize);
            to_json(&Json::Array(found.iter().map(|f| json!([f.sheet, f.cell.row, f.cell.col])).collect()))
        })
    })
}

fn value_json(v: &sheet_core::Value) -> Json {
    match v {
        sheet_core::Value::Empty => Json::Null,
        sheet_core::Value::Number(n) => json!(n),
        sheet_core::Value::Text(t) => json!(t.as_ref()),
        sheet_core::Value::Bool(b) => json!(b),
        sheet_core::Value::Error(e) => json!({"error": e.as_str()}),
    }
}

/// Evaluate a formula (without `=`) as if it were in a cell, without
/// changing anything: the status bar's sum and count of a selection.
/// Returns `{"value": ..., "text": shown}`.
///
/// # Safety
/// `formula` must be a valid NUL-terminated UTF-8 string.
#[no_mangle]
pub unsafe extern "C" fn sheet_evaluate_json(handle: i64, sheet: i64, row: i64, col: i64, formula: *const c_char) -> *mut c_char {
    call_str(|| {
        let formula = str_arg(formula, "formula")?;
        BOOKS.with(handle, |wb| {
            let s = check_sheet(wb, sheet)?;
            let v = wb.evaluate_in(s, CellRef::new(row.max(0) as u32, col.max(0) as u32), formula);
            let text = sheet_core::numfmt::format_value(&v, "General", &wb.format_options()).text;
            to_json(&json!({"value": value_json(&v), "text": text}))
        })
    })
}

/// Recalculate every formula. Returns the shape of [`sheet_apply`].
#[no_mangle]
pub extern "C" fn sheet_recalc(handle: i64) -> *mut c_char {
    call_str(|| {
        BOOKS.with(handle, |wb| {
            let changed = wb.recalc_all();
            let r = EditResult {
                changed,
                structure: false,
                circular: wb.circular_references().to_vec(),
            };
            to_json(&result_json(&r, wb))
        })
    })
}

/// Change the locale options of an open workbook.
///
/// # Safety
/// `options_json` must be a valid NUL-terminated UTF-8 string.
#[no_mangle]
pub unsafe extern "C" fn sheet_set_options(handle: i64, options_json: *const c_char) -> i64 {
    call_i64(|| {
        let opts = options(Some(str_arg(options_json, "options")?))?;
        BOOKS.with(handle, |wb| {
            if opts.date_order.is_some() {
                wb.date_order = date_order(&opts);
            }
            if let Some(m) = opts.utc_offset_minutes {
                wb.set_utc_offset(m);
            }
            Ok(0)
        })
    })
}

/// A number formatted with a format code, for format pickers:
/// `{"text": "1,234.50", "color": "#ff0000"|null}`.
///
/// # Safety
/// `code` must be a valid NUL-terminated UTF-8 string.
#[no_mangle]
pub unsafe extern "C" fn sheet_format_preview(value: f64, code: *const c_char, date_order: i32) -> *mut c_char {
    call_str(|| {
        let code = str_arg(code, "format code")?;
        let options = FormatOptions {
            date_order: match date_order {
                1 => DateOrder::Mdy,
                2 => DateOrder::Ymd,
                _ => DateOrder::Dmy,
            },
            date1904: false,
        };
        let f = sheet_core::numfmt::format_value(&sheet_core::Value::Number(value), code, &options);
        to_json(&json!({"text": f.text, "color": hex(f.color)}))
    })
}

/// Every worksheet function the engine computes, for autocomplete.
#[no_mangle]
pub extern "C" fn sheet_function_names_json() -> *mut c_char {
    call_str(|| to_json(&json!(sheet_core::formula::functions::names())))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::ffi::{CStr, CString};

    fn take(ptr: *mut c_char) -> String {
        assert!(!ptr.is_null(), "{}", doc_ffi::last_error());
        let s = unsafe { CStr::from_ptr(ptr) }.to_str().unwrap().to_owned();
        unsafe { sheet_free_string(ptr) };
        s
    }

    fn c(text: &str) -> CString {
        CString::new(text).unwrap()
    }

    fn apply(h: i64, op: Json) -> Json {
        let s = c(&op.to_string());
        serde_json::from_str(&take(unsafe { sheet_apply(h, s.as_ptr()) })).unwrap()
    }

    #[test]
    fn edit_save_and_reopen() {
        let h = unsafe { sheet_new(std::ptr::null()) };
        assert!(h > 0);
        apply(h, json!({"op": "set", "sheet": 0, "row": 0, "col": 0, "input": "12"}));
        let r = apply(h, json!({"op": "set", "sheet": 0, "row": 1, "col": 0, "input": "=A1*2"}));
        assert_eq!(r["changed"][0], json!([0, 1, 0]));
        apply(h, json!({"op": "format", "sheet": 0, "ranges": [[0, 0, 1, 0]], "patch": {"bold": true, "fill": "#ffff00"}}));
        let cells: Json = serde_json::from_str(&take(sheet_cells_json(h, 0, 0, 0, 10, 10))).unwrap();
        assert_eq!(cells["cells"][1][2], json!("24"));
        let style = cells["cells"][1][4].as_u64().unwrap();
        let ids = c(&format!("[{style}]"));
        let styles: Json = serde_json::from_str(&take(unsafe { sheet_styles_json(h, ids.as_ptr()) })).unwrap();
        assert_eq!(styles[0]["bold"], json!(true));
        assert_eq!(styles[0]["fill"], json!("#ffff00"));
        let cell: Json = serde_json::from_str(&take(sheet_cell_json(h, 0, 1, 0))).unwrap();
        assert_eq!(cell["edit"], json!("=A1*2"));

        // A bad formula is refused with a readable message.
        let bad = c(&json!({"op": "set", "sheet": 0, "row": 2, "col": 0, "input": "=SUM("}).to_string());
        assert!(unsafe { sheet_apply(h, bad.as_ptr()) }.is_null());
        assert!(doc_ffi::last_error().starts_with("INVALID_ARGUMENT: There's a problem with this formula"));

        let dir = std::env::temp_dir().join(format!("sheet_ffi_{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("book.xlsx");
        let p = c(path.to_str().unwrap());
        let fmt = c("xlsx");
        let saved: Json = serde_json::from_str(&take(unsafe { sheet_save(h, p.as_ptr(), fmt.as_ptr()) })).unwrap();
        assert_eq!(saved["warnings"], json!([]));
        let info: Json = serde_json::from_str(&take(sheet_info_json(h))).unwrap();
        assert_eq!(info["modified"], json!(false));
        assert_eq!(sheet_close(h), 0);

        let h2 = unsafe { sheet_open(p.as_ptr(), std::ptr::null(), std::ptr::null()) };
        assert!(h2 > 0, "{}", doc_ffi::last_error());
        let cells: Json = serde_json::from_str(&take(sheet_cells_json(h2, 0, 0, 0, 10, 10))).unwrap();
        assert_eq!(cells["cells"][1][2], json!("24"));
        let csv = dir.join("book.csv");
        let pc = c(csv.to_str().unwrap());
        let fc = c("csv");
        let out: Json = serde_json::from_str(&take(unsafe { sheet_write_copy(h2, pc.as_ptr(), fc.as_ptr()) })).unwrap();
        assert_eq!(out["warnings"].as_array().unwrap().len(), 1);
        assert_eq!(std::fs::read_to_string(&csv).unwrap(), "12\r\n24\r\n");
        let q = c("24");
        let found: Json = serde_json::from_str(&take(unsafe { sheet_find_json(h2, q.as_ptr(), 0, -1, 10) })).unwrap();
        assert_eq!(found, json!([[0, 1, 0]]));
        let f = c("SUM(A1:A2)");
        let v: Json = serde_json::from_str(&take(unsafe { sheet_evaluate_json(h2, 0, 5, 5, f.as_ptr()) })).unwrap();
        assert_eq!(v["value"], json!(36.0));
        assert_eq!(sheet_close(h2), 0);
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn previews_and_names() {
        let code = c("#,##0.00");
        let p: Json = serde_json::from_str(&take(unsafe { sheet_format_preview(1234.5, code.as_ptr(), 0) })).unwrap();
        assert_eq!(p["text"], json!("1,234.50"));
        let names: Json = serde_json::from_str(&take(sheet_function_names_json())).unwrap();
        assert!(names.as_array().unwrap().iter().any(|n| n == "VLOOKUP"));
    }
}
