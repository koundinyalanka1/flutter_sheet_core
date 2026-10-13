//! VLOOKUP, XLOOKUP, INDEX/MATCH and the other lookup and reference
//! functions.

use std::cmp::Ordering;

use super::super::ast::Expr;
use super::super::eval::{Array, Eval, Evaluator, SheetArea};
use super::criteria::{has_wildcards, wildcard_match};
use super::{arity, omitted, tri};
use crate::refs::{col_to_letters, quote_sheet_name, CellRef, Range, MAX_COLS, MAX_ROWS};
use crate::value::{compare, CellError, Value};

pub fn call(ev: &mut Evaluator, name: &str, args: &[Expr]) -> Option<Eval> {
    Some(match name {
        "VLOOKUP" => vhlookup(ev, args, true),
        "HLOOKUP" => vhlookup(ev, args, false),
        "LOOKUP" => lookup(ev, args),
        "MATCH" => match_fn(ev, args),
        "XMATCH" => xmatch(ev, args),
        "XLOOKUP" => xlookup(ev, args),
        "INDEX" => index(ev, args),
        "CHOOSE" => choose(ev, args),
        "ROW" => row_col(ev, args, true),
        "COLUMN" => row_col(ev, args, false),
        "ROWS" => rows_cols(ev, args, true),
        "COLUMNS" => rows_cols(ev, args, false),
        "OFFSET" => offset(ev, args),
        "INDIRECT" => indirect(ev, args),
        "ADDRESS" => address(ev, args),
        "TRANSPOSE" => transpose(ev, args),
        "HYPERLINK" => {
            if let Some(e) = arity(args, 1, 2) {
                return Some(e);
            }
            if omitted(args, 1) {
                Eval::Value(ev.arg(args, 0))
            } else {
                Eval::Value(ev.arg(args, 1))
            }
        }
        "AREAS" => {
            if let Some(e) = arity(args, 1, 1) {
                return Some(e);
            }
            match ev.reference_arg(args, 0) {
                Some(a) => Eval::number(a.len() as f64),
                None => Eval::error(CellError::Value),
            }
        }
        _ => return None,
    })
}

/// A table to look things up in: a (clipped) area of a sheet, read lazily
/// one row or column at a time, or an array.
enum Table {
    Area(SheetArea),
    Array(Array),
}

impl Table {
    fn from_eval(ev: &mut Evaluator, v: Eval) -> Result<Table, CellError> {
        match v {
            Eval::Ref(areas) => {
                if areas.len() != 1 {
                    return Err(CellError::Ref);
                }
                let area = areas[0];
                // Clip whole columns and rows to the used part of the sheet.
                let range = if area.range.cell_count() > 1_000_000 {
                    match ev.clip(area) {
                        Some(r) => Range::new(area.range.start, r.end.max(area.range.start)),
                        None => Range::cell(area.range.start),
                    }
                } else {
                    area.range
                };
                Ok(Table::Area(SheetArea { sheet: area.sheet, range }))
            }
            Eval::Array(a) => Ok(Table::Array(a)),
            Eval::Value(Value::Error(e)) => Err(e),
            Eval::Value(v) => Ok(Table::Array(Array::new(1, 1, vec![v]))),
        }
    }

    fn rows(&self) -> usize {
        match self {
            Table::Area(a) => a.range.rows() as usize,
            Table::Array(a) => a.rows,
        }
    }

    fn cols(&self) -> usize {
        match self {
            Table::Area(a) => a.range.cols() as usize,
            Table::Array(a) => a.cols,
        }
    }

