//! Arithmetic, rounding, trigonometry, SUM and its relatives, SUBTOTAL and
//! AGGREGATE.

use super::super::ast::Expr;
use super::super::eval::{Eval, Evaluator};
use super::criteria::{criteria_range, ifs_mask, sized_like, Criteria};
use super::stats::summarize;
use super::{arity, lift_num1, lift_nums, omitted, tri};
use crate::numfmt::round_half_away;
use crate::value::{round15, CellError, Value};

pub fn call(ev: &mut Evaluator, name: &str, args: &[Expr]) -> Option<Eval> {
    use std::f64::consts::PI;
    Some(match name {
        "SUM" => match ev.numbers(args) {
            Ok(n) => Eval::number(n.iter().sum()),
            Err(e) => Eval::error(e),
        },
        "SUMIF" => sum_if(ev, args),
        "SUMIFS" => sum_ifs(ev, args),
        "SUMPRODUCT" => sum_product(ev, args),
        "SUMSQ" => match ev.numbers(args) {
            Ok(n) => Eval::number(n.iter().map(|x| x * x).sum()),
            Err(e) => Eval::error(e),
        },
        "PRODUCT" => match ev.numbers(args) {
            Ok(n) if n.is_empty() => Eval::number(0.0),
            Ok(n) => Eval::number(n.iter().product()),
            Err(e) => Eval::error(e),
        },
        "SUMX2MY2" => pairs_sum(ev, args, |x, y| x * x - y * y),
        "SUMX2PY2" => pairs_sum(ev, args, |x, y| x * x + y * y),
        "SUMXMY2" => pairs_sum(ev, args, |x, y| (x - y) * (x - y)),
        "ABS" => lift_num1(ev, args, |x| Ok(x.abs())),
        "SIGN" => lift_num1(ev, args, |x| Ok(if x > 0.0 { 1.0 } else if x < 0.0 { -1.0 } else { 0.0 })),
        "INT" => lift_num1(ev, args, |x| Ok(round15(x).floor())),
        "TRUNC" => lift_nums(ev, args, 1, &[0.0], |a| Ok(round_toward_zero(a[0], a[1].trunc() as i32))),
        "ROUND" => lift_nums(ev, args, 2, &[], |a| Ok(round(a[0], a[1]))),
        "ROUNDUP" => lift_nums(ev, args, 2, &[], |a| Ok(round_away(a[0], a[1].trunc() as i32))),
        "ROUNDDOWN" => lift_nums(ev, args, 2, &[], |a| Ok(round_toward_zero(a[0], a[1].trunc() as i32))),
        "MROUND" => lift_nums(ev, args, 2, &[], |a| {
            let (n, m) = (a[0], a[1]);
            if m == 0.0 {
                return Ok(0.0);
            }
            if n * m < 0.0 {
                return Err(CellError::Num);
            }
            Ok(round_half_away(n / m, 0) * m)
        }),
        "CEILING" => lift_nums(ev, args, 1, &[1.0], |a| {
            let (n, s) = (a[0], a[1]);
            if s == 0.0 {
                return Ok(0.0);
            }
            if n > 0.0 && s < 0.0 {
                return Err(CellError::Num);
            }
            Ok(round15(n / s).ceil() * s)
        }),
        "CEILING.MATH" => lift_nums(ev, args, 1, &[1.0, 0.0], |a| {
            let (n, s, mode) = (a[0], a[1].abs(), a[2]);
            if s == 0.0 {
                return Ok(0.0);
            }
            let q = round15(n / s);
            Ok(if n < 0.0 && mode != 0.0 { q.floor() * s } else { q.ceil() * s })
        }),
        "CEILING.PRECISE" | "ISO.CEILING" => lift_nums(ev, args, 1, &[1.0], |a| {
            let s = a[1].abs();
            if s == 0.0 {
                return Ok(0.0);
            }
            Ok(round15(a[0] / s).ceil() * s)
        }),
        "FLOOR" => lift_nums(ev, args, 2, &[], |a| {
            let (n, s) = (a[0], a[1]);
            if s == 0.0 {
                return if n == 0.0 { Ok(0.0) } else { Err(CellError::Div0) };
            }
            if n > 0.0 && s < 0.0 {
                return Err(CellError::Num);
            }
            Ok(round15(n / s).floor() * s)
        }),
        "FLOOR.MATH" => lift_nums(ev, args, 1, &[1.0, 0.0], |a| {
            let (n, s, mode) = (a[0], a[1].abs(), a[2]);
            if s == 0.0 {
                return Ok(0.0);
            }
            let q = round15(n / s);
            Ok(if n < 0.0 && mode != 0.0 { q.ceil() * s } else { q.floor() * s })
        }),
        "FLOOR.PRECISE" => lift_nums(ev, args, 1, &[1.0], |a| {
            let s = a[1].abs();
            if s == 0.0 {
                return Ok(0.0);
            }
            Ok(round15(a[0] / s).floor() * s)
        }),
        "EVEN" => lift_num1(ev, args, |x| {
            let up = (round15(x.abs()) / 2.0).ceil() * 2.0;
            Ok(up.copysign(x))
        }),
        "ODD" => lift_num1(ev, args, |x| {
            let a = round15(x.abs()).ceil();
            let odd = if a % 2.0 == 0.0 { a + 1.0 } else { a };
            Ok(if x < 0.0 { -odd } else { odd })
        }),
        "MOD" => lift_nums(ev, args, 2, &[], |a| modulo(a[0], a[1])),
        "QUOTIENT" => lift_nums(ev, args, 2, &[], |a| {
            if a[1] == 0.0 {
                Err(CellError::Div0)
            } else {
                Ok((a[0] / a[1]).trunc())
            }
        }),
        "POWER" => lift_nums(ev, args, 2, &[], |a| power(a[0], a[1])),
        "SQRT" => lift_num1(ev, args, |x| if x < 0.0 { Err(CellError::Num) } else { Ok(x.sqrt()) }),
        "SQRTPI" => lift_num1(ev, args, |x| if x < 0.0 { Err(CellError::Num) } else { Ok((x * PI).sqrt()) }),
        "EXP" => lift_num1(ev, args, |x| Ok(x.exp())),
        "LN" => lift_num1(ev, args, |x| if x <= 0.0 { Err(CellError::Num) } else { Ok(x.ln()) }),
        "LOG10" => lift_num1(ev, args, |x| if x <= 0.0 { Err(CellError::Num) } else { Ok(x.log10()) }),
        "LOG" => lift_nums(ev, args, 1, &[10.0], |a| {
            let (x, b) = (a[0], a[1]);
            if x <= 0.0 || b <= 0.0 {
                return Err(CellError::Num);
            }
            if b == 1.0 {
                return Err(CellError::Div0);
            }
            Ok(if b == 10.0 { x.log10() } else { x.ln() / b.ln() })
        }),
        "PI" => arity(args, 0, 0).unwrap_or(Eval::number(PI)),
        "SIN" => lift_num1(ev, args, |x| Ok(x.sin())),
        "COS" => lift_num1(ev, args, |x| Ok(x.cos())),
        "TAN" => lift_num1(ev, args, |x| Ok(x.tan())),
        "ASIN" => lift_num1(ev, args, |x| in_unit(x).map(f64::asin)),
        "ACOS" => lift_num1(ev, args, |x| in_unit(x).map(f64::acos)),
        "ATAN" => lift_num1(ev, args, |x| Ok(x.atan())),
        "ATAN2" => lift_nums(ev, args, 2, &[], |a| {
            if a[0] == 0.0 && a[1] == 0.0 {
                Err(CellError::Div0)
            } else {
                // Excel's ATAN2 takes x first.
                Ok(a[1].atan2(a[0]))
            }
        }),
        "SINH" => lift_num1(ev, args, |x| Ok(x.sinh())),
        "COSH" => lift_num1(ev, args, |x| Ok(x.cosh())),
        "TANH" => lift_num1(ev, args, |x| Ok(x.tanh())),
        "ASINH" => lift_num1(ev, args, |x| Ok(x.asinh())),
        "ACOSH" => lift_num1(ev, args, |x| if x < 1.0 { Err(CellError::Num) } else { Ok(x.acosh()) }),
        "ATANH" => lift_num1(ev, args, |x| {
            if x <= -1.0 || x >= 1.0 {
                Err(CellError::Num)
            } else {
                Ok(x.atanh())
            }
        }),
        "COT" => lift_num1(ev, args, |x| if x == 0.0 { Err(CellError::Div0) } else { Ok(1.0 / x.tan()) }),
        "CSC" => lift_num1(ev, args, |x| if x == 0.0 { Err(CellError::Div0) } else { Ok(1.0 / x.sin()) }),
        "SEC" => lift_num1(ev, args, |x| Ok(1.0 / x.cos())),
        "DEGREES" => lift_num1(ev, args, |x| Ok(x.to_degrees())),
        "RADIANS" => lift_num1(ev, args, |x| Ok(x.to_radians())),
        "FACT" => lift_num1(ev, args, |x| factorial(x.trunc())),
        "FACTDOUBLE" => lift_num1(ev, args, |x| {
            let n = x.trunc();
            // Past 300 the result overflows anyway; the loop below would
            // otherwise run n / 2 times for whatever n a cell holds.
            if !(-1.0..=1000.0).contains(&n) {
                return Err(CellError::Num);
            }
            let mut r = 1.0;
            let mut k = n;
            while k > 1.0 {
                r *= k;
                k -= 2.0;
            }
            Ok(r)
        }),
        "COMBIN" => lift_nums(ev, args, 2, &[], |a| combin(a[0].trunc(), a[1].trunc())),
        "COMBINA" => lift_nums(ev, args, 2, &[], |a| {
            let (n, k) = (a[0].trunc(), a[1].trunc());
            if n < 0.0 || k < 0.0 {
                return Err(CellError::Num);
            }
            if n == 0.0 && k == 0.0 {
                return Ok(1.0);
            }
            combin(n + k - 1.0, k)
        }),
        "PERMUT" => lift_nums(ev, args, 2, &[], |a| {
            let (n, k) = (a[0].trunc(), a[1].trunc());
            // n! / (n - k)! is at least k!, which overflows past k = 170,
            // so the product need not run k times for a huge k.
            if n < 0.0 || k < 0.0 || n < k || k > 1000.0 {
                return Err(CellError::Num);
            }
            Ok((0..k as u64).fold(1.0, |acc, i| acc * (n - i as f64)))
        }),
        "GCD" | "LCM" => gcd_lcm(ev, args, name == "GCD"),
        "RAND" => {
            if let Some(e) = arity(args, 0, 0) {
                return Some(e);
            }
            ev.volatile = true;
            Eval::number(ev.grid.random())
        }
        "RANDBETWEEN" => rand_between(ev, args),
        "SUBTOTAL" => subtotal(ev, args),
        "AGGREGATE" => aggregate(ev, args),
        _ => return None,
    })
}

