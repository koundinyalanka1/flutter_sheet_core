//! Recalculation.
//!
//! A workbook opens with the values its file was saved with (Excel's own
//! results). After an edit, only the formulas downstream of the changed
//! cells are computed again, in dependency order, plus volatile formulas
//! (NOW, RAND, OFFSET, INDIRECT...). Dependencies come from a reverse index
//! built lazily from the formulas' references.
//!
//! A formula this engine cannot compute (an unknown function, a table
//! reference) keeps its saved value. Circular references keep their values
//! and are reported.

use std::collections::VecDeque;
use std::sync::{Arc, atomic::{AtomicBool, Ordering}};
use std::time::{Duration, Instant};

use crate::fasthash::{FastMap as HashMap, FastSet as HashSet};
use crate::formula::ast::{Area, Expr};
use crate::formula::eval::{evaluate, Eval, Evaluator, Grid};
use crate::model::Workbook;
use crate::numfmt::FormatOptions;
use crate::refs::{CellRef, Range, MAX_COLS};
use crate::value::{CellError, Value};

/// A cell anywhere in the workbook: (sheet index, cell).
pub type Key = (usize, CellRef);

/// Ranges wider than this many columns go in one list instead of one entry
/// per column.
const WIDE: u32 = 64;
/// How deep on-demand computation may nest (through INDIRECT and the like).
const MAX_NESTING: usize = 48;
/// Rebuild the index after this many incremental additions.
const STALE_LIMIT: usize = 20_000;

#[derive(Debug, Default)]
struct DepIndex {
    /// Formulas that read one cell.
    single: HashMap<Key, Vec<Key>>,
    /// Formulas that read a range, by (sheet, column): rows first..=last.
    cols: HashMap<(usize, u32), Vec<(u32, u32, Key)>>,
    /// Formulas that read ranges wider than [`WIDE`] columns.
    wide: Vec<(usize, Range, Key)>,
    /// Formulas recalculated after every edit: volatile functions and
    /// references built at run time (OFFSET, INDIRECT).
    always: HashSet<Key>,
    areas: HashMap<Key, Vec<(usize, Range)>>,
}

impl DepIndex {
    fn remove(&mut self, key: Key) {
        self.always.remove(&key);
        for (s, range) in self.areas.remove(&key).unwrap_or_default() {
            if range.cell_count() == 1 {
                if let Some(list) = self.single.get_mut(&(s, range.start)) {
                    list.retain(|k| *k != key);
                }
            } else if range.cols() > WIDE {
                self.wide.retain(|(_, _, k)| *k != key);
            } else {
                for c in range.start.col..=range.end.col {
                    if let Some(list) = self.cols.get_mut(&(s, c)) {
                        list.retain(|(_, _, k)| *k != key);
                    }
                }
            }
        }
    }

    fn add(&mut self, wb: &Workbook, sheet: usize, cell: CellRef, expr: &Expr) {
        let key = (sheet, cell);
        self.remove(key);
        let mut dynamic = false;
        for name in expr.functions() {
            if matches!(name, "NOW" | "TODAY" | "RAND" | "RANDBETWEEN" | "OFFSET" | "INDIRECT") {
                dynamic = true;
            }
        }
        if dynamic {
            self.always.insert(key);
        }
        let mut areas = Vec::new();
        collect_areas(wb, expr, sheet, &mut areas, 0);
        self.areas.insert(key, areas.clone());
        for (s, range) in areas {
            if range.cell_count() == 1 {
                let list = self.single.entry((s, range.start)).or_default();
                if !list.contains(&key) {
                    list.push(key);
                }
            } else if range.cols() > WIDE {
                self.wide.push((s, range, key));
            } else {
                for c in range.start.col..=range.end.col {
                    self.cols.entry((s, c)).or_default().push((range.start.row, range.end.row, key));
                }
            }
        }
    }

