//! IF, IFS, IFERROR, AND, OR and the other logical functions.

use super::super::ast::Expr;
use super::super::eval::{Array, Eval, Evaluator};
use super::{arity, lift};
use crate::value::{compare, CellError, Value};

pub fn call(ev: &mut Evaluator, name: &str, args: &[Expr]) -> Option<Eval> {
    Some(match name {
        "IF" => if_(ev, args),
        "IFS" => ifs(ev, args),
        "IFERROR" => if_error(ev, args, false),
        "IFNA" => if_error(ev, args, true),
        "AND" => and_or(ev, args, Junction::And),
        "OR" => and_or(ev, args, Junction::Or),
        "XOR" => and_or(ev, args, Junction::Xor),
        "NOT" => {
            if let Some(e) = arity(args, 1, 1) {
                return Some(e);
            }
            lift(ev, args, &mut |v| match v[0].to_bool() {
                Ok(b) => Value::Bool(!b),
                Err(e) => Value::Error(e),
            })
        }
        "TRUE" => arity(args, 0, 0).unwrap_or(Eval::bool(true)),
        "FALSE" => arity(args, 0, 0).unwrap_or(Eval::bool(false)),
        "SWITCH" => switch(ev, args),
        _ => return None,
    })
}

fn if_(ev: &mut Evaluator, args: &[Expr]) -> Eval {
    if let Some(e) = arity(args, 1, 3) {
        return e;
    }
    let cond = ev.eval(&args[0]);
    let else_value = |ev: &mut Evaluator| match args.get(2) {
        Some(e) => ev.eval(e),
        None => Eval::bool(false),
    };
    if ev.is_multi(&cond) {
        // An array condition chooses element by element.
        let cond = ev.to_array(cond);
        let then = match args.get(1) {
            Some(e) => {
                let v = ev.eval(e);
                ev.to_array(v)
            }
            None => Array::new(1, 1, vec![Value::Bool(true)]),
        };
        let otherwise = else_value(ev);
        let otherwise = ev.to_array(otherwise);
        let rows = cond.rows.max(then.rows).max(otherwise.rows);
        let cols = cond.cols.max(then.cols).max(otherwise.cols);
        let mut data = Vec::with_capacity(rows * cols);
        for r in 0..rows {
            for c in 0..cols {
                data.push(match cond.broadcast(r, c).to_bool() {
                    Ok(true) => then.broadcast(r, c),
                    Ok(false) => otherwise.broadcast(r, c),
                    Err(e) => Value::Error(e),
                });
            }
        }
        return Eval::Array(Array::new(rows, cols, data));
    }
    match ev.scalar(cond).to_bool() {
        Ok(true) => match args.get(1) {
            Some(e) => ev.eval(e),
            None => Eval::bool(true),
        },
        Ok(false) => else_value(ev),
        Err(e) => Eval::error(e),
    }
}

fn ifs(ev: &mut Evaluator, args: &[Expr]) -> Eval {
    if args.len() < 2 || args.len() % 2 != 0 {
        return Eval::error(CellError::Value);
    }
    for pair in args.chunks(2) {
        match ev.eval_scalar(&pair[0]).to_bool() {
            Ok(true) => return ev.eval(&pair[1]),
            Ok(false) => {}
            Err(e) => return Eval::error(e),
        }
    }
    Eval::error(CellError::NA)
}

fn if_error(ev: &mut Evaluator, args: &[Expr], only_na: bool) -> Eval {
    if let Some(e) = arity(args, 2, 2) {
        return e;
    }
    let caught = |v: &Value| match v {
        Value::Error(CellError::NA) => true,
        Value::Error(_) => !only_na,
        _ => false,
    };
    let v = ev.eval(&args[0]);
    if ev.is_multi(&v) {
        let arr = ev.to_array(v);
        if !arr.data.iter().any(caught) {
            return Eval::Array(arr);
        }
        let alt = ev.eval(&args[1]);
        let alt = ev.to_array(alt);
        let mut out = arr.clone();
        for r in 0..arr.rows {
            for c in 0..arr.cols {
                if caught(arr.get(r, c)) {
                    out.data[r * arr.cols + c] = alt.broadcast(r, c);
                }
            }
        }
        return Eval::Array(out);
    }
    // Keep a reference result as a reference (IFERROR(A1,0) in SUM).
    let probe = match &v {
        Eval::Ref(_) => {
            let copy = v.clone();
            ev.scalar(copy)
        }
        Eval::Value(x) => x.clone(),
        Eval::Array(a) => a.data.first().cloned().unwrap_or(Value::Empty),
    };
    if caught(&probe) {
        ev.eval(&args[1])
    } else {
        match v {
            Eval::Value(Value::Empty) => Eval::Value(Value::Empty),
            other => other,
        }
    }
}

#[derive(Clone, Copy, PartialEq)]
enum Junction {
    And,
    Or,
    Xor,
}

fn and_or(ev: &mut Evaluator, args: &[Expr], kind: Junction) -> Eval {
    if args.is_empty() {
        return Eval::error(CellError::Value);
    }
    let mut seen = false;
    let mut all = true;
    let mut any = false;
    let mut odd = false;
    for arg in args {
        if matches!(arg, Expr::Missing) {
            continue;
        }
        for (value, from_ref) in ev.flatten(arg) {
            let b = match value {
                Value::Bool(b) => b,
                Value::Number(n) => n != 0.0,
                Value::Error(e) => return Eval::error(e),
                Value::Empty => continue,
                Value::Text(t) => {
                    if from_ref {
                        continue;
                    }
                    // TRUE/FALSE, or a number in text ("1").
                    match Value::Text(t.clone()).to_bool() {
                        Ok(b) => b,
                        Err(e) => match Value::Text(t).to_number() {
                            Ok(n) => n != 0.0,
                            Err(_) => return Eval::error(e),
                        },
                    }
                }
            };
            seen = true;
            all &= b;
            any |= b;
            odd ^= b;
        }
    }
    if !seen {
        return Eval::error(CellError::Value);
    }
    Eval::bool(match kind {
        Junction::And => all,
        Junction::Or => any,
        Junction::Xor => odd,
    })
}

fn switch(ev: &mut Evaluator, args: &[Expr]) -> Eval {
    if args.len() < 3 {
        return Eval::error(CellError::Value);
    }
    let subject = ev.eval_scalar(&args[0]);
    if let Value::Error(e) = subject {
        return Eval::error(e);
    }
    let rest = &args[1..];
    let pairs = rest.len() / 2;
    for i in 0..pairs {
        let candidate = ev.eval_scalar(&rest[2 * i]);
        if let Value::Error(e) = candidate {
            return Eval::error(e);
        }
        let same_kind = std::mem::discriminant(&candidate) == std::mem::discriminant(&subject)
            || matches!((&candidate, &subject), (Value::Empty, _) | (_, Value::Empty));
        if same_kind && compare(&candidate, &subject) == std::cmp::Ordering::Equal {
            return ev.eval(&rest[2 * i + 1]);
        }
    }
    if rest.len() % 2 == 1 {
        ev.eval(&rest[rest.len() - 1])
    } else {
        Eval::error(CellError::NA)
    }
}
