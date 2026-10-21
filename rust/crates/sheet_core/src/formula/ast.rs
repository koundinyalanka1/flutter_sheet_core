//! The parsed form of a formula.

use std::sync::Arc;

use crate::refs::{CellRef, Range, MAX_COLS, MAX_ROWS};
use crate::value::CellError;

/// One end of a reference, with its `$` markers.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct CellAddr {
    pub row: u32,
    pub col: u32,
    pub row_abs: bool,
    pub col_abs: bool,
}

impl CellAddr {
    pub fn cell(self) -> CellRef {
        CellRef::new(self.row, self.col)
    }
}

/// What a reference covers.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Area {
    Cell(CellAddr),
    Range(CellAddr, CellAddr),
    /// Whole columns, `A:C` (rows are ignored).
    Cols(CellAddr, CellAddr),
    /// Whole rows, `1:3` (columns are ignored).
    Rows(CellAddr, CellAddr),
}

impl Area {
    /// The rectangle covered.
    pub fn range(self) -> Range {
        match self {
            Area::Cell(a) => Range::cell(a.cell()),
            Area::Range(a, b) => Range::new(a.cell(), b.cell()),
            Area::Cols(a, b) => Range::new(CellRef::new(0, a.col), CellRef::new(MAX_ROWS - 1, b.col)),
            Area::Rows(a, b) => Range::new(CellRef::new(a.row, 0), CellRef::new(b.row, MAX_COLS - 1)),
        }
    }
}

/// A reference to cells, here or on another sheet.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct Reference {
    /// None for the formula's own sheet.
    pub sheet: Option<String>,
    /// The last sheet of a 3-D reference (`Sheet1:Sheet3!A1`).
    pub last_sheet: Option<String>,
    /// None when the reference became invalid (`#REF!`), for example after
    /// the cells it pointed to were deleted.
    pub area: Option<Area>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum UnaryOp {
    Neg,
    Plus,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BinaryOp {
    Add,
    Sub,
    Mul,
    Div,
    Pow,
    Concat,
    Eq,
    Ne,
    Lt,
    Le,
    Gt,
    Ge,
    /// `:` between two expressions that yield references.
    Range,
    /// `,` inside parentheses: several areas as one reference.
    Union,
    /// A space between references: their common cells.
    Intersect,
}

impl BinaryOp {
    pub fn symbol(self) -> &'static str {
        match self {
            BinaryOp::Add => "+",
            BinaryOp::Sub => "-",
            BinaryOp::Mul => "*",
            BinaryOp::Div => "/",
            BinaryOp::Pow => "^",
            BinaryOp::Concat => "&",
            BinaryOp::Eq => "=",
            BinaryOp::Ne => "<>",
            BinaryOp::Lt => "<",
            BinaryOp::Le => "<=",
            BinaryOp::Gt => ">",
            BinaryOp::Ge => ">=",
            BinaryOp::Range => ":",
            BinaryOp::Union => ",",
            BinaryOp::Intersect => " ",
        }
    }

    /// Binding power for parsing and printing (higher binds tighter).
    pub fn precedence(self) -> u8 {
        match self {
            BinaryOp::Eq | BinaryOp::Ne | BinaryOp::Lt | BinaryOp::Le | BinaryOp::Gt | BinaryOp::Ge => 1,
            BinaryOp::Concat => 2,
            BinaryOp::Add | BinaryOp::Sub => 3,
            BinaryOp::Mul | BinaryOp::Div => 4,
            BinaryOp::Pow => 5,
            BinaryOp::Union => 8,
            BinaryOp::Intersect => 9,
            BinaryOp::Range => 10,
        }
    }
}

/// A formula expression.
#[derive(Clone, Debug, PartialEq)]
pub enum Expr {
    Number(f64),
    Text(Arc<str>),
    Bool(bool),
    Error(CellError),
    Ref(Reference),
    /// A defined name.
    Name(String),
    /// A structured reference to a table (`Table1[Amount]`), kept as written.
    Structured(String),
    Unary(UnaryOp, Box<Expr>),
    Binary(BinaryOp, Box<Expr>, Box<Expr>),
    Chain(Box<Expr>, Vec<(BinaryOp, Expr)>),
    Percent(Box<Expr>),
    /// A function call. The name is upper case, without Excel's `_xlfn.`
    /// prefix.
    Call(String, Vec<Expr>),
    /// An array constant `{1,2;3,4}`, row by row.
    Array(Vec<Vec<Expr>>),
    /// An omitted argument: the middle of `IF(A1,,2)`.
    Missing,
    /// Parentheses as written.
    Paren(Box<Expr>),
}