    fn dependents(&self, at: Key, out: &mut Vec<Key>) {
        out.clear();
        if let Some(list) = self.single.get(&at) {
            out.extend_from_slice(list);
        }
        if let Some(list) = self.cols.get(&(at.0, at.1.col)) {
            for (r0, r1, k) in list {
                if (*r0..=*r1).contains(&at.1.row) {
                    out.push(*k);
                }
            }
        }
        for (s, range, k) in &self.wide {
            if *s == at.0 && range.contains(at.1) {
                out.push(*k);
            }
        }
        out.sort_unstable();
        out.dedup();
    }
}

/// The areas a formula reads directly (names expanded).
fn collect_areas(wb: &Workbook, expr: &Expr, sheet: usize, out: &mut Vec<(usize, Range)>, depth: usize) {
    if depth > 8 {
        return;
    }
    expr.walk(&mut |e| match e {
        Expr::Ref(r) => {
            let Some(area) = r.area else { return };
            let first = match &r.sheet {
                None => Some(sheet),
                Some(name) => wb.sheet_index(name),
            };
            let Some(first) = first else { return };
            let last = r.last_sheet.as_ref().and_then(|n| wb.sheet_index(n)).unwrap_or(first);
            let range = match area {
                // Whole rows are indexed as wide ranges.
                Area::Rows(..) => Range::new(
                    CellRef::new(area.range().start.row, 0),
                    CellRef::new(area.range().end.row, MAX_COLS - 1),
                ),
                _ => area.range(),
            };
            for s in first.min(last)..=first.max(last) {
                out.push((s, range));
            }
        }
        Expr::Name(name) => {
            if let Some(inner) = wb.name_expr(name, sheet) {
                collect_areas(wb, &inner, sheet, out, depth + 1);
            }
        }
        _ => {}
    });
}

/// Recalculation state kept with a workbook.
#[derive(Debug)]
pub(crate) struct CalcState {
    index: Option<DepIndex>,
    stale: usize,
    /// Cells found in circular references by the last recalculation.
    pub circular: Vec<Key>,
    /// Formula cells read without a saved value; computed after opening.
    pub needs_calc: Vec<Key>,
    /// Minutes east of UTC, for NOW and TODAY.
    pub utc_offset_minutes: i32,
    /// A fixed "now" (tests).
    pub fixed_now: Option<f64>,
    budget: Duration,
    cancel: Arc<AtomicBool>,
    rng: u64,
}

impl Default for CalcState {
    fn default() -> Self {
        let seed = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos() as u64)
            .unwrap_or(0x9E37_79B9_7F4A_7C15);
        CalcState {
            index: None,
            stale: 0,
            circular: Vec::new(),
            needs_calc: Vec::new(),
            utc_offset_minutes: 0,
            fixed_now: None,
            budget: Duration::from_secs(2),
            cancel: Arc::new(AtomicBool::new(false)),
            rng: seed | 1,
        }
    }
}

impl CalcState {
    /// Forget the dependency index (after rows, columns or sheets moved).
    pub fn invalidate(&mut self) {
        self.index = None;
        self.stale = 0;
    }
}

impl Workbook {
    fn ensure_index(&mut self) {
        if self.calc.index.is_some() && self.calc.stale < STALE_LIMIT {
            return;
        }
        let mut index = DepIndex::default();
        for (s, sheet) in self.sheets.iter().enumerate() {
            for (&r, row) in &sheet.cells {
                for (&c, cell) in row {
                    if let Some(expr) = cell.formula.as_ref().and_then(|f| f.expr.clone()) {
                        index.add(self, s, CellRef::new(r, c), &expr);
                    }
                }
            }
        }
        self.calc.index = Some(index);
        self.calc.stale = 0;
    }

    /// Record a new or changed formula in the dependency index.
    pub(crate) fn index_formula(&mut self, sheet: usize, cell: CellRef) {
        let Some(index) = self.calc.index.take() else { return };
        let mut index = index;
        index.remove((sheet, cell));
        if let Some(expr) = self.sheets[sheet].cell(cell).and_then(|c| c.formula.as_ref()).and_then(|f| f.expr.clone()) {
            index.add(self, sheet, cell, &expr);
            self.calc.stale += 1;
        }
        self.calc.index = Some(index);
    }