    /// One column (or row) as values.
    fn line(&self, ev: &mut Evaluator, index: usize, column: bool) -> Vec<Value> {
        match self {
            Table::Array(a) => {
                if column {
                    (0..a.rows).map(|r| a.get(r, index).clone()).collect()
                } else {
                    (0..a.cols).map(|c| a.get(index, c).clone()).collect()
                }
            }
            Table::Area(area) => {
                let r = area.range;
                let (range, len) = if column {
                    let col = r.start.col + index as u32;
                    (Range::new(CellRef::new(r.start.row, col), CellRef::new(r.end.row, col)), r.rows())
                } else {
                    let row = r.start.row + index as u32;
                    (Range::new(CellRef::new(row, r.start.col), CellRef::new(row, r.end.col)), r.cols())
                };
                let mut out = vec![Value::Empty; len as usize];
                for (cell, v) in ev.grid.cells_in(area.sheet, range) {
                    let i = if column { cell.row - r.start.row } else { cell.col - r.start.col };
                    out[i as usize] = v;
                }
                out
            }
        }
    }

    fn get(&self, ev: &mut Evaluator, row: usize, col: usize) -> Value {
        match self {
            Table::Array(a) => a.get(row, col).clone(),
            Table::Area(area) => {
                let cell = CellRef::new(area.range.start.row + row as u32, area.range.start.col + col as u32);
                ev.grid.value(area.sheet, cell)
            }
        }
    }

    /// A sub-rectangle, as a reference when the table is one.
    fn slice(&self, row: usize, col: usize, rows: usize, cols: usize) -> Eval {
        match self {
            Table::Area(area) => {
                let start = CellRef::new(area.range.start.row + row as u32, area.range.start.col + col as u32);
                let end = CellRef::new(start.row + rows as u32 - 1, start.col + cols as u32 - 1);
                Eval::Ref(vec![SheetArea {
                    sheet: area.sheet,
                    range: Range::new(start, end),
                }])
            }
            Table::Array(a) => {
                let mut data = Vec::with_capacity(rows * cols);
                for r in row..row + rows {
                    for c in col..col + cols {
                        data.push(a.get(r, c).clone());
                    }
                }
                if rows * cols == 1 {
                    Eval::Value(data.pop().unwrap_or_default())
                } else {
                    Eval::Array(Array::new(rows, cols, data))
                }
            }
        }
    }
}

fn same_kind(a: &Value, b: &Value) -> bool {
    matches!(
        (a, b),
        (Value::Number(_), Value::Number(_)) | (Value::Text(_), Value::Text(_)) | (Value::Bool(_), Value::Bool(_))
    )
}

/// The first position holding the value (case-insensitive; with wildcards
/// for text when allowed), searching forwards or backwards.
fn find_exact(hay: &[Value], needle: &Value, wildcards: bool, reverse: bool) -> Option<usize> {
    let wild = wildcards && matches!(needle, Value::Text(t) if has_wildcards(t));
    let hit = |v: &Value| {
        if wild {
            match (v, needle) {
                (Value::Text(t), Value::Text(p)) => wildcard_match(p, t),
                _ => false,
            }
        } else {
            same_kind(v, needle) && compare(v, needle) == Ordering::Equal
        }
    };
    if reverse {
        hay.iter().rposition(hit)
    } else {
        hay.iter().position(hit)
    }
}

/// Binary search as Excel does it for sorted data: the last position whose
/// value is <= the needle (ascending) or >= it (descending), looking only
/// at values of the needle's kind.
fn find_sorted(hay: &[Value], needle: &Value, descending: bool) -> Option<usize> {
    if hay.is_empty() {
        return None;
    }
    let (mut lo, mut hi) = (0i64, hay.len() as i64 - 1);
    let mut found = None;
    while lo <= hi {
        let mid = (lo + hi) / 2;
        let mut m = mid;
        while m >= lo && !same_kind(&hay[m as usize], needle) {
            m -= 1;
        }
        if m < lo {
            lo = mid + 1;
            continue;
        }
        let ord = compare(&hay[m as usize], needle);
        let ok = if descending { ord != Ordering::Less } else { ord != Ordering::Greater };
        if ok {
            found = Some(m as usize);
            lo = mid + 1;
        } else {
            hi = m - 1;
        }
    }
    found
}

