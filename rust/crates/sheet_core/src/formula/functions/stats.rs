//! Counting, averages, extremes, ranks, percentiles, spread, correlation
//! and the conditional (*IF and *IFS) forms.

use std::cmp::Ordering;

use super::super::ast::Expr;
use super::super::eval::{Eval, Evaluator};
use super::criteria::{criteria_range, ifs_mask, sized_like, Criteria};
use super::{arity, omitted, tri};
use crate::value::{parse_number, CellError, Value};

pub fn call(ev: &mut Evaluator, name: &str, args: &[Expr]) -> Option<Eval> {
    Some(match name {
        "COUNT" => count(ev, args),
        "COUNTA" => count_a(ev, args),
        "COUNTBLANK" => count_blank(ev, args),
        "COUNTIF" => count_if(ev, args),
        "COUNTIFS" => match ifs_mask(ev, args) {
            Ok((_, _, mask)) => Eval::number(mask.iter().filter(|m| **m).count() as f64),
            Err(e) => Eval::error(e),
        },
        "AVERAGE" => with_numbers(ev, args, false, mean),
        "AVERAGEA" => with_numbers(ev, args, true, mean),
        "AVERAGEIF" => average_if(ev, args),
        "AVERAGEIFS" => conditional(ev, args, Cond::Average),
        "MAX" => with_numbers(ev, args, false, |n| Ok(max(n))),
        "MAXA" => with_numbers(ev, args, true, |n| Ok(max(n))),
        "MAXIFS" => conditional(ev, args, Cond::Max),
        "MIN" => with_numbers(ev, args, false, |n| Ok(min(n))),
        "MINA" => with_numbers(ev, args, true, |n| Ok(min(n))),
        "MINIFS" => conditional(ev, args, Cond::Min),
        "MEDIAN" => with_numbers(ev, args, false, median),
        "MODE" | "MODE.SNGL" => with_numbers(ev, args, false, mode),
        "LARGE" => kth(ev, args, true),
        "SMALL" => kth(ev, args, false),
        "RANK" | "RANK.EQ" => rank(ev, args, false),
        "RANK.AVG" => rank(ev, args, true),
        "PERCENTILE" | "PERCENTILE.INC" => percentile_fn(ev, args, false, 1.0),
        "PERCENTILE.EXC" => percentile_fn(ev, args, true, 1.0),
        "QUARTILE" | "QUARTILE.INC" => percentile_fn(ev, args, false, 4.0),
        "QUARTILE.EXC" => percentile_fn(ev, args, true, 4.0),
        "STDEV" | "STDEV.S" => with_numbers(ev, args, false, |n| variance(n, true).map(f64::sqrt)),
        "STDEVA" => with_numbers(ev, args, true, |n| variance(n, true).map(f64::sqrt)),
        "STDEVP" | "STDEV.P" => with_numbers(ev, args, false, |n| variance(n, false).map(f64::sqrt)),
        "STDEVPA" => with_numbers(ev, args, true, |n| variance(n, false).map(f64::sqrt)),
        "VAR" | "VAR.S" => with_numbers(ev, args, false, |n| variance(n, true)),
        "VARA" => with_numbers(ev, args, true, |n| variance(n, true)),
        "VARP" | "VAR.P" => with_numbers(ev, args, false, |n| variance(n, false)),
        "VARPA" => with_numbers(ev, args, true, |n| variance(n, false)),
        "AVEDEV" => with_numbers(ev, args, false, |n| {
            if n.is_empty() {
                return Err(CellError::Num);
            }
            let m = mean(n)?;
            Ok(n.iter().map(|x| (x - m).abs()).sum::<f64>() / n.len() as f64)
        }),
        "DEVSQ" => with_numbers(ev, args, false, |n| {
            if n.is_empty() {
                return Err(CellError::Num);
            }
            let m = mean(n)?;
            Ok(n.iter().map(|x| (x - m) * (x - m)).sum())
        }),
        "GEOMEAN" => with_numbers(ev, args, false, |n| {
            if n.is_empty() || n.iter().any(|x| *x <= 0.0) {
                return Err(CellError::Num);
            }
            Ok((n.iter().map(|x| x.ln()).sum::<f64>() / n.len() as f64).exp())
        }),
        "HARMEAN" => with_numbers(ev, args, false, |n| {
            if n.is_empty() || n.iter().any(|x| *x <= 0.0) {
                return Err(CellError::Num);
            }
            Ok(n.len() as f64 / n.iter().map(|x| 1.0 / x).sum::<f64>())
        }),
        "CORREL" | "PEARSON" => paired(ev, args, |x, y| correl(x, y)),
        "COVAR" | "COVARIANCE.P" => paired(ev, args, |x, y| covariance(x, y, false)),
        "COVARIANCE.S" => paired(ev, args, |x, y| covariance(x, y, true)),
        "SLOPE" => paired(ev, args, |y, x| regression(x, y).map(|(s, _)| s)),
        "INTERCEPT" => paired(ev, args, |y, x| regression(x, y).map(|(_, i)| i)),
        "RSQ" => paired(ev, args, |y, x| correl(x, y).map(|r| r * r)),
        "FORECAST" | "FORECAST.LINEAR" => forecast(ev, args),
        "PERCENTRANK" | "PERCENTRANK.INC" => percent_rank(ev, args),
        _ => return None,
    })
}