    /// Recalculate everything that depends on `changed` (cells whose value
    /// or formula changed), plus volatile formulas. Returns the cells whose
    /// values changed as a result.
    pub fn recalc_from(&mut self, changed: &[Key]) -> Vec<Key> {
        let deadline = Instant::now() + self.calc.budget;
        self.ensure_index();
        let index = self.calc.index.take().unwrap_or_default();
        let mut dirty: HashSet<Key> = HashSet::default();
        let mut queue: VecDeque<Key> = VecDeque::new();
        let mut edges: HashMap<Key, Vec<Key>> = HashMap::default();
        let is_formula = |wb: &Workbook, k: Key| {
            wb.sheets
                .get(k.0)
                .and_then(|s| s.cell(k.1))
                .is_some_and(|c| c.formula.as_ref().is_some_and(|f| f.expr.is_some()))
        };
        for &k in changed.iter().chain(self.calc.needs_calc.iter()) {
            if is_formula(self, k) {
                dirty.insert(k);
            }
            queue.push_back(k);
        }
        for &k in &index.always {
            if is_formula(self, k) && dirty.insert(k) {
                queue.push_back(k);
            }
        }
        let mut seen: HashSet<Key> = HashSet::default();
        let mut deps = Vec::new();
        let mut members = Vec::new();
        while let Some(x) = queue.pop_front() {
            if !seen.insert(x) {
                continue;
            }
            // An array formula's whole range changes with it.
            members.clear();
            members.push(x.1);
            if let Some(range) = self.sheets.get(x.0).and_then(|s| s.cell(x.1)).and_then(|c| c.formula.as_ref()).and_then(|f| f.array) {
                members.extend(range.iter().filter(|c| *c != x.1).take(100_000));
            }
            for &m in &members {
                index.dependents((x.0, m), &mut deps);
                for &f in &deps {
                    if !is_formula(self, f) {
                        continue;
                    }
                    edges.entry(x).or_default().push(f);
                    if dirty.insert(f) {
                        queue.push_back(f);
                    }
                }
            }
        }
        self.calc.index = Some(index);

        // Kahn's algorithm over the dirty formulas.
        let mut indegree: HashMap<Key, usize> = dirty.iter().map(|k| (*k, 0)).collect();
        let mut adjacency: HashMap<Key, Vec<Key>> = HashMap::default();
        for (x, targets) in &edges {
            if !dirty.contains(x) {
                continue;
            }
            let mut ts = targets.clone();
            ts.sort_unstable();
            ts.dedup();
            for t in &ts {
                *indegree.entry(*t).or_default() += 1;
            }
            adjacency.insert(*x, ts);
        }
        let mut ready: Vec<Key> = indegree.iter().filter(|(_, d)| **d == 0).map(|(k, _)| *k).collect();
        ready.sort_unstable_by(|a, b| b.cmp(a));
        let mut order = Vec::with_capacity(dirty.len());
        while let Some(k) = ready.pop() {
            order.push(k);
            if let Some(ts) = adjacency.get(&k) {
                for t in ts {
                    if let Some(d) = indegree.get_mut(t) {
                        *d -= 1;
                        if *d == 0 {
                            ready.push(*t);
                        }
                    }
                }
            }
        }
        // What is left is in, or downstream of, a cycle.
        let mut rest: Vec<Key> = indegree.iter().filter(|(_, d)| **d > 0).map(|(k, _)| *k).collect();
        rest.sort_unstable();

        let mut calc = Calc::new(self, dirty, deadline);
        for k in &rest {
            calc.pending.remove(k);
            calc.blocked.insert(*k);
        }
        calc.circular = rest;
        for k in &order {
            if calc.stopped() {
                break;
            }
            calc.compute(*k);
        }
        let Calc { changed: out, mut circular, pending, .. } = calc;
        circular.sort_unstable();
        circular.dedup();
        self.calc.circular = circular;
        self.calc.needs_calc = pending.into_iter().collect();
        self.calc.needs_calc.sort_unstable();
        if !out.is_empty() {
            self.recalculated = true;
            for (s, _) in &out {
                if let Some(sheet) = self.sheets.get_mut(*s) {
                    sheet.modified = true;
                }
            }
        }
        out
    }