fn rand_between(ev: &mut Evaluator, args: &[Expr]) -> Eval {
    if let Some(e) = arity(args, 2, 2) {
        return e;
    }
    ev.volatile = true;
    let lo = tri!(ev.number(args, 0)).ceil();
    let hi = tri!(ev.number(args, 1)).floor();
    if lo > hi {
        return Eval::error(CellError::Num);
    }
    let r = ev.grid.random();
    Eval::number((lo + (r * (hi - lo + 1.0)).floor()).min(hi))
}

fn in_unit(x: f64) -> Result<f64, CellError> {
    if (-1.0..=1.0).contains(&x) {
        Ok(x)
    } else {
        Err(CellError::Num)
    }
}

/// ROUND: half away from zero on the 15-digit decimal value.
pub(crate) fn round(x: f64, digits: f64) -> f64 {
    let d = digits.trunc().clamp(-308.0, 308.0) as i32;
    if d > 15 {
        return x;
    }
    round_half_away(x, d)
}

fn scale(d: i32) -> f64 {
    10f64.powi(d)
}

/// ROUNDUP: away from zero.
pub(crate) fn round_away(x: f64, d: i32) -> f64 {
    let d = d.clamp(-308, 308);
    let m = scale(d);
    let r = round15(round15(x.abs()) * m).ceil() / m;
    round15(r).copysign(x)
}

