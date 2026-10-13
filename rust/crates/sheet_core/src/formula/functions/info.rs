//! IS* functions, N, NA, TYPE, ERROR.TYPE, ISFORMULA and FORMULATEXT.

use super::super::ast::Expr;
use super::super::eval::{Eval, Evaluator};
use super::{arity, lift};
use crate::value::{CellError, Value};

pub fn call(ev: &mut Evaluator, name: &str, args: &[Expr]) -> Option<Eval> {
    let test: fn(&Value) -> Option<bool> = match name {
        "ISBLANK" => |v| Some(v.is_empty()),
        "ISERR" => |v| Some(matches!(v, Value::Error(e) if *e != CellError::NA)),
        "ISERROR" => |v| Some(matches!(v, Value::Error(_))),
        "ISNA" => |v| Some(matches!(v, Value::Error(CellError::NA))),
        "ISLOGICAL" => |v| Some(matches!(v, Value::Bool(_))),
        "ISNUMBER" => |v| Some(matches!(v, Value::Number(_))),
        "ISTEXT" => |v| Some(matches!(v, Value::Text(_))),
        "ISNONTEXT" => |v| Some(!matches!(v, Value::Text(_))),
        "ISEVEN" | "ISODD" => {
            if let Some(e) = arity(args, 1, 1) {
                return Some(e);
            }
            let odd = name == "ISODD";
            return Some(lift(ev, args, &mut |v| match &v[0] {
                Value::Bool(_) => Value::Error(CellError::Value),
                other => match other.to_number() {
                    Ok(n) => Value::Bool(((n.trunc() as i64).rem_euclid(2) == 1) == odd),
                    Err(e) => Value::Error(e),
                },
            }));
        }
        _ => return other(ev, name, args),
    };
    if let Some(e) = arity(args, 1, 1) {
        return Some(e);
    }
    Some(lift(ev, args, &mut |v| Value::Bool(test(&v[0]).unwrap_or(false))))
}

fn other(ev: &mut Evaluator, name: &str, args: &[Expr]) -> Option<Eval> {
    Some(match name {
        "ISREF" => {
            if let Some(e) = arity(args, 1, 1) {
                return Some(e);
            }
            Eval::bool(matches!(ev.eval(&args[0]), Eval::Ref(_)))
        }
        "ISFORMULA" | "FORMULATEXT" => {
            if let Some(e) = arity(args, 1, 1) {
                return Some(e);
            }
            let Some(areas) = ev.reference_arg(args, 0) else {
                return Some(Eval::error(CellError::Value));
            };
            let Some(area) = areas.first() else {
                return Some(Eval::error(CellError::Ref));
            };
            let f = ev.grid.formula_text(area.sheet, area.range.start);
            if name == "ISFORMULA" {
                Eval::bool(f.is_some())
            } else {
                match f {
                    Some(text) => Eval::text(format!("={text}")),
                    None => Eval::error(CellError::NA),
                }
            }
        }
        "N" => {
            if let Some(e) = arity(args, 1, 1) {
                return Some(e);
            }
            match ev.arg(args, 0) {
                Value::Number(n) => Eval::number(n),
                Value::Bool(b) => Eval::number(b as u8 as f64),
                Value::Error(e) => Eval::error(e),
                _ => Eval::number(0.0),
            }
        }
        "NA" => arity(args, 0, 0).unwrap_or(Eval::error(CellError::NA)),
        "TYPE" => {
            if let Some(e) = arity(args, 1, 1) {
                return Some(e);
            }
            let v = ev.eval(&args[0]);
            if ev.is_multi(&v) {
                return Some(Eval::number(64.0));
            }
            Eval::number(match ev.scalar(v) {
                Value::Number(_) | Value::Empty => 1.0,
                Value::Text(_) => 2.0,
                Value::Bool(_) => 4.0,
                Value::Error(_) => 16.0,
            })
        }
        "ERROR.TYPE" => {
            if let Some(e) = arity(args, 1, 1) {
                return Some(e);
            }
            match ev.arg(args, 0) {
                Value::Error(e) => Eval::number(e.type_number()),
                _ => Eval::error(CellError::NA),
            }
        }
        _ => return None,
    })
}