    /// Recalculate every formula in the workbook.
    pub fn recalc_all(&mut self) -> Vec<Key> {
        let mut all = Vec::new();
        for (s, sheet) in self.sheets.iter().enumerate() {
            for (&r, row) in &sheet.cells {
                for (&c, cell) in row {
                    if cell.formula.as_ref().is_some_and(|f| f.expr.is_some()) {
                        all.push((s, CellRef::new(r, c)));
                    }
                }
            }
        }
        self.recalc_from(&all)
    }

    /// Compute formulas that were read without a saved value.
    pub fn calc_missing(&mut self) {
        let missing = std::mem::take(&mut self.calc.needs_calc);
        if !missing.is_empty() {
            self.recalc_from(&missing);
        }
    }

    /// Cells in circular references found by the last recalculation.
    pub fn circular_references(&self) -> &[Key] {
        &self.calc.circular
    }

    pub fn set_utc_offset(&mut self, minutes: i32) {
        self.calc.utc_offset_minutes = minutes;
    }

    pub fn set_fixed_now(&mut self, serial: Option<f64>) {
        self.calc.fixed_now = serial;
    }

    pub fn set_calculation_budget(&mut self, budget: Duration) {
        self.calc.budget = budget.min(Duration::from_secs(60));
    }

    pub fn calculation_cancel_flag(&self) -> Arc<AtomicBool> {
        self.calc.cancel.clone()
    }

    pub fn calculation_pending(&self) -> usize {
        self.calc.needs_calc.len()
    }

    /// Evaluate a formula as if it were in a cell, without storing it
    /// (for previews such as the status bar's sum).
    pub fn evaluate_in(&mut self, sheet: usize, cell: CellRef, formula: &str) -> Value {
        let Ok(expr) = crate::formula::parse(formula) else {
            return Value::Error(CellError::Name);
        };
        let deadline = Instant::now() + self.calc.budget;
        let mut calc = Calc::new(self, HashSet::default(), deadline);
        evaluate(&mut calc, sheet, cell, &expr).0
    }
}

/// The grid formulas see while recalculating.
struct Calc<'w> {
    wb: &'w mut Workbook,
    pending: HashSet<Key>,
    requested: Option<Key>,
    blocked: HashSet<Key>,
    blocked_read: bool,
    deadline: Instant,
    lines: HashMap<(usize, Range, bool), Arc<[Value]>>,
    line_values: usize,
    changed: Vec<Key>,
    circular: Vec<Key>,
}

impl<'w> Calc<'w> {
    fn new(wb: &'w mut Workbook, pending: HashSet<Key>, deadline: Instant) -> Self {
        Calc {
            wb,
            pending,
            requested: None,
            blocked: HashSet::default(),
            blocked_read: false,
            deadline,
            lines: HashMap::default(),
            line_values: 0,
            changed: Vec::new(),
            circular: Vec::new(),
        }
    }

    fn stopped(&self) -> bool {
        self.wb.calc.cancel.load(Ordering::Relaxed) || Instant::now() >= self.deadline
    }

    fn store(&mut self, key: Key, value: Value) {
        self.lines.retain(|(s, range, _), _| *s != key.0 || !range.contains(key.1));
        self.line_values = self.lines.values().map(|v| v.len()).sum();
        let Some(sheet) = self.wb.sheets.get_mut(key.0) else { return };
        match sheet.cell_mut(key.1) {
            Some(cell) => {
                if cell.value != value {
                    cell.value = value;
                    cell.clear_source();
                    self.changed.push(key);
                }
            }
            None => {
                if !value.is_empty() {
                    sheet.set(key.1, crate::model::Cell::value(value));
                    self.changed.push(key);
                }
            }
        }
    }