/// Apply a function to an aggregate's numbers (SUM rules, or the *A rules).
fn with_numbers(ev: &mut Evaluator, args: &[Expr], a_rules: bool, f: impl Fn(&[f64]) -> Result<f64, CellError>) -> Eval {
    if args.is_empty() {
        return Eval::error(CellError::Value);
    }
    let nums = if a_rules { ev.numbers_a(args) } else { ev.numbers(args) };
    match nums {
        Ok(n) => f(&n).into(),
        Err(e) => Eval::error(e),
    }
}

pub(crate) fn mean(n: &[f64]) -> Result<f64, CellError> {
    if n.is_empty() {
        return Err(CellError::Div0);
    }
    Ok(n.iter().sum::<f64>() / n.len() as f64)
}

fn max(n: &[f64]) -> f64 {
    n.iter().copied().fold(None, |acc: Option<f64>, x| Some(acc.map_or(x, |a| a.max(x)))).unwrap_or(0.0)
}

fn min(n: &[f64]) -> f64 {
    n.iter().copied().fold(None, |acc: Option<f64>, x| Some(acc.map_or(x, |a| a.min(x)))).unwrap_or(0.0)
}

pub(crate) fn variance(n: &[f64], sample: bool) -> Result<f64, CellError> {
    let k = n.len();
    if k == 0 || (sample && k < 2) {
        return Err(CellError::Div0);
    }
    let m = n.iter().sum::<f64>() / k as f64;
    let ss: f64 = n.iter().map(|x| (x - m) * (x - m)).sum();
    Ok(ss / if sample { (k - 1) as f64 } else { k as f64 })
}

fn sorted(n: &[f64]) -> Vec<f64> {
    let mut v = n.to_vec();
    v.sort_by(|a, b| a.partial_cmp(b).unwrap_or(Ordering::Equal));
    v
}

pub(crate) fn median(n: &[f64]) -> Result<f64, CellError> {
    if n.is_empty() {
        return Err(CellError::Num);
    }
    let v = sorted(n);
    let mid = v.len() / 2;
    Ok(if v.len() % 2 == 1 { v[mid] } else { (v[mid - 1] + v[mid]) / 2.0 })
}

pub(crate) fn mode(n: &[f64]) -> Result<f64, CellError> {
    let mut best: Option<(f64, usize)> = None;
    for (i, x) in n.iter().enumerate() {
        if n[..i].contains(x) {
            continue;
        }
        let c = n[i..].iter().filter(|y| *y == x).count();
        if c > 1 && best.is_none_or(|(_, bc)| c > bc) {
            best = Some((*x, c));
        }
    }
    best.map(|(x, _)| x).ok_or(CellError::NA)
}

/// The k-th largest or smallest (k is 1-based, rounded up).
pub(crate) fn kth_value(n: &[f64], k: f64, largest: bool) -> Result<f64, CellError> {
    let k = k.ceil();
    if k < 1.0 || k > n.len() as f64 {
        return Err(CellError::Num);
    }
    let v = sorted(n);
    let i = k as usize - 1;
    Ok(if largest { v[v.len() - 1 - i] } else { v[i] })
}

