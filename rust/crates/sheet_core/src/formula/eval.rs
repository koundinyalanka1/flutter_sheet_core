//! Evaluating formulas.

use std::sync::Arc;

use super::ast::{Area, BinaryOp, Expr, Reference, UnaryOp};
use super::functions;
use crate::numfmt::FormatOptions;
use crate::refs::{CellRef, Range};
use crate::value::{compare, CellError, Value};

/// What a formula can see of its workbook. The calculation engine
/// implements it; evaluating a cell that holds a formula computes it first
/// if needed.
pub trait Grid {
    /// A cell's value.
    fn value(&mut self, sheet: usize, cell: CellRef) -> Value;
    /// The non-empty cells of a range, row by row.
    fn cells_in(&mut self, sheet: usize, range: Range) -> Vec<(CellRef, Value)>;
    /// The part of a sheet that holds anything (for whole rows/columns).
    fn used_range(&self, sheet: usize) -> Option<Range>;
    fn sheet_index(&self, name: &str) -> Option<usize>;
    fn sheet_count(&self) -> usize;
    /// A defined name, as seen from `sheet`.
    fn defined_name(&self, name: &str, sheet: usize) -> Option<Expr>;
    fn format_options(&self) -> FormatOptions;
    /// The current date and time as a serial, in local time.
    fn now(&self) -> f64;
    /// A random number in [0, 1).
    fn random(&mut self) -> f64;
    /// The formula text of a cell, if it holds one (FORMULATEXT, ISFORMULA).
    fn formula_text(&self, sheet: usize, cell: CellRef) -> Option<String>;
}

/// A rectangle on a particular sheet.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SheetArea {
    pub sheet: usize,
    pub range: Range,
}

/// A two-dimensional array of values, row by row.
#[derive(Clone, Debug, PartialEq)]
pub struct Array {
    pub rows: usize,
    pub cols: usize,
    pub data: Vec<Value>,
}

impl Array {
    pub fn new(rows: usize, cols: usize, data: Vec<Value>) -> Self {
        debug_assert_eq!(rows * cols, data.len());
        Array { rows, cols, data }
    }

    pub fn get(&self, row: usize, col: usize) -> &Value {
        &self.data[row * self.cols + col]
    }

    /// Element for broadcasting: a single row or column repeats.
    fn broadcast(&self, row: usize, col: usize) -> Value {
        let r = if self.rows == 1 { 0 } else { row };
        let c = if self.cols == 1 { 0 } else { col };
        if r < self.rows && c < self.cols {
            self.get(r, c).clone()
        } else {
            Value::Error(CellError::NA)
        }
    }
}

/// The result of evaluating an expression.
#[derive(Clone, Debug, PartialEq)]
pub enum Eval {
    Value(Value),
    /// A reference: one or more areas.
    Ref(Vec<SheetArea>),
    Array(Array),
}

impl Eval {
    pub fn error(e: CellError) -> Eval {
        Eval::Value(Value::Error(e))
    }

    pub fn number(n: f64) -> Eval {
        if n.is_finite() {
            Eval::Value(Value::Number(n))
        } else {
            Eval::error(CellError::Num)
        }
    }

    pub fn text(s: impl Into<Arc<str>>) -> Eval {
        Eval::Value(Value::Text(s.into()))
    }

    pub fn bool(b: bool) -> Eval {
        Eval::Value(Value::Bool(b))
    }

    pub fn as_error(&self) -> Option<CellError> {
        match self {
            Eval::Value(Value::Error(e)) => Some(*e),
            _ => None,
        }
    }
}

impl From<Value> for Eval {
    fn from(v: Value) -> Self {
        Eval::Value(v)
    }
}

impl From<Result<f64, CellError>> for Eval {
    fn from(r: Result<f64, CellError>) -> Self {
        match r {
            Ok(n) => Eval::number(n),
            Err(e) => Eval::error(e),
        }
    }
}

/// Recursion limit for names that refer to names, nested INDIRECTs and the
/// like.
const MAX_DEPTH: usize = 64;

/// Evaluates expressions for one cell.
pub struct Evaluator<'g> {
    pub grid: &'g mut dyn Grid,
    /// The sheet and cell the formula lives in.
    pub sheet: usize,
    pub cell: CellRef,
    depth: usize,
    /// Set when the formula used something this engine cannot compute (an
    /// unknown function, a table reference), so the caller can keep the
    /// value the file was saved with instead.
    pub unsupported: bool,
    /// Set when the formula used a volatile function (NOW, RAND, OFFSET...).
    pub volatile: bool,
}