    /// Compute one formula cell (if it is still pending).
    fn compute(&mut self, key: Key) {
        if !self.pending.contains(&key) {
            return;
        }
        let mut work = Vec::with_capacity(MAX_NESTING);
        let mut visiting = HashSet::default();
        work.push(key);
        visiting.insert(key);
        while let Some(&key) = work.last() {
            if self.stopped() {
                break;
            }
            let formula = self.wb.sheets.get(key.0).and_then(|s| s.cell(key.1)).and_then(|c| c.formula.as_ref());
            let Some((expr, array)) = formula.and_then(|f| f.expr.clone().map(|e| (e, f.array))) else {
                self.pending.remove(&key);
                visiting.remove(&key);
                work.pop();
                continue;
            };
            self.requested = None;
            self.blocked_read = false;
            let rng = self.wb.calc.rng;
            let (result, unsupported) = match array {
                None => {
                    let (value, unsupported, _) = evaluate(self, key.0, key.1, &expr);
                    (Eval::Value(value), unsupported)
                }
                Some(_) => {
                    let mut ev = Evaluator::new(self, key.0, key.1);
                    ev.array = true;
                    let result = ev.eval(&expr);
                    let arr = ev.to_array(result);
                    (Eval::Array(arr), ev.unsupported)
                }
            };
            if self.stopped() {
                self.wb.calc.rng = rng;
                break;
            }
            if self.blocked_read {
                self.wb.calc.rng = rng;
                self.blocked.insert(key);
                self.circular.push(key);
            } else if let Some(dependency) = self.requested.take() {
                self.wb.calc.rng = rng;
                if visiting.contains(&dependency) {
                    for k in work.drain(..) {
                        self.blocked.insert(k);
                        self.circular.push(k);
                        self.pending.remove(&k);
                    }
                    break;
                }
                work.push(dependency);
                visiting.insert(dependency);
                continue;
            } else if !unsupported {
                match (array, result) {
                    (Some(range), Eval::Array(arr)) => {
                        for cell in range.iter().take(1_000_000) {
                            let r = (cell.row - range.start.row) as usize;
                            let c = (cell.col - range.start.col) as usize;
                            // A single value fills the range; a too-small array
                            // leaves #N/A beyond its edges, as in Excel.
                            let v = if arr.rows * arr.cols == 1 {
                                arr.get(0, 0).clone()
                            } else if (arr.rows == 1 || r < arr.rows) && (arr.cols == 1 || c < arr.cols) {
                                arr.broadcast(r, c)
                            } else {
                                Value::Error(CellError::NA)
                            };
                            let v = if v.is_empty() { Value::Number(0.0) } else { v };
                            self.store((key.0, cell), v);
                        }
                    }
                    (_, Eval::Value(value)) => self.store(key, value),
                    _ => {}
                }
            }
            self.pending.remove(&key);
            visiting.remove(&key);
            work.pop();
        }
        self.requested = None;
        self.blocked_read = false;
    }
}