/// PERCENTILE.INC (exclusive = false) or PERCENTILE.EXC.
pub(crate) fn percentile(n: &[f64], p: f64, exclusive: bool) -> Result<f64, CellError> {
    if n.is_empty() || !(0.0..=1.0).contains(&p) {
        return Err(CellError::Num);
    }
    let v = sorted(n);
    let len = v.len() as f64;
    let rank = if exclusive {
        let r = p * (len + 1.0) - 1.0;
        if r < 0.0 || r > len - 1.0 {
            return Err(CellError::Num);
        }
        r
    } else {
        p * (len - 1.0)
    };
    let lo = rank.floor() as usize;
    let frac = rank - lo as f64;
    if lo + 1 >= v.len() {
        return Ok(v[v.len() - 1]);
    }
    Ok(v[lo] + frac * (v[lo + 1] - v[lo]))
}

/// The SUBTOTAL and AGGREGATE functions by number, over cell values.
pub(crate) fn summarize(func: u32, values: &[Value], k: Option<f64>) -> Result<f64, CellError> {
    let nums: Vec<f64> = values
        .iter()
        .filter_map(|v| match v {
            Value::Number(n) => Some(*n),
            _ => None,
        })
        .collect();
    match func {
        1 => mean(&nums),
        2 => Ok(nums.len() as f64),
        3 => Ok(values.iter().filter(|v| !v.is_empty()).count() as f64),
        4 => Ok(max(&nums)),
        5 => Ok(min(&nums)),
        6 => Ok(if nums.is_empty() { 0.0 } else { nums.iter().product() }),
        7 => variance(&nums, true).map(f64::sqrt),
        8 => variance(&nums, false).map(f64::sqrt),
        9 => Ok(nums.iter().sum()),
        10 => variance(&nums, true),
        11 => variance(&nums, false),
        12 => median(&nums),
        13 => mode(&nums),
        14 => kth_value(&nums, k.unwrap_or(1.0), true),
        15 => kth_value(&nums, k.unwrap_or(1.0), false),
        16 => percentile(&nums, k.unwrap_or(0.0), false),
        17 => quartile(&nums, k.unwrap_or(0.0), false),
        18 => percentile(&nums, k.unwrap_or(0.0), true),
        19 => quartile(&nums, k.unwrap_or(0.0), true),
        _ => Err(CellError::Value),
    }
}

fn quartile(n: &[f64], q: f64, exclusive: bool) -> Result<f64, CellError> {
    let q = q.trunc();
    let ok = if exclusive { (1.0..=3.0).contains(&q) } else { (0.0..=4.0).contains(&q) };
    if !ok {
        return Err(CellError::Num);
    }
    percentile(n, q / 4.0, exclusive)
}

fn count(ev: &mut Evaluator, args: &[Expr]) -> Eval {
    let mut total = 0usize;
    let options = ev.options();
    for arg in args {
        if matches!(arg, Expr::Missing) {
            continue;
        }
        for (value, from_ref) in ev.flatten(arg) {
            let counted = match value {
                Value::Number(_) => true,
                Value::Bool(_) => !from_ref,
                Value::Text(t) if !from_ref => {
                    parse_number(&t).is_some()
                        || crate::input::parse_date_time(&t, options.date_order, options.date1904).is_some()
                }
                _ => false,
            };
            total += counted as usize;
        }
    }
    Eval::number(total as f64)
}

fn count_a(ev: &mut Evaluator, args: &[Expr]) -> Eval {
    let mut total = 0usize;
    for arg in args {
        if matches!(arg, Expr::Missing) {
            continue;
        }
        for (value, from_ref) in ev.flatten(arg) {
            if !from_ref || !value.is_empty() {
                total += 1;
            }
        }
    }
    Eval::number(total as f64)
}

fn count_blank(ev: &mut Evaluator, args: &[Expr]) -> Eval {
    if let Some(e) = arity(args, 1, 1) {
        return e;
    }
    let range = tri!(criteria_range(ev, &args[0]));
    let n = range
        .data
        .iter()
        .filter(|v| matches!(v, Value::Empty) || matches!(v, Value::Text(t) if t.is_empty()))
        .count();
    Eval::number(n as f64)
}

fn count_if(ev: &mut Evaluator, args: &[Expr]) -> Eval {
    if let Some(e) = arity(args, 2, 2) {
        return e;
    }
    let range = tri!(criteria_range(ev, &args[0]));
    let crit = ev.eval_scalar(&args[1]);
    let criteria = Criteria::parse(&crit, &ev.options());
    Eval::number(range.data.iter().filter(|v| criteria.matches(v)).count() as f64)
}