impl<'g> Evaluator<'g> {
    pub fn new(grid: &'g mut dyn Grid, sheet: usize, cell: CellRef) -> Self {
        Evaluator {
            grid,
            sheet,
            cell,
            depth: 0,
            unsupported: false,
            volatile: false,
        }
    }

    /// The value of a whole formula as a cell shows it: a reference or array
    /// result gives its top-left (or intersecting) value.
    pub fn evaluate(&mut self, expr: &Expr) -> Value {
        let result = self.eval(expr);
        match result {
            Eval::Value(Value::Empty) => Value::Number(0.0),
            Eval::Ref(areas) if areas.len() == 1 && areas[0].range.cell_count() == 1 => {
                match self.grid.value(areas[0].sheet, areas[0].range.start) {
                    Value::Empty => Value::Number(0.0),
                    v => v,
                }
            }
            other => match self.scalar(other) {
                Value::Empty => Value::Number(0.0),
                v => v,
            },
        }
    }

    pub fn eval(&mut self, expr: &Expr) -> Eval {
        self.depth += 1;
        let result = if self.depth > MAX_DEPTH {
            Eval::error(CellError::Calc)
        } else {
            self.eval_inner(expr)
        };
        self.depth -= 1;
        result
    }

    fn eval_inner(&mut self, expr: &Expr) -> Eval {
        match expr {
            Expr::Number(n) => Eval::number(*n),
            Expr::Text(t) => Eval::Value(Value::Text(t.clone())),
            Expr::Bool(b) => Eval::bool(*b),
            Expr::Error(e) => Eval::error(*e),
            Expr::Missing => Eval::Value(Value::Empty),
            Expr::Paren(e) => self.eval(e),
            Expr::Ref(r) => self.reference(r),
            Expr::Name(name) => match self.grid.defined_name(name, self.sheet) {
                Some(e) => self.eval(&e),
                None => Eval::error(CellError::Name),
            },
            Expr::Structured(_) => {
                self.unsupported = true;
                Eval::error(CellError::Ref)
            }
            Expr::Array(rows) => {
                let cols = rows.first().map_or(0, Vec::len);
                let mut data = Vec::with_capacity(rows.len() * cols);
                for row in rows {
                    for e in row {
                        data.push(self.eval_scalar(e));
                    }
                }
                Eval::Array(Array::new(rows.len(), cols, data))
            }
            Expr::Unary(op, e) => {
                let v = self.eval(e);
                self.map_values(v, &mut |x| match x.to_number() {
                    Ok(n) => Value::Number(if *op == UnaryOp::Neg { -n } else { n }),
                    Err(e) => Value::Error(e),
                })
            }
            Expr::Percent(e) => {
                let v = self.eval(e);
                self.map_values(v, &mut |x| match x.to_number() {
                    Ok(n) => Value::Number(n / 100.0),
                    Err(e) => Value::Error(e),
                })
            }
            Expr::Binary(op, a, b) => self.binary(*op, a, b),
            Expr::Call(name, args) => match functions::call(self, name, args) {
                Some(result) => result,
                None => {
                    self.unsupported = true;
                    Eval::error(CellError::Name)
                }
            },
        }
    }

    fn reference(&mut self, r: &Reference) -> Eval {
        let Some(area) = r.area else {
            return Eval::error(CellError::Ref);
        };
        let first = match &r.sheet {
            None => self.sheet,
            Some(name) => match self.grid.sheet_index(name) {
                Some(i) => i,
                None => return Eval::error(CellError::Ref),
            },
        };
        let last = match &r.last_sheet {
            None => first,
            Some(name) => match self.grid.sheet_index(name) {
                Some(i) => i,
                None => return Eval::error(CellError::Ref),
            },
        };
        let (lo, hi) = (first.min(last), first.max(last));
        Eval::Ref((lo..=hi).map(|sheet| SheetArea { sheet, range: area.range() }).collect())
    }