impl Grid for Calc<'_> {
    fn value(&mut self, sheet: usize, cell: CellRef) -> Value {
        let Some(c) = self.wb.sheets.get(sheet).and_then(|s| s.cell(cell)) else {
            return Value::Empty;
        };
        let key = if c.formula.is_some() {
            Some((sheet, cell))
        } else {
            // A cell filled by a pending array formula.
            c.array_member().map(|anchor| (sheet, anchor))
        };
        if let Some(key) = key {
            if self.blocked.contains(&key) {
                self.blocked_read = true;
            } else if self.pending.contains(&key) {
                self.requested.get_or_insert(key);
                return Value::Error(CellError::Calc);
            }
        }
        c.value.clone()
    }

    fn cells_in(&mut self, sheet: usize, range: Range) -> Vec<(CellRef, Value)> {
        let Some(s) = self.wb.sheets.get(sheet) else {
            return Vec::new();
        };
        let mut out = Vec::new();
        for (i, (at, c)) in s.cells_in(range).enumerate() {
            if i % 1024 == 0 && self.stopped() {
                break;
            }
            let key = if c.formula.is_some() { Some((sheet, at)) } else { c.array_member().map(|a| (sheet, a)) };
            if let Some(key) = key {
                if self.blocked.contains(&key) {
                    self.blocked_read = true;
                    break;
                }
                if self.pending.contains(&key) {
                    self.requested.get_or_insert(key);
                    break;
                }
            }
            if !c.value.is_empty() {
                out.push((at, c.value.clone()));
            }
        }
        out
    }

    fn interrupted(&self) -> bool {
        self.requested.is_some() || self.blocked_read || self.stopped()
    }

    fn lookup_line(&mut self, sheet: usize, range: Range, column: bool) -> Arc<[Value]> {
        let key = (sheet, range, column);
        if let Some(line) = self.lines.get(&key) {
            return line.clone();
        }
        let len = if column { range.rows() } else { range.cols() } as usize;
        let mut out = vec![Value::Empty; len];
        for (cell, value) in self.cells_in(sheet, range) {
            let i = if column { cell.row - range.start.row } else { cell.col - range.start.col };
            out[i as usize] = value;
        }
        let out: Arc<[Value]> = out.into();
        if !self.interrupted() && len <= 1_000_000 {
            if self.lines.len() >= 128 || self.line_values + len > 1_000_000 {
                self.lines.clear();
                self.line_values = 0;
            }
            self.line_values += len;
            self.lines.insert(key, out.clone());
        }
        out
    }

    fn used_range(&self, sheet: usize) -> Option<Range> {
        self.wb.sheets.get(sheet)?.used_range()
    }

    fn sheet_index(&self, name: &str) -> Option<usize> {
        self.wb.sheet_index(name)
    }

    fn sheet_count(&self) -> usize {
        self.wb.sheets.len()
    }

    fn defined_name(&self, name: &str, sheet: usize) -> Option<Expr> {
        self.wb.name_expr(name, sheet).map(|e| Arc::unwrap_or_clone(e))
    }

    fn format_options(&self) -> FormatOptions {
        self.wb.format_options()
    }

    fn now(&self) -> f64 {
        if let Some(n) = self.wb.calc.fixed_now {
            return n;
        }
        let secs = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs_f64())
            .unwrap_or(0.0);
        let local = secs + self.wb.calc.utc_offset_minutes as f64 * 60.0;
        let serial_1970 = if self.wb.date1904 { 24_107.0 } else { 25_569.0 };
        serial_1970 + local / 86_400.0
    }

    fn random(&mut self) -> f64 {
        // xorshift64*
        let mut x = self.wb.calc.rng;
        x ^= x >> 12;
        x ^= x << 25;
        x ^= x >> 27;
        self.wb.calc.rng = x;
        (x.wrapping_mul(0x2545_F491_4F6C_DD1D) >> 11) as f64 / (1u64 << 53) as f64
    }

    fn formula_text(&self, sheet: usize, cell: CellRef) -> Option<String> {
        self.wb.sheets.get(sheet)?.cell(cell)?.formula.as_ref().map(|f| f.text.clone())
    }

    fn row_hidden(&self, sheet: usize, row: u32) -> bool {
        self.wb.sheets.get(sheet).is_some_and(|s| s.row_hidden(row))
    }

    fn sheet_name(&self, sheet: usize) -> Option<String> {
        self.wb.sheets.get(sheet).map(|s| s.name.clone())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{Cell, Formula};

    fn formula(wb: &mut Workbook, row: u32, col: u32, text: &str, cached: f64) {
        wb.sheets[0].set(CellRef::new(row, col), Cell {
            value: Value::Number(cached),
            formula: Some(Box::new(Formula::new(text))),
            ..Cell::default()
        });
    }

    #[test]
    fn chains_and_cycles_use_bounded_stack() {
        for stack in [512 * 1024, 1024 * 1024] {
            std::thread::Builder::new().stack_size(stack).spawn(|| {
                let started = Instant::now();
                let mut wb = Workbook::new();
                formula(&mut wb, 0, 1, &vec!["1"; 4096].join("+"), -1.0);
                for row in 0..1000 {
                    let text = if row == 999 { "1".into() } else { format!("SUM(INDIRECT(\"A{}\"))+1", row + 2) };
                    formula(&mut wb, row, 0, &text, -1.0);
                }
                wb.recalc_all();
                assert_eq!(wb.sheets[0].value(CellRef::new(0, 0)), Value::Number(1000.0));
                assert_eq!(wb.sheets[0].value(CellRef::new(0, 1)), Value::Number(4096.0));
                assert_eq!(wb.calculation_pending(), 0);
                assert!(wb.circular_references().is_empty());
                for dynamic in [false, true] {
                    let mut wb = Workbook::new();
                    for row in 0..300 {
                        let next = (row + 1) % 300 + 1;
                        let text = if dynamic { format!("SUM(INDIRECT(\"A{next}\"))+1") } else { format!("SUM(A{next})+1") };
                        formula(&mut wb, row, 0, &text, row as f64);
                    }
                    wb.recalc_all();
                    assert_eq!(wb.circular_references().len(), 300);
                    for row in 0..300 {
                        assert_eq!(wb.sheets[0].value(CellRef::new(row, 0)), Value::Number(row as f64));
                    }
                }
                assert!(started.elapsed() < Duration::from_secs(2));
            }).unwrap().join().unwrap();
        }
    }

    #[test]
    fn cancellation_and_deadline_leave_formulas_dirty() {
        let mut wb = Workbook::new();
        formula(&mut wb, 0, 0, "1+2", 99.0);
        let cancel = wb.calculation_cancel_flag();
        cancel.store(true, Ordering::Relaxed);
        assert!(wb.recalc_all().is_empty());
        assert_eq!(wb.calculation_pending(), 1);
        assert_eq!(wb.sheets[0].value(CellRef::new(0, 0)), Value::Number(99.0));
        cancel.store(false, Ordering::Relaxed);
        wb.set_calculation_budget(Duration::ZERO);
        assert!(wb.recalc_from(&[]).is_empty());
        assert_eq!(wb.calculation_pending(), 1);
        wb.set_calculation_budget(Duration::from_secs(2));
        wb.recalc_from(&[]);
        assert_eq!(wb.calculation_pending(), 0);
        assert_eq!(wb.sheets[0].value(CellRef::new(0, 0)), Value::Number(3.0));
    }

    #[test]
    fn unsupported_formulas_keep_cached_values() {
        let mut wb = Workbook::new();
        formula(&mut wb, 0, 0, "UNKNOWN(A2)", 123.0);
        formula(&mut wb, 1, 0, "7", -1.0);
        wb.recalc_all();
        assert_eq!(wb.sheets[0].value(CellRef::new(0, 0)), Value::Number(123.0));
        assert_eq!(wb.calculation_pending(), 0);
    }

    #[test]
    fn five_thousand_lookups_reuse_key_columns() {
        let mut wb = Workbook::new();
        for row in 0..5000 {
            wb.sheets[0].set(CellRef::new(row, 0), Cell::value(Value::Number(row as f64)));
            wb.sheets[0].set(CellRef::new(row, 1), Cell::value(Value::Number((row * 2) as f64)));
            formula(&mut wb, row, 2, &format!("VLOOKUP({row},$A$1:$B$5000,2,FALSE)"), -1.0);
        }
        let started = Instant::now();
        wb.recalc_all();
        assert_eq!(wb.calculation_pending(), 0);
        for row in 0..5000 {
            assert_eq!(wb.sheets[0].value(CellRef::new(row, 2)), Value::Number((row * 2) as f64));
        }
        assert!(started.elapsed() < Duration::from_secs(1));
    }
}