/// ROUNDDOWN and TRUNC: toward zero.
pub(crate) fn round_toward_zero(x: f64, d: i32) -> f64 {
    let d = d.clamp(-308, 308);
    let m = scale(d);
    let r = round15(round15(x.abs()) * m).floor() / m;
    if r == 0.0 {
        0.0
    } else {
        round15(r).copysign(x)
    }
}

/// MOD takes the sign of the divisor.
pub(crate) fn modulo(n: f64, d: f64) -> Result<f64, CellError> {
    if d == 0.0 {
        return Err(CellError::Div0);
    }
    let q = (n / d).floor();
    let r = n - d * q;
    // Floating error can leave r equal to d.
    Ok(if round15(r) == round15(d) { 0.0 } else { r })
}

pub(crate) fn power(x: f64, y: f64) -> Result<f64, CellError> {
    if x == 0.0 && y == 0.0 {
        return Err(CellError::Num);
    }
    if x == 0.0 && y < 0.0 {
        return Err(CellError::Div0);
    }
    let r = x.powf(y);
    if r.is_finite() {
        Ok(r)
    } else {
        Err(CellError::Num)
    }
}

pub(crate) fn factorial(n: f64) -> Result<f64, CellError> {
    if n < 0.0 || n > 170.0 {
        return Err(CellError::Num);
    }
    Ok((1..=n as u64).fold(1.0, |acc, k| acc * k as f64))
}