    fn binary(&mut self, op: BinaryOp, a: &Expr, b: &Expr) -> Eval {
        match op {
            BinaryOp::Range | BinaryOp::Union | BinaryOp::Intersect => {
                let (x, y) = (self.eval(a), self.eval(b));
                let (Eval::Ref(x), Eval::Ref(y)) = (x, y) else {
                    return Eval::error(CellError::Value);
                };
                match op {
                    BinaryOp::Union => Eval::Ref(x.into_iter().chain(y).collect()),
                    BinaryOp::Range => {
                        let all: Vec<SheetArea> = x.into_iter().chain(y).collect();
                        let sheet = all[0].sheet;
                        if all.iter().any(|s| s.sheet != sheet) {
                            return Eval::error(CellError::Ref);
                        }
                        let range = all.iter().skip(1).fold(all[0].range, |acc, s| acc.union(s.range));
                        Eval::Ref(vec![SheetArea { sheet, range }])
                    }
                    _ => {
                        let mut out = Vec::new();
                        for p in &x {
                            for q in &y {
                                if p.sheet == q.sheet {
                                    if let Some(range) = p.range.intersection(q.range) {
                                        out.push(SheetArea { sheet: p.sheet, range });
                                    }
                                }
                            }
                        }
                        if out.is_empty() {
                            Eval::error(CellError::Null)
                        } else {
                            Eval::Ref(out)
                        }
                    }
                }
            }
            _ => {
                let (x, y) = (self.eval(a), self.eval(b));
                self.lift2(x, y, &mut |l, r| binary_values(op, l, r))
            }
        }
    }

    /// Apply a scalar operation element by element when either side is a
    /// multi-cell reference or array (dynamic-array semantics).
    pub fn lift2(&mut self, a: Eval, b: Eval, f: &mut dyn FnMut(&Value, &Value) -> Value) -> Eval {
        let a_multi = self.is_multi(&a);
        let b_multi = self.is_multi(&b);
        if !a_multi && !b_multi {
            let (x, y) = (self.scalar(a), self.scalar(b));
            return Eval::Value(f(&x, &y));
        }
        let x = self.to_array(a);
        let y = self.to_array(b);
        let rows = x.rows.max(y.rows);
        let cols = x.cols.max(y.cols);
        let mut data = Vec::with_capacity(rows * cols);
        for r in 0..rows {
            for c in 0..cols {
                data.push(f(&x.broadcast(r, c), &y.broadcast(r, c)));
            }
        }
        Eval::Array(Array::new(rows, cols, data))
    }

    /// Apply a scalar operation to every element.
    pub fn map_values(&mut self, v: Eval, f: &mut dyn FnMut(&Value) -> Value) -> Eval {
        if self.is_multi(&v) {
            let mut arr = self.to_array(v);
            for x in arr.data.iter_mut() {
                *x = f(x);
            }
            Eval::Array(arr)
        } else {
            let x = self.scalar(v);
            Eval::Value(f(&x))
        }
    }

    pub fn is_multi(&self, v: &Eval) -> bool {
        match v {
            Eval::Ref(areas) => areas.len() > 1 || areas.first().is_some_and(|a| a.range.cell_count() > 1),
            Eval::Array(a) => a.rows * a.cols > 1,
            Eval::Value(_) => false,
        }
    }

    /// A reference's cells clipped to where the sheet has content, so whole
    /// columns stay cheap.
    pub fn clip(&self, area: SheetArea) -> Option<Range> {
        let used = self.grid.used_range(area.sheet)?;
        let bounds = Range::new(CellRef::new(0, 0), used.end);
        area.range.intersection(bounds)
    }

    /// Values of a reference or array as a 2-D array (references are
    /// clipped to the used part of their sheet, keeping the top-left).
    pub fn to_array(&mut self, v: Eval) -> Array {
        match v {
            Eval::Array(a) => a,
            Eval::Value(x) => Array::new(1, 1, vec![x]),
            Eval::Ref(areas) => {
                let Some(area) = areas.first().copied() else {
                    return Array::new(1, 1, vec![Value::Error(CellError::Ref)]);
                };
                if areas.len() > 1 {
                    return Array::new(1, 1, vec![Value::Error(CellError::Value)]);
                }
                // Keep the full shape for small areas; clip whole rows/columns.
                let range = if area.range.cell_count() <= 1_000_000 {
                    area.range
                } else {
                    match self.clip(area) {
                        Some(r) => Range::new(area.range.start, r.end.max(area.range.start)),
                        None => Range::cell(area.range.start),
                    }
                };
                let rows = range.rows() as usize;
                let cols = range.cols() as usize;
                if rows * cols > 4_000_000 {
                    return Array::new(1, 1, vec![Value::Error(CellError::Num)]);
                }
                let mut data = vec![Value::Empty; rows * cols];
                for (cell, value) in self.grid.cells_in(area.sheet, range) {
                    let r = (cell.row - range.start.row) as usize;
                    let c = (cell.col - range.start.col) as usize;
                    data[r * cols + c] = value;
                }
                Array::new(rows, cols, data)
            }
        }
    }

