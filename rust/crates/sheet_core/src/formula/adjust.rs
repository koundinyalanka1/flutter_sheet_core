//! Keeping formulas pointing at the right cells as a workbook changes.

use super::ast::{Area, CellAddr, Expr, Reference};
use crate::refs::{MAX_COLS, MAX_ROWS};

/// A structural change to one sheet.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Shift {
    InsertRows { at: u32, count: u32 },
    DeleteRows { at: u32, count: u32 },
    InsertCols { at: u32, count: u32 },
    DeleteCols { at: u32, count: u32 },
}

fn refers_to(r: &Reference, formula_sheet: &str, target: &str) -> bool {
    if r.last_sheet.is_some() {
        return false;
    }
    match &r.sheet {
        Some(name) => name.eq_ignore_ascii_case(target),
        None => formula_sheet.eq_ignore_ascii_case(target),
    }
}

/// Adjust a span [start, end] along one axis for an insertion or deletion.
/// Returns None when the whole span was deleted.
fn shift_span(start: u32, end: u32, insert: bool, at: u32, count: u32, max: u32) -> Option<(u32, u32)> {
    if insert {
        let move_point = |p: u32| if p >= at { (p + count).min(max - 1) } else { p };
        // A span starting exactly at the insertion point moves; a span that
        // contains it grows.
        Some((move_point(start), move_point(end)))
    } else {
        let gone_end = at + count; // exclusive
        let new_start = if start >= gone_end {
            start - count
        } else if start >= at {
            at
        } else {
            start
        };
        let new_end = if end >= gone_end {
            end - count
        } else if end >= at {
            if at == 0 {
                return None;
            }
            at - 1
        } else {
            end
        };
        if new_end < new_start || (start >= at && end < gone_end) {
            return None;
        }
        Some((new_start, new_end))
    }
}

fn shift_area(area: Area, shift: Shift) -> Option<Area> {
    let (rows, insert, at, count) = match shift {
        Shift::InsertRows { at, count } => (true, true, at, count),
        Shift::DeleteRows { at, count } => (true, false, at, count),
        Shift::InsertCols { at, count } => (false, true, at, count),
        Shift::DeleteCols { at, count } => (false, false, at, count),
    };
    let max = if rows { MAX_ROWS } else { MAX_COLS };
    let apply = |a: CellAddr, b: CellAddr| -> Option<(CellAddr, CellAddr)> {
        let (s, e) = if rows { (a.row, b.row) } else { (a.col, b.col) };
        let (ns, ne) = shift_span(s, e, insert, at, count, max)?;
        let (mut a, mut b) = (a, b);
        if rows {
            a.row = ns;
            b.row = ne;
        } else {
            a.col = ns;
            b.col = ne;
        }
        Some((a, b))
    };
    match area {
        Area::Cell(a) => {
            let (na, _) = apply(a, a)?;
            Some(Area::Cell(na))
        }
        Area::Range(a, b) => {
            let (na, nb) = apply(a, b)?;
            Some(Area::Range(na, nb))
        }
        // Whole columns are unaffected by rows coming and going, and the
        // other way round.
        Area::Cols(a, b) if rows => Some(Area::Cols(a, b)),
        Area::Rows(a, b) if !rows => Some(Area::Rows(a, b)),
        Area::Cols(a, b) => {
            let (na, nb) = apply(a, b)?;
            Some(Area::Cols(na, nb))
        }
        Area::Rows(a, b) => {
            let (na, nb) = apply(a, b)?;
            Some(Area::Rows(na, nb))
        }
    }
}

/// Adjust a formula on `formula_sheet` after `shift` was applied to
/// `target_sheet`. Returns whether anything changed.
pub fn shift_references(expr: &mut Expr, formula_sheet: &str, target_sheet: &str, shift: Shift) -> bool {
    let mut changed = false;
    expr.walk_mut(&mut |e| {
        if let Expr::Ref(r) = e {
            if let Some(area) = r.area {
                if refers_to(r, formula_sheet, target_sheet) {
                    let new = shift_area(area, shift);
                    if new != Some(area) {
                        r.area = new;
                        changed = true;
                    }
                }
            }
        }
    });
    changed
}