pub(crate) fn combin(n: f64, k: f64) -> Result<f64, CellError> {
    if n < 0.0 || k < 0.0 || n < k {
        return Err(CellError::Num);
    }
    let k = k.min(n - k);
    // With k at most n / 2 the result is at least 2^k, which overflows past
    // k = 1024, so the loop need not run k times for a huge k.
    if k > 1100.0 {
        return Err(CellError::Num);
    }
    let mut r = 1.0;
    for i in 0..k as u64 {
        r = r * (n - i as f64) / (i as f64 + 1.0);
    }
    Ok(r.round())
}

fn gcd_lcm(ev: &mut Evaluator, args: &[Expr], gcd: bool) -> Eval {
    let nums = tri!(ev.numbers(args));
    if nums.is_empty() {
        return Eval::error(CellError::Value);
    }
    let mut acc: Option<u128> = None;
    for n in nums {
        if n < 0.0 || n >= 2f64.powi(53) {
            return Eval::error(CellError::Num);
        }
        let n = n.trunc() as u128;
        acc = Some(match acc {
            None => n,
            Some(a) => {
                if gcd {
                    euclid(a, n)
                } else if a == 0 || n == 0 {
                    0
                } else {
                    a / euclid(a, n) * n
                }
            }
        });
    }
    Eval::number(acc.unwrap_or(0) as f64)
}

fn euclid(mut a: u128, mut b: u128) -> u128 {
    while b != 0 {
        let t = a % b;
        a = b;
        b = t;
    }
    a
}