fn lookup_value(ev: &mut Evaluator, args: &[Expr]) -> Result<Value, CellError> {
    match ev.arg(args, 0) {
        Value::Error(e) => Err(e),
        v => Ok(v),
    }
}

fn vhlookup(ev: &mut Evaluator, args: &[Expr], vertical: bool) -> Eval {
    if let Some(e) = arity(args, 3, 4) {
        return e;
    }
    let needle = tri!(lookup_value(ev, args));
    let table_eval = ev.eval(&args[1]);
    let table = tri!(Table::from_eval(ev, table_eval));
    let index = tri!(ev.number(args, 2)).trunc();
    let approximate = tri!(ev.bool_or(args, 3, true));
    if index < 1.0 {
        return Eval::error(CellError::Value);
    }
    let width = if vertical { table.cols() } else { table.rows() };
    if index as usize > width {
        return Eval::error(CellError::Ref);
    }
    let keys = table.line(ev, 0, vertical);
    let pos = if approximate {
        find_sorted(&keys, &needle, false)
    } else {
        find_exact(&keys, &needle, true, false)
    };
    match pos {
        Some(p) => {
            let i = index as usize - 1;
            let v = if vertical { table.get(ev, p, i) } else { table.get(ev, i, p) };
            Eval::Value(v)
        }
        None => Eval::error(CellError::NA),
    }
}

fn lookup(ev: &mut Evaluator, args: &[Expr]) -> Eval {
    if let Some(e) = arity(args, 2, 3) {
        return e;
    }
    let needle = tri!(lookup_value(ev, args));
    let v = ev.eval(&args[1]);
    let table = tri!(Table::from_eval(ev, v));
    if omitted(args, 2) {
        // Array form: search the first row or column, return from the last.
        let by_row = table.cols() > table.rows();
        let keys = table.line(ev, 0, !by_row);
        return match find_sorted(&keys, &needle, false) {
            Some(p) => Eval::Value(if by_row {
                table.get(ev, table.rows() - 1, p)
            } else {
                table.get(ev, p, table.cols() - 1)
            }),
            None => Eval::error(CellError::NA),
        };
    }
    let keys = vector(ev, &table);
    let Some(p) = find_sorted(&keys, &needle, false) else {
        return Eval::error(CellError::NA);
    };
    let rv = ev.eval(&args[2]);
    let result = tri!(Table::from_eval(ev, rv));
    if result.rows() == 1 {
        if p >= result.cols() {
            return match result {
                Table::Area(a) => {
                    let cell = CellRef::new(a.range.start.row, a.range.start.col + p as u32);
                    Eval::Value(ev.grid.value(a.sheet, cell))
                }
                Table::Array(_) => Eval::error(CellError::NA),
            };
        }
        Eval::Value(result.get(ev, 0, p))
    } else {
        if p >= result.rows() {
            return match result {
                Table::Area(a) => {
                    let cell = CellRef::new(a.range.start.row + p as u32, a.range.start.col);
                    Eval::Value(ev.grid.value(a.sheet, cell))
                }
                Table::Array(_) => Eval::error(CellError::NA),
            };
        }
        Eval::Value(result.get(ev, p, 0))
    }
}

/// A one-row or one-column table as a list.
fn vector(ev: &mut Evaluator, table: &Table) -> Vec<Value> {
    if table.cols() == 1 {
        table.line(ev, 0, true)
    } else {
        table.line(ev, 0, false)
    }
}

fn match_fn(ev: &mut Evaluator, args: &[Expr]) -> Eval {
    if let Some(e) = arity(args, 2, 3) {
        return e;
    }
    let needle = tri!(lookup_value(ev, args));
    let v = ev.eval(&args[1]);
    let table = tri!(Table::from_eval(ev, v));
    if table.rows() > 1 && table.cols() > 1 {
        return Eval::error(CellError::NA);
    }
    let kind = if omitted(args, 2) { 1.0 } else { tri!(ev.number(args, 2)).trunc() };
    let keys = vector(ev, &table);
    let pos = if kind == 0.0 {
        find_exact(&keys, &needle, true, false)
    } else {
        find_sorted(&keys, &needle, kind < 0.0)
    };
    match pos {
        Some(p) => Eval::number(p as f64 + 1.0),
        None => Eval::error(CellError::NA),
    }
}