impl Expr {
    /// Every reference in the expression, depth first.
    pub fn references(&self) -> Vec<&Reference> {
        let mut out = Vec::new();
        self.walk(&mut |e| {
            if let Expr::Ref(r) = e {
                out.push(r);
            }
        });
        out
    }

    /// Visit this expression and everything inside it.
    pub fn walk<'a>(&'a self, f: &mut impl FnMut(&'a Expr)) {
        let mut pending = vec![self];
        while let Some(e) = pending.pop() {
            f(e);
            match e {
                Expr::Unary(_, e) | Expr::Percent(e) | Expr::Paren(e) => pending.push(e),
                Expr::Binary(_, a, b) => {
                    pending.push(b);
                    pending.push(a);
                }
                Expr::Chain(first, rest) => {
                    pending.extend(rest.iter().rev().map(|(_, e)| e));
                    pending.push(first);
                }
                Expr::Call(_, args) => pending.extend(args.iter().rev()),
                Expr::Array(rows) => pending.extend(rows.iter().rev().flat_map(|r| r.iter().rev())),
                _ => {}
            }
        }
    }

    pub(crate) fn within_depth(&self, limit: usize) -> bool {
        let mut pending = vec![(self, 1)];
        while let Some((e, depth)) = pending.pop() {
            if depth > limit {
                return false;
            }
            let next = depth + 1;
            match e {
                Expr::Unary(_, e) | Expr::Percent(e) | Expr::Paren(e) => pending.push((e, next)),
                Expr::Binary(_, a, b) => {
                    pending.push((b, next));
                    pending.push((a, next));
                }
                Expr::Chain(first, rest) => {
                    pending.extend(rest.iter().map(|(_, e)| (e, next)));
                    pending.push((first, next));
                }
                Expr::Call(_, args) => pending.extend(args.iter().map(|e| (e, next))),
                Expr::Array(rows) => pending.extend(rows.iter().flatten().map(|e| (e, next))),
                _ => {}
            }
        }
        true
    }

    /// Visit mutably.
    pub fn walk_mut(&mut self, f: &mut impl FnMut(&mut Expr)) {
        let mut pending = vec![self];
        while let Some(e) = pending.pop() {
            f(e);
            match e {
                Expr::Unary(_, e) | Expr::Percent(e) | Expr::Paren(e) => pending.push(e),
                Expr::Binary(_, a, b) => {
                    pending.push(b);
                    pending.push(a);
                }
                Expr::Chain(first, rest) => {
                    pending.extend(rest.iter_mut().rev().map(|(_, e)| e));
                    pending.push(first);
                }
                Expr::Call(_, args) => pending.extend(args.iter_mut().rev()),
                Expr::Array(rows) => pending.extend(rows.iter_mut().rev().flat_map(|r| r.iter_mut().rev())),
                _ => {}
            }
        }
    }

    /// Names of the functions called, upper case.
    pub fn functions(&self) -> Vec<&str> {
        let mut out = Vec::new();
        self.walk(&mut |e| {
            if let Expr::Call(name, _) = e {
                out.push(name.as_str());
            }
        });
        out
    }
}

impl Drop for Expr {
    fn drop(&mut self) {
        fn detach(e: &mut Expr, pending: &mut Vec<Expr>) {
            match e {
                Expr::Unary(_, e) | Expr::Percent(e) | Expr::Paren(e) => {
                    pending.push(std::mem::replace(e.as_mut(), Expr::Missing));
                }
                Expr::Binary(_, a, b) => {
                    pending.push(std::mem::replace(a.as_mut(), Expr::Missing));
                    pending.push(std::mem::replace(b.as_mut(), Expr::Missing));
                }
                Expr::Chain(first, rest) => {
                    pending.push(std::mem::replace(first.as_mut(), Expr::Missing));
                    pending.extend(rest.drain(..).map(|(_, e)| e));
                }
                Expr::Call(_, args) => pending.append(args),
                Expr::Array(rows) => pending.extend(rows.drain(..).flatten()),
                _ => {}
            }
        }
        let mut pending = Vec::new();
        detach(self, &mut pending);
        while let Some(mut e) = pending.pop() {
            detach(&mut e, &mut pending);
        }
    }
}