    /// One value from any result: a single cell, the cell a range shares
    /// with this formula's row or column (implicit intersection), or an
    /// array's top-left.
    pub fn scalar(&mut self, v: Eval) -> Value {
        match v {
            Eval::Value(x) => x,
            Eval::Array(a) => a.data.into_iter().next().unwrap_or(Value::Empty),
            Eval::Ref(areas) => {
                if areas.len() != 1 {
                    return Value::Error(CellError::Value);
                }
                let SheetArea { sheet, range } = areas[0];
                if range.cell_count() == 1 {
                    return self.grid.value(sheet, range.start);
                }
                if range.cols() == 1 && (range.start.row..=range.end.row).contains(&self.cell.row) {
                    return self.grid.value(sheet, CellRef::new(self.cell.row, range.start.col));
                }
                if range.rows() == 1 && (range.start.col..=range.end.col).contains(&self.cell.col) {
                    return self.grid.value(sheet, CellRef::new(range.start.row, self.cell.col));
                }
                Value::Error(CellError::Value)
            }
        }
    }

    pub fn eval_scalar(&mut self, e: &Expr) -> Value {
        let v = self.eval(e);
        self.scalar(v)
    }

    // ----- argument helpers for functions

    pub fn arg(&mut self, args: &[Expr], i: usize) -> Value {
        match args.get(i) {
            Some(e) => self.eval_scalar(e),
            None => Value::Empty,
        }
    }

    pub fn number(&mut self, args: &[Expr], i: usize) -> Result<f64, CellError> {
        self.arg(args, i).to_number()
    }

    /// An optional number argument (missing or omitted gives `default`).
    pub fn number_or(&mut self, args: &[Expr], i: usize, default: f64) -> Result<f64, CellError> {
        match args.get(i) {
            None | Some(Expr::Missing) => Ok(default),
            Some(_) => self.number(args, i),
        }
    }

    pub fn text(&mut self, args: &[Expr], i: usize) -> Result<Arc<str>, CellError> {
        self.arg(args, i).to_text()
    }

    pub fn boolean(&mut self, args: &[Expr], i: usize) -> Result<bool, CellError> {
        self.arg(args, i).to_bool()
    }

    pub fn bool_or(&mut self, args: &[Expr], i: usize, default: bool) -> Result<bool, CellError> {
        match args.get(i) {
            None | Some(Expr::Missing) => Ok(default),
            Some(_) => self.boolean(args, i),
        }
    }

    /// Every value an argument holds: one for a scalar, every cell of a
    /// reference (non-empty only), every element of an array. The flag says
    /// whether the value came from a reference or array (where text and
    /// booleans are skipped by SUM and friends) rather than typed directly.
    pub fn flatten(&mut self, e: &Expr) -> Vec<(Value, bool)> {
        let v = self.eval(e);
        self.flatten_eval(v)
    }

    pub fn flatten_eval(&mut self, v: Eval) -> Vec<(Value, bool)> {
        match v {
            Eval::Value(x) => vec![(x, false)],
            Eval::Array(a) => a.data.into_iter().map(|x| (x, true)).collect(),
            Eval::Ref(areas) => {
                let mut out = Vec::new();
                for area in areas {
                    if let Some(range) = self.clip(area) {
                        for (_, value) in self.grid.cells_in(area.sheet, range) {
                            out.push((value, true));
                        }
                    }
                }
                out
            }
        }
    }

    /// Numbers for SUM, AVERAGE, MAX and the like: typed arguments are
    /// converted (text that reads as a number, booleans), while text and
    /// booleans inside references are ignored. The first error wins.
    pub fn numbers(&mut self, args: &[Expr]) -> Result<Vec<f64>, CellError> {
        let mut out = Vec::new();
        for arg in args {
            if matches!(arg, Expr::Missing) {
                continue;
            }
            for (value, from_ref) in self.flatten(arg) {
                match value {
                    Value::Number(n) => out.push(n),
                    Value::Error(e) => return Err(e),
                    Value::Bool(b) if !from_ref => out.push(if b { 1.0 } else { 0.0 }),
                    Value::Text(t) if !from_ref => out.push(crate::value::parse_number(&t).ok_or(CellError::Value)?),
                    _ => {}
                }
            }
        }
        Ok(out)
    }