/// XMATCH/XLOOKUP matching: the position of the needle in the keys.
fn xfind(keys: &[Value], needle: &Value, match_mode: i64, search_mode: i64) -> Result<Option<usize>, CellError> {
    if !matches!(match_mode, -1..=2) || !matches!(search_mode, -2 | -1 | 1 | 2) {
        return Err(CellError::Value);
    }
    if search_mode.abs() == 2 {
        // Binary search on sorted keys.
        let descending = search_mode == -2;
        let exact = find_sorted(keys, needle, descending)
            .filter(|&p| compare(&keys[p], needle) == Ordering::Equal && same_kind(&keys[p], needle));
        if exact.is_some() || match_mode == 0 || match_mode == 2 {
            return Ok(exact);
        }
        let below = find_sorted(keys, needle, descending);
        return Ok(match (match_mode, descending) {
            (-1, false) => below,
            (1, false) => below.map_or(Some(0), |p| (p + 1 < keys.len()).then_some(p + 1)),
            (1, true) => below,
            (-1, true) => below.map_or(Some(0), |p| (p + 1 < keys.len()).then_some(p + 1)),
            _ => None,
        });
    }
    let reverse = search_mode == -1;
    if let Some(p) = find_exact(keys, needle, match_mode == 2, reverse) {
        return Ok(Some(p));
    }
    if match_mode == 0 || match_mode == 2 {
        return Ok(None);
    }
    // The next smaller (-1) or larger (1) value.
    let mut best: Option<usize> = None;
    let order: Box<dyn Iterator<Item = usize>> = if reverse {
        Box::new((0..keys.len()).rev())
    } else {
        Box::new(0..keys.len())
    };
    for i in order {
        let k = &keys[i];
        if !same_kind(k, needle) {
            continue;
        }
        let ord = compare(k, needle);
        let candidate = if match_mode < 0 { ord == Ordering::Less } else { ord == Ordering::Greater };
        if !candidate {
            continue;
        }
        let better = match best {
            None => true,
            Some(b) => {
                let c = compare(k, &keys[b]);
                if match_mode < 0 {
                    c == Ordering::Greater
                } else {
                    c == Ordering::Less
                }
            }
        };
        if better {
            best = Some(i);
        }
    }
    Ok(best)
}

fn xmatch(ev: &mut Evaluator, args: &[Expr]) -> Eval {
    if let Some(e) = arity(args, 2, 4) {
        return e;
    }
    let needle = tri!(lookup_value(ev, args));
    let v = ev.eval(&args[1]);
    let table = tri!(Table::from_eval(ev, v));
    if table.rows() > 1 && table.cols() > 1 {
        return Eval::error(CellError::Value);
    }
    let mm = if omitted(args, 2) { 0 } else { tri!(ev.number(args, 2)) as i64 };
    let sm = if omitted(args, 3) { 1 } else { tri!(ev.number(args, 3)) as i64 };
    let keys = vector(ev, &table);
    match tri!(xfind(&keys, &needle, mm, sm)) {
        Some(p) => Eval::number(p as f64 + 1.0),
        None => Eval::error(CellError::NA),
    }
}