fn average_if(ev: &mut Evaluator, args: &[Expr]) -> Eval {
    if let Some(e) = arity(args, 2, 3) {
        return e;
    }
    let range = tri!(criteria_range(ev, &args[0]));
    let crit = ev.eval_scalar(&args[1]);
    let criteria = Criteria::parse(&crit, &ev.options());
    let avg_range = if omitted(args, 2) {
        range.clone()
    } else {
        tri!(sized_like(ev, &args[2], range.rows, range.cols))
    };
    let mut nums = Vec::new();
    for (i, v) in range.data.iter().enumerate() {
        if criteria.matches(v) {
            match avg_range.data.get(i) {
                Some(Value::Number(n)) => nums.push(*n),
                Some(Value::Error(e)) => return Eval::error(*e),
                _ => {}
            }
        }
    }
    mean(&nums).into()
}

#[derive(Clone, Copy)]
enum Cond {
    Average,
    Max,
    Min,
}

fn conditional(ev: &mut Evaluator, args: &[Expr], kind: Cond) -> Eval {
    if args.len() < 3 || args.len() % 2 == 0 {
        return Eval::error(CellError::Value);
    }
    let (rows, cols, mask) = tri!(ifs_mask(ev, &args[1..]));
    let target = tri!(criteria_range(ev, &args[0]));
    if target.cols != cols {
        return Eval::error(CellError::Value);
    }
    let mut nums = Vec::new();
    for r in 0..rows.min(target.rows) {
        for c in 0..cols {
            if mask[r * cols + c] {
                match target.get(r, c) {
                    Value::Number(n) => nums.push(*n),
                    Value::Error(e) => return Eval::error(*e),
                    _ => {}
                }
            }
        }
    }
    match kind {
        Cond::Average => mean(&nums).into(),
        Cond::Max => Eval::number(max(&nums)),
        Cond::Min => Eval::number(min(&nums)),
    }
}

fn kth(ev: &mut Evaluator, args: &[Expr], largest: bool) -> Eval {
    if let Some(e) = arity(args, 2, 2) {
        return e;
    }
    let nums = tri!(ev.numbers(&args[..1]));
    let k = tri!(ev.number(args, 1));
    kth_value(&nums, k, largest).into()
}

fn rank(ev: &mut Evaluator, args: &[Expr], average: bool) -> Eval {
    if let Some(e) = arity(args, 2, 3) {
        return e;
    }
    let x = tri!(ev.number(args, 0));
    let nums = tri!(ev.numbers(&args[1..2]));
    let ascending = !omitted(args, 2) && tri!(ev.number(args, 2)) != 0.0;
    let eq = |a: f64| crate::value::round15(a) == crate::value::round15(x);
    let equal = nums.iter().filter(|n| eq(**n)).count();
    if equal == 0 {
        return Eval::error(CellError::NA);
    }
    let before = nums
        .iter()
        .filter(|n| !eq(**n) && if ascending { **n < x } else { **n > x })
        .count();
    let r = before as f64 + 1.0;
    Eval::number(if average { r + (equal as f64 - 1.0) / 2.0 } else { r })
}

fn percentile_fn(ev: &mut Evaluator, args: &[Expr], exclusive: bool, divisor: f64) -> Eval {
    if let Some(e) = arity(args, 2, 2) {
        return e;
    }
    let nums = tri!(ev.numbers(&args[..1]));
    let p = tri!(ev.number(args, 1));
    if divisor == 1.0 {
        percentile(&nums, p, exclusive).into()
    } else {
        quartile(&nums, p, exclusive).into()
    }
}

/// Pairs of numbers from two equally sized arguments (pairs where either
/// side is not a number are skipped).
fn pairs(ev: &mut Evaluator, args: &[Expr], i: usize, j: usize) -> Result<(Vec<f64>, Vec<f64>), CellError> {
    let a = super::array_arg(ev, args, i);
    let b = super::array_arg(ev, args, j);
    if a.data.len() != b.data.len() {
        return Err(CellError::NA);
    }
    let (mut xs, mut ys) = (Vec::new(), Vec::new());
    for (p, q) in a.data.iter().zip(&b.data) {
        match (p, q) {
            (Value::Error(e), _) | (_, Value::Error(e)) => return Err(*e),
            (Value::Number(x), Value::Number(y)) => {
                xs.push(*x);
                ys.push(*y);
            }
            _ => {}
        }
    }
    Ok((xs, ys))
}