    /// Numbers for the *A functions (AVERAGEA, MAXA...): text counts as 0
    /// and booleans as 1/0, wherever they are.
    pub fn numbers_a(&mut self, args: &[Expr]) -> Result<Vec<f64>, CellError> {
        let mut out = Vec::new();
        for arg in args {
            for (value, from_ref) in self.flatten(arg) {
                match value {
                    Value::Number(n) => out.push(n),
                    Value::Error(e) => return Err(e),
                    Value::Bool(b) => out.push(if b { 1.0 } else { 0.0 }),
                    Value::Text(t) => out.push(if from_ref { 0.0 } else { crate::value::parse_number(&t).ok_or(CellError::Value)? }),
                    Value::Empty => {}
                }
            }
        }
        Ok(out)
    }

    /// A reference argument (for INDEX, OFFSET, ROW...), or None.
    pub fn reference_arg(&mut self, args: &[Expr], i: usize) -> Option<Vec<SheetArea>> {
        match self.eval(args.get(i)?) {
            Eval::Ref(areas) => Some(areas),
            _ => None,
        }
    }

    pub fn options(&self) -> FormatOptions {
        self.grid.format_options()
    }
}

fn arith(op: BinaryOp, x: f64, y: f64) -> Value {
    let r = match op {
        BinaryOp::Add => x + y,
        BinaryOp::Sub => x - y,
        BinaryOp::Mul => x * y,
        BinaryOp::Div => {
            if y == 0.0 {
                return Value::Error(CellError::Div0);
            }
            x / y
        }
        BinaryOp::Pow => {
            if x == 0.0 && y == 0.0 {
                return Value::Error(CellError::Num);
            }
            if x == 0.0 && y < 0.0 {
                return Value::Error(CellError::Div0);
            }
            let r = x.powf(y);
            if r.is_nan() {
                return Value::Error(CellError::Num);
            }
            r
        }
        _ => unreachable!(),
    };
    if r.is_finite() {
        Value::Number(r)
    } else {
        Value::Error(CellError::Num)
    }
}

/// A binary operator on two values.
pub fn binary_values(op: BinaryOp, a: &Value, b: &Value) -> Value {
    if let Value::Error(e) = a {
        return Value::Error(*e);
    }
    if let Value::Error(e) = b {
        return Value::Error(*e);
    }
    match op {
        BinaryOp::Add | BinaryOp::Sub | BinaryOp::Mul | BinaryOp::Div | BinaryOp::Pow => {
            match (a.to_number(), b.to_number()) {
                (Ok(x), Ok(y)) => arith(op, x, y),
                (Err(e), _) | (_, Err(e)) => Value::Error(e),
            }
        }
        BinaryOp::Concat => match (a.to_text(), b.to_text()) {
            (Ok(x), Ok(y)) => {
                let mut s = String::with_capacity(x.len() + y.len());
                s.push_str(&x);
                s.push_str(&y);
                Value::text(s)
            }
            (Err(e), _) | (_, Err(e)) => Value::Error(e),
        },
        BinaryOp::Eq | BinaryOp::Ne | BinaryOp::Lt | BinaryOp::Le | BinaryOp::Gt | BinaryOp::Ge => {
            use std::cmp::Ordering::*;
            let ord = compare(a, b);
            Value::Bool(match op {
                BinaryOp::Eq => ord == Equal,
                BinaryOp::Ne => ord != Equal,
                BinaryOp::Lt => ord == Less,
                BinaryOp::Le => ord != Greater,
                BinaryOp::Gt => ord == Greater,
                _ => ord != Less,
            })
        }
        _ => Value::Error(CellError::Value),
    }
}

/// Evaluate a formula in a cell.
pub fn evaluate(grid: &mut dyn Grid, sheet: usize, cell: CellRef, expr: &Expr) -> (Value, bool, bool) {
    let mut ev = Evaluator::new(grid, sheet, cell);
    let value = ev.evaluate(expr);
    (value, ev.unsupported, ev.volatile)
}

/// Areas of a reference expression that is not evaluated (for dependency
/// tracking): the cells a formula reads directly.
pub fn static_areas(expr: &Expr, grid: &dyn Grid, sheet: usize) -> Vec<SheetArea> {
    let mut out = Vec::new();
    for r in expr.references() {
        let Some(area) = r.area else { continue };
        let first = match &r.sheet {
            None => Some(sheet),
            Some(name) => grid.sheet_index(name),
        };
        let Some(first) = first else { continue };
        let last = r.last_sheet.as_ref().and_then(|n| grid.sheet_index(n)).unwrap_or(first);
        for s in first.min(last)..=first.max(last) {
            out.push(SheetArea { sheet: s, range: area_range(area) });
        }
    }
    out
}

fn area_range(area: Area) -> Range {
    area.range()
}