fn xlookup(ev: &mut Evaluator, args: &[Expr]) -> Eval {
    if let Some(e) = arity(args, 3, 6) {
        return e;
    }
    let needle = tri!(lookup_value(ev, args));
    let v = ev.eval(&args[1]);
    let keys_table = tri!(Table::from_eval(ev, v));
    let vertical = keys_table.cols() == 1;
    if !vertical && keys_table.rows() != 1 {
        return Eval::error(CellError::Value);
    }
    let rv = ev.eval(&args[2]);
    let result = tri!(Table::from_eval(ev, rv));
    let len = if vertical { keys_table.rows() } else { keys_table.cols() };
    let rlen = if vertical { result.rows() } else { result.cols() };
    if rlen != len {
        return Eval::error(CellError::Value);
    }
    let mm = if omitted(args, 4) { 0 } else { tri!(ev.number(args, 4)) as i64 };
    let sm = if omitted(args, 5) { 1 } else { tri!(ev.number(args, 5)) as i64 };
    let keys = vector(ev, &keys_table);
    match tri!(xfind(&keys, &needle, mm, sm)) {
        Some(p) => {
            if vertical {
                result.slice(p, 0, 1, result.cols())
            } else {
                result.slice(0, p, result.rows(), 1)
            }
        }
        None if !omitted(args, 3) => ev.eval(&args[3]),
        None => Eval::error(CellError::NA),
    }
}

fn index(ev: &mut Evaluator, args: &[Expr]) -> Eval {
    if let Some(e) = arity(args, 2, 4) {
        return e;
    }
    let base = ev.eval(&args[0]);
    let row = if omitted(args, 1) { 0.0 } else { tri!(ev.number(args, 1)).trunc() };
    let col_given = !omitted(args, 2);
    let col = if col_given { tri!(ev.number(args, 2)).trunc() } else { 0.0 };
    let area_num = if omitted(args, 3) { 1.0 } else { tri!(ev.number(args, 3)).trunc() };
    if row < 0.0 || col < 0.0 || area_num < 1.0 {
        return Eval::error(CellError::Value);
    }
    let table = match base {
        Eval::Ref(areas) => {
            let Some(area) = areas.get(area_num as usize - 1).copied() else {
                return Eval::error(CellError::Ref);
            };
            Table::Area(area)
        }
        Eval::Array(a) => Table::Array(a),
        Eval::Value(Value::Error(e)) => return Eval::error(e),
        Eval::Value(v) => Table::Array(Array::new(1, 1, vec![v])),
    };
    let (rows, cols) = (table.rows(), table.cols());
    let (mut r, mut c) = (row as usize, col as usize);
    // One index into a single row or column picks along it.
    if !col_given && rows == 1 && cols > 1 {
        c = r;
        r = 1;
        if c == 0 {
            return table.slice(0, 0, 1, cols);
        }
    }
    if r > rows || c > cols {
        return Eval::error(CellError::Ref);
    }
    match (r, c) {
        (0, 0) => table.slice(0, 0, rows, cols),
        (0, c) => table.slice(0, c - 1, rows, 1),
        (r, 0) if cols == 1 => table.slice(r - 1, 0, 1, 1),
        (r, 0) => table.slice(r - 1, 0, 1, cols),
        (r, c) => table.slice(r - 1, c - 1, 1, 1),
    }
}

fn choose(ev: &mut Evaluator, args: &[Expr]) -> Eval {
    if args.len() < 2 {
        return Eval::error(CellError::Value);
    }
    let i = tri!(ev.number(args, 0)).trunc();
    if i < 1.0 || i as usize >= args.len() {
        return Eval::error(CellError::Value);
    }
    ev.eval(&args[i as usize])
}

fn row_col(ev: &mut Evaluator, args: &[Expr], row: bool) -> Eval {
    if let Some(e) = arity(args, 0, 1) {
        return e;
    }
    if omitted(args, 0) {
        let n = if row { ev.cell.row } else { ev.cell.col };
        return Eval::number(n as f64 + 1.0);
    }
    let Some(areas) = ev.reference_arg(args, 0) else {
        return Eval::error(CellError::Value);
    };
    let Some(area) = areas.first() else {
        return Eval::error(CellError::Ref);
    };
    let r = area.range;
    let (start, count) = if row { (r.start.row, r.rows()) } else { (r.start.col, r.cols()) };
    // Whole columns or rows give their first number.
    let full = if row { r.rows() == MAX_ROWS } else { r.cols() == MAX_COLS };
    if count == 1 || full {
        return Eval::number(start as f64 + 1.0);
    }
    let data: Vec<Value> = (0..count.min(100_000)).map(|i| Value::Number((start + i) as f64 + 1.0)).collect();
    let n = data.len();
    Eval::Array(if row { Array::new(n, 1, data) } else { Array::new(1, n, data) })
}

