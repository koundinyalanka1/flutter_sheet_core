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

use std::collections::{HashMap, HashSet, VecDeque};
use std::sync::Arc;

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
}

impl DepIndex {
    fn add(&mut self, wb: &Workbook, sheet: usize, cell: CellRef, expr: &Expr) {
        let key = (sheet, cell);
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
        self.ensure_index();
        let index = self.calc.index.take().unwrap_or_default();
        let mut dirty: HashSet<Key> = HashSet::new();
        let mut queue: VecDeque<Key> = VecDeque::new();
        let mut edges: HashMap<Key, Vec<Key>> = HashMap::new();
        let is_formula = |wb: &Workbook, k: Key| {
            wb.sheets
                .get(k.0)
                .and_then(|s| s.cell(k.1))
                .is_some_and(|c| c.formula.as_ref().is_some_and(|f| f.expr.is_some()))
        };
        for &k in changed {
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
        let mut seen: HashSet<Key> = HashSet::new();
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
                    if f == x {
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
        let mut adjacency: HashMap<Key, Vec<Key>> = HashMap::new();
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

        let mut calc = Calc::new(self, dirty);
        for k in order.iter().chain(rest.iter()) {
            calc.compute(*k);
        }
        let Calc { changed: out, circular, .. } = calc;
        self.calc.circular = circular;
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

    /// Evaluate a formula as if it were in a cell, without storing it
    /// (for previews such as the status bar's sum).
    pub fn evaluate_in(&mut self, sheet: usize, cell: CellRef, formula: &str) -> Value {
        let Ok(expr) = crate::formula::parse(formula) else {
            return Value::Error(CellError::Name);
        };
        let mut calc = Calc::new(self, HashSet::new());
        evaluate(&mut calc, sheet, cell, &expr).0
    }
}

/// The grid formulas see while recalculating.
struct Calc<'w> {
    wb: &'w mut Workbook,
    pending: HashSet<Key>,
    computing: Vec<Key>,
    changed: Vec<Key>,
    circular: Vec<Key>,
}

impl<'w> Calc<'w> {
    fn new(wb: &'w mut Workbook, pending: HashSet<Key>) -> Self {
        Calc {
            wb,
            pending,
            computing: Vec::new(),
            changed: Vec::new(),
            circular: Vec::new(),
        }
    }

    fn store(&mut self, key: Key, value: Value) {
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
        if !self.pending.remove(&key) {
            return;
        }
        let Some(formula) = self.wb.sheets.get(key.0).and_then(|s| s.cell(key.1)).and_then(|c| c.formula.as_ref()) else {
            return;
        };
        let Some(expr) = formula.expr.clone() else { return };
        let array = formula.array;
        self.computing.push(key);
        match array {
            None => {
                let (value, unsupported, _) = evaluate(self, key.0, key.1, &expr);
                if !unsupported {
                    self.store(key, value);
                }
            }
            Some(range) => {
                let mut ev = Evaluator::new(self, key.0, key.1);
                let result = ev.eval(&expr);
                let unsupported = ev.unsupported;
                let arr = match result {
                    Eval::Value(v) => crate::formula::eval::Array::new(1, 1, vec![v]),
                    other => ev.to_array(other),
                };
                if !unsupported {
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
            }
        }
        self.computing.pop();
    }
}

impl Grid for Calc<'_> {
    fn value(&mut self, sheet: usize, cell: CellRef) -> Value {
        let key = (sheet, cell);
        if self.pending.contains(&key) {
            if self.computing.len() < MAX_NESTING {
                self.compute(key);
            }
        } else if self.computing.contains(&key) {
            if !self.circular.contains(&key) {
                self.circular.push(key);
            }
        } else if let Some(anchor) = self.wb.sheets.get(sheet).and_then(|s| s.cell(cell)).and_then(|c| c.array_member()) {
            // A cell filled by a pending array formula.
            let ak = (sheet, anchor);
            if self.pending.contains(&ak) && self.computing.len() < MAX_NESTING {
                self.compute(ak);
            }
        }
        self.wb.sheets.get(sheet).map(|s| s.value(cell)).unwrap_or_default()
    }

    fn cells_in(&mut self, sheet: usize, range: Range) -> Vec<(CellRef, Value)> {
        let Some(s) = self.wb.sheets.get(sheet) else {
            return Vec::new();
        };
        if self.pending.is_empty() {
            return s
                .cells_in(range)
                .filter(|(_, c)| !c.value.is_empty())
                .map(|(at, c)| (at, c.value.clone()))
                .collect();
        }
        let keys: Vec<CellRef> = s.cells_in(range).map(|(at, _)| at).collect();
        let mut out = Vec::with_capacity(keys.len());
        for at in keys {
            let v = self.value(sheet, at);
            if !v.is_empty() {
                out.push((at, v));
            }
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
