//! Criteria for COUNTIF, SUMIF and the *IFS functions, and the `*`, `?`
//! and `~` wildcards they share with MATCH, VLOOKUP and SEARCH.

use std::cmp::Ordering;

use super::super::ast::Expr;
use super::super::eval::{Array, Eval, Evaluator, SheetArea};
use crate::input::parse_date_time;
use crate::numfmt::FormatOptions;
use crate::refs::{CellRef, Range};
use crate::value::{compare_text, parse_number, CellError, Value};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Op {
    Eq,
    Ne,
    Lt,
    Le,
    Gt,
    Ge,
}

impl Op {
    fn test(self, ord: Ordering) -> bool {
        match self {
            Op::Eq => ord == Ordering::Equal,
            Op::Ne => ord != Ordering::Equal,
            Op::Lt => ord == Ordering::Less,
            Op::Le => ord != Ordering::Greater,
            Op::Gt => ord == Ordering::Greater,
            Op::Ge => ord != Ordering::Less,
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
enum Operand {
    Number(f64),
    /// Lower-cased text, and whether it holds wildcards worth matching.
    Text(String, bool),
    Bool(bool),
    Error(CellError),
    /// `""`, `"="` or `"<>"`: blank cells (or not).
    Blank,
}

/// A parsed criterion such as `">=10"`, `"apple*"` or `"<>"`.
#[derive(Clone, Debug, PartialEq)]
pub struct Criteria {
    op: Op,
    operand: Operand,
}

impl Criteria {
    /// Read a criterion from the value given to COUNTIF and friends.
    pub fn parse(value: &Value, options: &FormatOptions) -> Criteria {
        match value {
            Value::Number(n) => Criteria {
                op: Op::Eq,
                operand: Operand::Number(*n),
            },
            // A reference to an empty cell means 0.
            Value::Empty => Criteria {
                op: Op::Eq,
                operand: Operand::Number(0.0),
            },
            Value::Bool(b) => Criteria {
                op: Op::Eq,
                operand: Operand::Bool(*b),
            },
            Value::Error(e) => Criteria {
                op: Op::Eq,
                operand: Operand::Error(*e),
            },
            Value::Text(t) => Self::parse_text(t, options),
        }
    }

    fn parse_text(text: &str, options: &FormatOptions) -> Criteria {
        let (op, rest) = if let Some(r) = text.strip_prefix("<=") {
            (Op::Le, r)
        } else if let Some(r) = text.strip_prefix(">=") {
            (Op::Ge, r)
        } else if let Some(r) = text.strip_prefix("<>") {
            (Op::Ne, r)
        } else if let Some(r) = text.strip_prefix('<') {
            (Op::Lt, r)
        } else if let Some(r) = text.strip_prefix('>') {
            (Op::Gt, r)
        } else if let Some(r) = text.strip_prefix('=') {
            (Op::Eq, r)
        } else {
            (Op::Eq, text)
        };
        let operand = if rest.is_empty() {
            if matches!(op, Op::Eq | Op::Ne) {
                Operand::Blank
            } else {
                Operand::Text(String::new(), false)
            }
        } else if let Some(n) = parse_number(rest) {
            Operand::Number(n)
        } else if let Some((n, _)) = parse_date_time(rest, options.date_order, options.date1904) {
            Operand::Number(n)
        } else if rest.eq_ignore_ascii_case("TRUE") {
            Operand::Bool(true)
        } else if rest.eq_ignore_ascii_case("FALSE") {
            Operand::Bool(false)
        } else if let Some(e) = CellError::parse(rest) {
            Operand::Error(e)
        } else {
            let wild = matches!(op, Op::Eq | Op::Ne) && rest.contains(['*', '?', '~']);
            Operand::Text(rest.to_lowercase(), wild)
        };
        Criteria { op, operand }
    }

    /// Whether a cell's value meets the criterion.
    pub fn matches(&self, value: &Value) -> bool {
        let ne = self.op == Op::Ne;
        match &self.operand {
            Operand::Blank => {
                let blank = matches!(value, Value::Empty) || matches!(value, Value::Text(t) if t.is_empty());
                blank != ne
            }
            Operand::Number(n) => match value {
                Value::Number(x) => self.op.test(crate::value::compare(&Value::Number(*x), &Value::Number(*n))),
                // Text that reads as the same number counts as equal.
                Value::Text(t) => match self.op {
                    Op::Eq => parse_number(t).is_some_and(|x| crate::value::round15(x) == crate::value::round15(*n)),
                    Op::Ne => !parse_number(t).is_some_and(|x| crate::value::round15(x) == crate::value::round15(*n)),
                    _ => false,
                },
                _ => ne,
            },
            Operand::Text(pattern, wild) => match value {
                Value::Text(t) => {
                    if matches!(self.op, Op::Eq | Op::Ne) {
                        let hit = if *wild {
                            wildcard_match(pattern, t)
                        } else {
                            compare_text(t, pattern) == Ordering::Equal
                        };
                        hit != ne
                    } else {
                        self.op.test(compare_text(t, pattern))
                    }
                }
                _ => ne,
            },
            Operand::Bool(b) => match value {
                Value::Bool(x) => self.op.test(x.cmp(b)),
                _ => ne,
            },
            Operand::Error(e) => match value {
                Value::Error(x) => (x == e) != ne,
                _ => ne,
            },
        }
    }
}

/// Match text against a pattern with `*` (any run), `?` (one character)
/// and `~` (take the next character literally), ignoring case.
pub fn wildcard_match(pattern: &str, text: &str) -> bool {
    #[derive(Clone, Copy, PartialEq)]
    enum P {
        Star,
        One,
        Ch(char),
    }
    let mut pat = Vec::new();
    let mut chars = pattern.chars().flat_map(char::to_lowercase).peekable();
    while let Some(c) = chars.next() {
        match c {
            '~' => match chars.next() {
                Some(n) => pat.push(P::Ch(n)),
                None => pat.push(P::Ch('~')),
            },
            '*' => {
                if pat.last() != Some(&P::Star) {
                    pat.push(P::Star)
                }
            }
            '?' => pat.push(P::One),
            c => pat.push(P::Ch(c)),
        }
    }
    let text: Vec<char> = text.chars().flat_map(char::to_lowercase).collect();
    // Greedy matching with backtracking to the last star.
    let (mut p, mut t) = (0usize, 0usize);
    let mut star: Option<(usize, usize)> = None;
    while t < text.len() {
        match pat.get(p) {
            Some(P::Star) => {
                star = Some((p, t));
                p += 1;
            }
            Some(P::One) => {
                p += 1;
                t += 1;
            }
            Some(P::Ch(c)) if *c == text[t] => {
                p += 1;
                t += 1;
            }
            _ => match star {
                Some((sp, st)) => {
                    p = sp + 1;
                    t = st + 1;
                    star = Some((sp, st + 1));
                }
                None => return false,
            },
        }
    }
    pat[p..].iter().all(|x| *x == P::Star)
}

/// Whether text holds a wildcard that changes matching.
pub fn has_wildcards(text: &str) -> bool {
    text.contains(['*', '?', '~'])
}

/// A criteria range as a grid of values. Whole columns and rows are clipped
/// to the part of the sheet in use.
pub(crate) fn criteria_range(ev: &mut Evaluator, expr: &Expr) -> Result<Array, CellError> {
    let v = ev.eval(expr);
    match &v {
        Eval::Value(Value::Error(e)) => return Err(*e),
        Eval::Ref(areas) if areas.len() != 1 => return Err(CellError::Value),
        _ => {}
    }
    Ok(ev.to_array(v))
}

/// The range that SUMIF/AVERAGEIF add up: Excel takes the top-left cell of
/// the given range and extends it to the criteria range's shape.
pub(crate) fn sized_like(ev: &mut Evaluator, expr: &Expr, rows: usize, cols: usize) -> Result<Array, CellError> {
    match ev.eval(expr) {
        Eval::Ref(areas) => {
            let Some(first) = areas.first().copied() else {
                return Err(CellError::Ref);
            };
            let start = first.range.start;
            let end = start
                .offset(rows as i64 - 1, cols as i64 - 1)
                .unwrap_or(CellRef::new(crate::refs::MAX_ROWS - 1, crate::refs::MAX_COLS - 1));
            let area = SheetArea {
                sheet: first.sheet,
                range: Range::new(start, end),
            };
            Ok(ev.to_array(Eval::Ref(vec![area])))
        }
        Eval::Value(Value::Error(e)) => Err(e),
        other => Ok(ev.to_array(other)),
    }
}

/// For the *IFS functions: which cells of the first range meet every
/// (range, criterion) pair. All ranges must have the same shape.
pub(crate) fn ifs_mask(ev: &mut Evaluator, pairs: &[Expr]) -> Result<(usize, usize, Vec<bool>), CellError> {
    if pairs.is_empty() || pairs.len() % 2 != 0 {
        return Err(CellError::Value);
    }
    let options = ev.options();
    let mut shape: Option<(usize, usize)> = None;
    let mut mask: Vec<bool> = Vec::new();
    for pair in pairs.chunks(2) {
        let range = criteria_range(ev, &pair[0])?;
        let crit_value = ev.eval_scalar(&pair[1]);
        let criteria = Criteria::parse(&crit_value, &options);
        match shape {
            None => {
                shape = Some((range.rows, range.cols));
                mask = range.data.iter().map(|v| criteria.matches(v)).collect();
            }
            Some((rows, cols)) => {
                // Clipped whole columns can differ in length; align them
                // to the first range, treating missing cells as blank.
                if range.cols != cols {
                    return Err(CellError::Value);
                }
                for r in 0..rows {
                    for c in 0..cols {
                        let i = r * cols + c;
                        if mask[i] {
                            let v = if r < range.rows { range.get(r, c).clone() } else { Value::Empty };
                            mask[i] = criteria.matches(&v);
                        }
                    }
                }
            }
        }
    }
    let (rows, cols) = shape.unwrap_or((0, 0));
    Ok((rows, cols, mask))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn c(text: &str) -> Criteria {
        Criteria::parse(&Value::text(text), &FormatOptions::default())
    }

    #[test]
    fn wildcards() {
        assert!(wildcard_match("a*", "Apple"));
        assert!(wildcard_match("*ple", "apple"));
        assert!(wildcard_match("a?ple", "apple"));
        assert!(!wildcard_match("a?ple", "aple"));
        assert!(wildcard_match("*", ""));
        assert!(wildcard_match("a*b*c", "aXXbYYc"));
        assert!(!wildcard_match("a*b*c", "aXXbYY"));
        assert!(wildcard_match("~*", "*"));
        assert!(!wildcard_match("~*", "x"));
        assert!(wildcard_match("what~?", "What?"));
    }

    #[test]
    fn criteria_rules() {
        let n = Value::Number;
        assert!(c(">5").matches(&n(6.0)));
        assert!(!c(">5").matches(&n(5.0)));
        assert!(!c(">5").matches(&Value::text("6")));
        assert!(c("5").matches(&Value::text("5")));
        assert!(c("<>5").matches(&Value::Empty));
        assert!(c("<>5").matches(&Value::text("x")));
        assert!(c("").matches(&Value::Empty));
        assert!(c("").matches(&Value::text("")));
        assert!(!c("").matches(&n(0.0)));
        assert!(c("<>").matches(&n(0.0)));
        assert!(!c("<>").matches(&Value::Empty));
        assert!(c("app*").matches(&Value::text("APPLE")));
        assert!(!c("app*").matches(&n(1.0)));
        assert!(c("<>app*").matches(&Value::text("pear")));
        assert!(c("<b").matches(&Value::text("Apple")));
        assert!(c("TRUE").matches(&Value::Bool(true)));
        assert!(c("#N/A").matches(&Value::Error(CellError::NA)));
        // Dates in criteria follow the locale's order (day first here).
        assert!(c(">=1/1/2026").matches(&n(46_023.0)));
        assert!(!c(">=2/1/2026").matches(&n(46_023.0)));
        let empty_cell = Criteria::parse(&Value::Empty, &FormatOptions::default());
        assert!(empty_cell.matches(&n(0.0)));
    }
}