fn rows_cols(ev: &mut Evaluator, args: &[Expr], rows: bool) -> Eval {
    if let Some(e) = arity(args, 1, 1) {
        return e;
    }
    match ev.eval(&args[0]) {
        Eval::Ref(areas) => {
            if areas.len() != 1 {
                return Eval::error(CellError::Ref);
            }
            let r = areas[0].range;
            Eval::number(if rows { r.rows() } else { r.cols() } as f64)
        }
        Eval::Array(a) => Eval::number(if rows { a.rows } else { a.cols } as f64),
        Eval::Value(Value::Error(e)) => Eval::error(e),
        Eval::Value(_) => Eval::number(1.0),
    }
}

fn offset(ev: &mut Evaluator, args: &[Expr]) -> Eval {
    if let Some(e) = arity(args, 3, 5) {
        return e;
    }
    ev.volatile = true;
    let Some(areas) = ev.reference_arg(args, 0) else {
        return Eval::error(CellError::Value);
    };
    if areas.len() != 1 {
        return Eval::error(CellError::Value);
    }
    let base = areas[0];
    let dr = tri!(ev.number(args, 1)).trunc() as i64;
    let dc = tri!(ev.number(args, 2)).trunc() as i64;
    let h = if omitted(args, 3) { base.range.rows() as i64 } else { tri!(ev.number(args, 3)).trunc() as i64 };
    let w = if omitted(args, 4) { base.range.cols() as i64 } else { tri!(ev.number(args, 4)).trunc() as i64 };
    if h == 0 || w == 0 {
        return Eval::error(CellError::Ref);
    }
    let Some(start) = base.range.start.offset(dr, dc) else {
        return Eval::error(CellError::Ref);
    };
    // Negative heights and widths extend up and to the left.
    let end_r = start.row as i64 + if h > 0 { h - 1 } else { h + 1 };
    let end_c = start.col as i64 + if w > 0 { w - 1 } else { w + 1 };
    if end_r < 0 || end_c < 0 || end_r >= MAX_ROWS as i64 || end_c >= MAX_COLS as i64 {
        return Eval::error(CellError::Ref);
    }
    Eval::Ref(vec![SheetArea {
        sheet: base.sheet,
        range: Range::new(start, CellRef::new(end_r as u32, end_c as u32)),
    }])
}

fn indirect(ev: &mut Evaluator, args: &[Expr]) -> Eval {
    if let Some(e) = arity(args, 1, 2) {
        return e;
    }
    ev.volatile = true;
    let text = tri!(ev.text(args, 0));
    let a1 = tri!(ev.bool_or(args, 1, true));
    let text = text.trim();
    if text.is_empty() {
        return Eval::error(CellError::Ref);
    }
    if !a1 {
        return match parse_r1c1(text, ev.cell) {
            Some((sheet, range)) => {
                let sheet = match sheet {
                    None => ev.sheet,
                    Some(name) => match ev.grid.sheet_index(&name) {
                        Some(i) => i,
                        None => return Eval::error(CellError::Ref),
                    },
                };
                Eval::Ref(vec![SheetArea { sheet, range }])
            }
            None => Eval::error(CellError::Ref),
        };
    }
    let Ok(expr) = super::super::parse(text) else {
        return Eval::error(CellError::Ref);
    };
    let ok = match &expr {
        Expr::Ref(_) | Expr::Name(_) => true,
        Expr::Binary(super::super::BinaryOp::Range, a, b) => {
            matches!(**a, Expr::Ref(_)) && matches!(**b, Expr::Ref(_))
        }
        _ => false,
    };
    if !ok {
        return Eval::error(CellError::Ref);
    }
    match ev.eval(&expr) {
        r @ Eval::Ref(_) => r,
        _ => Eval::error(CellError::Ref),
    }
}