fn offset_addr(a: CellAddr, rows: i64, cols: i64, move_row: bool, move_col: bool) -> Option<CellAddr> {
    let mut out = a;
    if move_row && !a.row_abs {
        let r = a.row as i64 + rows;
        if !(0..MAX_ROWS as i64).contains(&r) {
            return None;
        }
        out.row = r as u32;
    }
    if move_col && !a.col_abs {
        let c = a.col as i64 + cols;
        if !(0..MAX_COLS as i64).contains(&c) {
            return None;
        }
        out.col = c as u32;
    }
    Some(out)
}

/// Move a formula's relative references by an offset, as copying or
/// filling it does. References pushed off the sheet become `#REF!`.
pub fn offset_references(expr: &mut Expr, rows: i64, cols: i64) -> bool {
    if rows == 0 && cols == 0 {
        return false;
    }
    let mut changed = false;
    expr.walk_mut(&mut |e| {
        if let Expr::Ref(r) = e {
            let Some(area) = r.area else { return };
            let new = match area {
                Area::Cell(a) => offset_addr(a, rows, cols, true, true).map(Area::Cell),
                Area::Range(a, b) => match (offset_addr(a, rows, cols, true, true), offset_addr(b, rows, cols, true, true)) {
                    (Some(a), Some(b)) => Some(Area::Range(a, b)),
                    _ => None,
                },
                Area::Cols(a, b) => match (offset_addr(a, rows, cols, false, true), offset_addr(b, rows, cols, false, true)) {
                    (Some(a), Some(b)) => Some(Area::Cols(a, b)),
                    _ => None,
                },
                Area::Rows(a, b) => match (offset_addr(a, rows, cols, true, false), offset_addr(b, rows, cols, true, false)) {
                    (Some(a), Some(b)) => Some(Area::Rows(a, b)),
                    _ => None,
                },
            };
            if new != Some(area) {
                r.area = new;
                changed = true;
            }
        }
    });
    changed
}

/// Follow a sheet rename.
pub fn rename_sheet(expr: &mut Expr, old: &str, new: &str) -> bool {
    let mut changed = false;
    expr.walk_mut(&mut |e| {
        if let Expr::Ref(r) = e {
            for name in [&mut r.sheet, &mut r.last_sheet].into_iter().flatten() {
                if name.eq_ignore_ascii_case(old) {
                    *name = new.to_owned();
                    changed = true;
                }
            }
        }
    });
    changed
}

/// References to a deleted sheet become `#REF!`.
pub fn invalidate_sheet(expr: &mut Expr, deleted: &str) -> bool {
    let mut changed = false;
    expr.walk_mut(&mut |e| {
        if let Expr::Ref(r) = e {
            let hit = [&r.sheet, &r.last_sheet]
                .into_iter()
                .flatten()
                .any(|n| n.eq_ignore_ascii_case(deleted));
            if hit && r.area.is_some() {
                r.area = None;
                changed = true;
            }
        }
    });
    changed
}

/// Move references to cells inside `from` by the offset that took that
/// block to `to_top_left` on the same sheet (cutting and pasting a block,
/// or sorting rows, moves the cells formulas point at).
pub fn move_block(
    expr: &mut Expr,
    formula_sheet: &str,
    target_sheet: &str,
    from: crate::refs::Range,
    to_top_left: crate::refs::CellRef,
) -> bool {
    let dr = to_top_left.row as i64 - from.start.row as i64;
    let dc = to_top_left.col as i64 - from.start.col as i64;
    let mut changed = false;
    expr.walk_mut(&mut |e| {
        if let Expr::Ref(r) = e {
            if !refers_to(r, formula_sheet, target_sheet) {
                return;
            }
            let Some(area) = r.area else { return };
            let range = area.range();
            if !(from.contains(range.start) && from.contains(range.end)) {
                return;
            }
            let mv = |a: CellAddr| -> Option<CellAddr> {
                let mut out = a;
                out.row = u32::try_from(a.row as i64 + dr).ok().filter(|&r| r < MAX_ROWS)?;
                out.col = u32::try_from(a.col as i64 + dc).ok().filter(|&c| c < MAX_COLS)?;
                Some(out)
            };
            let new = match area {
                Area::Cell(a) => mv(a).map(Area::Cell),
                Area::Range(a, b) => mv(a).zip(mv(b)).map(|(a, b)| Area::Range(a, b)),
                other => Some(other),
            };
            if new != Some(area) {
                r.area = new;
                changed = true;
            }
        }
    });
    changed
}