fn sum_if(ev: &mut Evaluator, args: &[Expr]) -> Eval {
    if let Some(e) = arity(args, 2, 3) {
        return e;
    }
    let range = tri!(criteria_range(ev, &args[0]));
    let crit = ev.eval_scalar(&args[1]);
    let criteria = Criteria::parse(&crit, &ev.options());
    let sum_range = if omitted(args, 2) {
        range.clone()
    } else {
        tri!(sized_like(ev, &args[2], range.rows, range.cols))
    };
    let mut total = 0.0;
    for (i, v) in range.data.iter().enumerate() {
        if criteria.matches(v) {
            match sum_range.data.get(i) {
                Some(Value::Number(n)) => total += n,
                Some(Value::Error(e)) => return Eval::error(*e),
                _ => {}
            }
        }
    }
    Eval::number(total)
}

fn sum_ifs(ev: &mut Evaluator, args: &[Expr]) -> Eval {
    if args.len() < 3 || args.len() % 2 == 0 {
        return Eval::error(CellError::Value);
    }
    let (rows, cols, mask) = tri!(ifs_mask(ev, &args[1..]));
    let sum_range = tri!(criteria_range(ev, &args[0]));
    if sum_range.cols != cols {
        return Eval::error(CellError::Value);
    }
    let mut total = 0.0;
    for r in 0..rows.min(sum_range.rows) {
        for c in 0..cols {
            if mask[r * cols + c] {
                match sum_range.get(r, c) {
                    Value::Number(n) => total += n,
                    Value::Error(e) => return Eval::error(*e),
                    _ => {}
                }
            }
        }
    }
    Eval::number(total)
}

fn sum_product(ev: &mut Evaluator, args: &[Expr]) -> Eval {
    if args.is_empty() {
        return Eval::error(CellError::Value);
    }
    let mut arrays = Vec::with_capacity(args.len());
    for a in args {
        let v = ev.eval(a);
        if let Some(e) = v.as_error() {
            return Eval::error(e);
        }
        arrays.push(ev.to_array(v));
    }
    let (rows, cols) = (arrays[0].rows, arrays[0].cols);
    if arrays.iter().any(|a| a.rows != rows || a.cols != cols) {
        return Eval::error(CellError::Value);
    }
    let mut total = 0.0;
    for i in 0..rows * cols {
        let mut p = 1.0;
        for a in &arrays {
            match &a.data[i] {
                Value::Number(n) => p *= n,
                Value::Error(e) => return Eval::error(*e),
                _ => p = 0.0,
            }
        }
        total += p;
    }
    Eval::number(total)
}

fn pairs_sum(ev: &mut Evaluator, args: &[Expr], f: fn(f64, f64) -> f64) -> Eval {
    if let Some(e) = arity(args, 2, 2) {
        return e;
    }
    let x = super::array_arg(ev, args, 0);
    let y = super::array_arg(ev, args, 1);
    if x.data.len() != y.data.len() {
        return Eval::error(CellError::NA);
    }
    let mut total = 0.0;
    let mut pairs = 0;
    for (a, b) in x.data.iter().zip(&y.data) {
        match (a, b) {
            (Value::Error(e), _) | (_, Value::Error(e)) => return Eval::error(*e),
            (Value::Number(p), Value::Number(q)) => {
                total += f(*p, *q);
                pairs += 1;
            }
            _ => {}
        }
    }
    // No pair of numbers at all: Excel's #DIV/0!.
    if pairs == 0 {
        return Eval::error(CellError::Div0);
    }
    Eval::number(total)
}

/// Whether a cell holds SUBTOTAL or AGGREGATE (which totals skip, so that
/// subtotals are not counted twice).
fn is_subtotal_cell(ev: &Evaluator, sheet: usize, cell: crate::refs::CellRef) -> bool {
    ev.grid.formula_text(sheet, cell).is_some_and(|f| {
        let u = f.to_ascii_uppercase();
        u.contains("SUBTOTAL(") || u.contains("AGGREGATE(")
    })
}