/// `R2C3`, `R[1]C[-1]`, `R1C1:R5C2`, `Sheet2!R1C1`.
fn parse_r1c1(text: &str, origin: CellRef) -> Option<(Option<String>, Range)> {
    let (sheet, body) = match text.rfind('!') {
        Some(i) => {
            let s = &text[..i];
            let s = s.strip_prefix('\'').and_then(|s| s.strip_suffix('\'')).map(|s| s.replace("''", "'"));
            (Some(s.unwrap_or_else(|| text[..i].to_owned())), &text[i + 1..])
        }
        None => (None, text),
    };
    let cell = |s: &str| -> Option<CellRef> {
        let u = s.to_ascii_uppercase();
        let rest = u.strip_prefix('R')?;
        let cpos = rest.find('C')?;
        let (rp, cp) = (&rest[..cpos], &rest[cpos + 1..]);
        let axis = |p: &str, base: u32| -> Option<i64> {
            if p.is_empty() {
                Some(base as i64)
            } else if let Some(inner) = p.strip_prefix('[').and_then(|x| x.strip_suffix(']')) {
                Some(base as i64 + inner.parse::<i64>().ok()?)
            } else {
                Some(p.parse::<i64>().ok()? - 1)
            }
        };
        let r = axis(rp, origin.row)?;
        let c = axis(cp, origin.col)?;
        if r < 0 || c < 0 || r >= MAX_ROWS as i64 || c >= MAX_COLS as i64 {
            return None;
        }
        Some(CellRef::new(r as u32, c as u32))
    };
    let range = match body.split_once(':') {
        Some((a, b)) => Range::new(cell(a)?, cell(b)?),
        None => Range::cell(cell(body)?),
    };
    Some((sheet, range))
}

fn address(ev: &mut Evaluator, args: &[Expr]) -> Eval {
    if let Some(e) = arity(args, 2, 5) {
        return e;
    }
    let row = tri!(ev.number(args, 0)).trunc();
    let col = tri!(ev.number(args, 1)).trunc();
    let abs = if omitted(args, 2) { 1.0 } else { tri!(ev.number(args, 2)).trunc() };
    let a1 = tri!(ev.bool_or(args, 3, true));
    if row < 1.0 || col < 1.0 || row > MAX_ROWS as f64 || col > MAX_COLS as f64 || !(1.0..=4.0).contains(&abs) {
        return Eval::error(CellError::Value);
    }
    let (row_abs, col_abs) = match abs as u8 {
        1 => (true, true),
        2 => (true, false),
        3 => (false, true),
        _ => (false, false),
    };
    let body = if a1 {
        format!(
            "{}{}{}{}",
            if col_abs { "$" } else { "" },
            col_to_letters(col as u32 - 1),
            if row_abs { "$" } else { "" },
            row as u32
        )
    } else {
        let part = |n: f64, absolute: bool| if absolute { format!("{n}") } else { format!("[{n}]") };
        format!("R{}C{}", part(row, row_abs), part(col, col_abs))
    };
    if omitted(args, 4) {
        return Eval::text(body);
    }
    let sheet = tri!(ev.text(args, 4));
    if sheet.is_empty() {
        return Eval::text(format!("!{body}"));
    }
    Eval::text(format!("{}!{body}", quote_sheet_name(&sheet)))
}

fn transpose(ev: &mut Evaluator, args: &[Expr]) -> Eval {
    if let Some(e) = arity(args, 1, 1) {
        return e;
    }
    let a = super::array_arg(ev, args, 0);
    let mut data = Vec::with_capacity(a.data.len());
    for c in 0..a.cols {
        for r in 0..a.rows {
            data.push(a.get(r, c).clone());
        }
    }
    if data.len() == 1 {
        return Eval::Value(data.pop().unwrap_or_default());
    }
    Eval::Array(Array::new(a.cols, a.rows, data))
}