fn paired(ev: &mut Evaluator, args: &[Expr], f: impl Fn(&[f64], &[f64]) -> Result<f64, CellError>) -> Eval {
    if let Some(e) = arity(args, 2, 2) {
        return e;
    }
    match pairs(ev, args, 0, 1) {
        Ok((x, y)) => f(&x, &y).into(),
        Err(e) => Eval::error(e),
    }
}

fn covariance(x: &[f64], y: &[f64], sample: bool) -> Result<f64, CellError> {
    let n = x.len();
    if n == 0 || (sample && n < 2) {
        return Err(CellError::Div0);
    }
    let (mx, my) = (mean(x)?, mean(y)?);
    let s: f64 = x.iter().zip(y).map(|(a, b)| (a - mx) * (b - my)).sum();
    Ok(s / if sample { (n - 1) as f64 } else { n as f64 })
}

fn correl(x: &[f64], y: &[f64]) -> Result<f64, CellError> {
    if x.len() < 2 {
        return Err(CellError::Div0);
    }
    let (mx, my) = (mean(x)?, mean(y)?);
    let sxy: f64 = x.iter().zip(y).map(|(a, b)| (a - mx) * (b - my)).sum();
    let sxx: f64 = x.iter().map(|a| (a - mx) * (a - mx)).sum();
    let syy: f64 = y.iter().map(|b| (b - my) * (b - my)).sum();
    if sxx == 0.0 || syy == 0.0 {
        return Err(CellError::Div0);
    }
    Ok(sxy / (sxx * syy).sqrt())
}

/// Least-squares slope and intercept of y on x.
fn regression(x: &[f64], y: &[f64]) -> Result<(f64, f64), CellError> {
    if x.is_empty() {
        return Err(CellError::Div0);
    }
    let (mx, my) = (mean(x)?, mean(y)?);
    let sxy: f64 = x.iter().zip(y).map(|(a, b)| (a - mx) * (b - my)).sum();
    let sxx: f64 = x.iter().map(|a| (a - mx) * (a - mx)).sum();
    if sxx == 0.0 {
        return Err(CellError::Div0);
    }
    let slope = sxy / sxx;
    Ok((slope, my - slope * mx))
}

fn forecast(ev: &mut Evaluator, args: &[Expr]) -> Eval {
    if let Some(e) = arity(args, 3, 3) {
        return e;
    }
    let x = tri!(ev.number(args, 0));
    let (ys, xs) = tri!(pairs(ev, args, 1, 2));
    let (slope, intercept) = tri!(regression(&xs, &ys));
    Eval::number(intercept + slope * x)
}

fn percent_rank(ev: &mut Evaluator, args: &[Expr]) -> Eval {
    if let Some(e) = arity(args, 2, 3) {
        return e;
    }
    let nums = sorted(&tri!(ev.numbers(&args[..1])));
    let x = tri!(ev.number(args, 1));
    let sig = if omitted(args, 2) { 3.0 } else { tri!(ev.number(args, 2)).trunc() };
    if sig < 1.0 || nums.is_empty() {
        return Eval::error(CellError::Num);
    }
    let n = nums.len();
    if x < nums[0] || x > nums[n - 1] {
        return Eval::error(CellError::NA);
    }
    if n == 1 {
        return Eval::number(1.0);
    }
    let less = |v: f64| nums.iter().filter(|a| **a < v).count() as f64;
    let rank = if nums.contains(&x) {
        less(x) / (n - 1) as f64
    } else {
        let lo = nums.iter().copied().filter(|a| *a < x).fold(f64::MIN, f64::max);
        let hi = nums.iter().copied().filter(|a| *a > x).fold(f64::MAX, f64::min);
        let (rl, rh) = (less(lo) / (n - 1) as f64, less(hi) / (n - 1) as f64);
        rl + (x - lo) / (hi - lo) * (rh - rl)
    };
    let m = 10f64.powi(sig as i32);
    Eval::number((crate::value::round15(rank * m)).floor() / m)
}