/// The values a SUBTOTAL or AGGREGATE reference argument contributes.
fn collect_cells(
    ev: &mut Evaluator,
    arg: &Expr,
    skip_hidden: bool,
    skip_nested: bool,
) -> Result<Vec<Value>, CellError> {
    let v = ev.eval(arg);
    match v {
        Eval::Ref(areas) => {
            let mut out = Vec::new();
            for area in areas {
                let Some(range) = ev.clip(area) else { continue };
                let cells = ev.grid.cells_in(area.sheet, range);
                for (cell, value) in cells {
                    if skip_hidden && ev.grid.row_hidden(area.sheet, cell.row) {
                        continue;
                    }
                    if skip_nested && is_subtotal_cell(ev, area.sheet, cell) {
                        continue;
                    }
                    out.push(value);
                }
            }
            Ok(out)
        }
        Eval::Array(a) => Ok(a.data),
        Eval::Value(Value::Error(e)) => Err(e),
        Eval::Value(_) => Err(CellError::Value),
    }
}

fn subtotal(ev: &mut Evaluator, args: &[Expr]) -> Eval {
    if args.len() < 2 {
        return Eval::error(CellError::Value);
    }
    let code = tri!(ev.number(args, 0)).trunc() as i64;
    let (func, skip_hidden) = match code {
        1..=11 => (code as u32, false),
        101..=111 => ((code - 100) as u32, true),
        _ => return Eval::error(CellError::Value),
    };
    let mut values = Vec::new();
    for a in &args[1..] {
        values.extend(tri!(collect_cells(ev, a, skip_hidden, true)));
    }
    if let Some(e) = values.iter().find_map(Value::as_error) {
        return Eval::error(e);
    }
    summarize(func, &values, None).into()
}

fn aggregate(ev: &mut Evaluator, args: &[Expr]) -> Eval {
    if args.len() < 3 {
        return Eval::error(CellError::Value);
    }
    let func = tri!(ev.number(args, 0)).trunc() as i64;
    let options = if omitted(args, 1) { 0 } else { tri!(ev.number(args, 1)).trunc() as i64 };
    if !(1..=19).contains(&func) || !(0..=7).contains(&options) {
        return Eval::error(CellError::Value);
    }
    let skip_nested = matches!(options, 0..=3);
    let skip_hidden = matches!(options, 1 | 3 | 5 | 7);
    let skip_errors = matches!(options, 2 | 3 | 6 | 7);
    let takes_k = func >= 14;
    let (data_args, k) = if takes_k {
        if args.len() != 4 {
            return Eval::error(CellError::Value);
        }
        (&args[2..3], Some(tri!(ev.number(args, 3))))
    } else {
        (&args[2..], None)
    };
    let mut values = Vec::new();
    for a in data_args {
        values.extend(tri!(collect_cells(ev, a, skip_hidden, skip_nested)));
    }
    if skip_errors {
        values.retain(|v| v.as_error().is_none());
    } else if let Some(e) = values.iter().find_map(Value::as_error) {
        return Eval::error(e);
    }
    summarize(func as u32, &values, k).into()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rounding_rules() {
        assert_eq!(round(2.675, 2.0), 2.68);
        assert_eq!(round(-2.5, 0.0), -3.0);
        assert_eq!(round(1234.5678, -2.0), 1200.0);
        assert_eq!(round_away(3.2, 0), 4.0);
        assert_eq!(round_away(-3.2, 0), -4.0);
        assert_eq!(round_away(0.1 + 0.2, 1), 0.3);
        assert_eq!(round_toward_zero(-3.99, 0), -3.0);
        assert_eq!(round_toward_zero(3.14159, 3), 3.141);
        assert_eq!(modulo(-3.0, 2.0), Ok(1.0));
        assert_eq!(modulo(3.0, -2.0), Ok(-1.0));
        assert_eq!(modulo(1.0, 0.0), Err(CellError::Div0));
        assert_eq!(combin(5.0, 2.0), Ok(10.0));
        assert_eq!(factorial(5.0), Ok(120.0));
    }
}
