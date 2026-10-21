//! Loans, investments and depreciation.

use super::super::ast::Expr;
use super::super::eval::{Eval, Evaluator};
use super::date::date_arg;
use super::{arity, lift_nums, omitted, tri_some};
use crate::value::{CellError, Value};

pub fn call(ev: &mut Evaluator, name: &str, args: &[Expr]) -> Option<Eval> {
    Some(match name {
        "PMT" => lift_nums(ev, args, 3, &[0.0, 0.0], |a| pmt(a[0], a[1], a[2], a[3], a[4])),
        "FV" => lift_nums(ev, args, 3, &[0.0, 0.0], |a| Ok(fv(a[0], a[1], a[2], a[3], a[4]))),
        "PV" => lift_nums(ev, args, 3, &[0.0, 0.0], |a| Ok(pv(a[0], a[1], a[2], a[3], a[4]))),
        "NPER" => lift_nums(ev, args, 3, &[0.0, 0.0], |a| nper(a[0], a[1], a[2], a[3], a[4])),
        "IPMT" => lift_nums(ev, args, 4, &[0.0, 0.0], |a| ipmt(a[0], a[1], a[2], a[3], a[4], a[5])),
        "PPMT" => lift_nums(ev, args, 4, &[0.0, 0.0], |a| {
            Ok(pmt(a[0], a[2], a[3], a[4], a[5])? - ipmt(a[0], a[1], a[2], a[3], a[4], a[5])?)
        }),
        "RATE" => lift_nums(ev, args, 3, &[0.0, 0.0, 0.1], |a| rate(a[0], a[1], a[2], a[3], a[4], a[5])),
        "NPV" => {
            if args.len() < 2 {
                return Some(Eval::error(CellError::Value));
            }
            let r = tri_some!(ev.number(args, 0));
            let values = tri_some!(ev.numbers(&args[1..]));
            npv(r, &values).into()
        }
        "XNPV" => {
            if let Some(e) = arity(args, 3, 3) {
                return Some(e);
            }
            let r = tri_some!(ev.number(args, 0));
            let (values, dates) = tri_some!(cash_flows(ev, args, 1, 2));
            xnpv(r, &values, &dates).into()
        }
        "IRR" => {
            if let Some(e) = arity(args, 1, 2) {
                return Some(e);
            }
            let values = tri_some!(ev.numbers(&args[..1]));
            let guess = if omitted(args, 1) { 0.1 } else { tri_some!(ev.number(args, 1)) };
            irr(&values, guess).into()
        }
        "XIRR" => {
            if let Some(e) = arity(args, 2, 3) {
                return Some(e);
            }
            let (values, dates) = tri_some!(cash_flows(ev, args, 0, 1));
            let guess = if omitted(args, 2) { 0.1 } else { tri_some!(ev.number(args, 2)) };
            xirr(&values, &dates, guess).into()
        }
        "MIRR" => {
            if let Some(e) = arity(args, 3, 3) {
                return Some(e);
            }
            let values = tri_some!(ev.numbers(&args[..1]));
            let fr = tri_some!(ev.number(args, 1));
            let rr = tri_some!(ev.number(args, 2));
            mirr(&values, fr, rr).into()
        }
        "SLN" => lift_nums(ev, args, 3, &[], |a| {
            if a[2] == 0.0 {
                Err(CellError::Div0)
            } else {
                Ok((a[0] - a[1]) / a[2])
            }
        }),
        "SYD" => lift_nums(ev, args, 4, &[], |a| {
            let (cost, salvage, life, per) = (a[0], a[1], a[2], a[3]);
            if life <= 0.0 || per <= 0.0 || per > life {
                return Err(CellError::Num);
            }
            Ok((cost - salvage) * (life - per + 1.0) * 2.0 / (life * (life + 1.0)))
        }),
        "DB" => lift_nums(ev, args, 4, &[12.0], |a| db(a[0], a[1], a[2], a[3], a[4])),
        "DDB" => lift_nums(ev, args, 4, &[2.0], |a| ddb(a[0], a[1], a[2], a[3], a[4])),
        "EFFECT" => lift_nums(ev, args, 2, &[], |a| {
            let n = a[1].trunc();
            if a[0] <= 0.0 || n < 1.0 {
                return Err(CellError::Num);
            }
            Ok((1.0 + a[0] / n).powf(n) - 1.0)
        }),
        "NOMINAL" => lift_nums(ev, args, 2, &[], |a| {
            let n = a[1].trunc();
            if a[0] <= 0.0 || n < 1.0 {
                return Err(CellError::Num);
            }
            Ok(n * ((1.0 + a[0]).powf(1.0 / n) - 1.0))
        }),
        "CUMIPMT" | "CUMPRINC" => {
            let interest = name == "CUMIPMT";
            lift_nums(ev, args, 6, &[], move |a| cumulative(a[0], a[1], a[2], a[3], a[4], a[5], interest))
        }
        "PDURATION" => lift_nums(ev, args, 3, &[], |a| {
            let (r, p, f) = (a[0], a[1], a[2]);
            if r <= 0.0 || p <= 0.0 || f <= 0.0 {
                return Err(CellError::Num);
            }
            Ok((f.ln() - p.ln()) / (1.0 + r).ln())
        }),
        "RRI" => lift_nums(ev, args, 3, &[], |a| {
            let (n, p, f) = (a[0], a[1], a[2]);
            if n <= 0.0 || p == 0.0 {
                return Err(CellError::Num);
            }
            Ok((f / p).powf(1.0 / n) - 1.0)
        }),
        _ => return None,
    })
}

pub(crate) fn fv(r: f64, n: f64, pmt: f64, pv: f64, t: f64) -> f64 {
    let t = if t != 0.0 { 1.0 } else { 0.0 };
    if r == 0.0 {
        return -(pv + pmt * n);
    }
    let g = (1.0 + r).powf(n);
    -(pv * g + pmt * (1.0 + r * t) * (g - 1.0) / r)
}

pub(crate) fn pv(r: f64, n: f64, pmt: f64, fv: f64, t: f64) -> f64 {
    let t = if t != 0.0 { 1.0 } else { 0.0 };
    if r == 0.0 {
        return -(fv + pmt * n);
    }
    let g = (1.0 + r).powf(n);
    -(fv + pmt * (1.0 + r * t) * (g - 1.0) / r) / g
}

pub(crate) fn pmt(r: f64, n: f64, pv: f64, fv: f64, t: f64) -> Result<f64, CellError> {
    let t = if t != 0.0 { 1.0 } else { 0.0 };
    if n == 0.0 {
        return Err(CellError::Num);
    }
    if r == 0.0 {
        return Ok(-(pv + fv) / n);
    }
    let g = (1.0 + r).powf(n);
    Ok(-(r * (fv + pv * g)) / ((1.0 + r * t) * (g - 1.0)))
}

fn nper(r: f64, pmt: f64, pv: f64, fv: f64, t: f64) -> Result<f64, CellError> {
    let t = if t != 0.0 { 1.0 } else { 0.0 };
    if r == 0.0 {
        if pmt == 0.0 {
            return Err(CellError::Num);
        }
        return Ok(-(pv + fv) / pmt);
    }
    let num = pmt * (1.0 + r * t) - fv * r;
    let den = pv * r + pmt * (1.0 + r * t);
    if den == 0.0 || num / den <= 0.0 {
        return Err(CellError::Num);
    }
    Ok((num / den).ln() / (1.0 + r).ln())
}

fn ipmt(r: f64, per: f64, n: f64, pv: f64, fv_: f64, t: f64) -> Result<f64, CellError> {
    let t = if t != 0.0 { 1.0 } else { 0.0 };
    if per < 1.0 || per > n {
        return Err(CellError::Num);
    }
    let p = pmt(r, n, pv, fv_, t)?;
    if t == 1.0 && per == 1.0 {
        return Ok(0.0);
    }
    let balance = fv(r, per - 1.0, p, pv, t);
    let i = balance * r;
    Ok(if t == 1.0 { i / (1.0 + r) } else { i })
}

/// Newton's method with a bisection fallback on [lo, hi].
fn solve(f: impl Fn(f64) -> f64, guess: f64, lo: f64, hi: f64) -> Result<f64, CellError> {
    let mut x = guess;
    for _ in 0..100 {
        let y = f(x);
        if !y.is_finite() {
            break;
        }
        if y.abs() < 1e-10 {
            return Ok(x);
        }
        let h = 1e-7 * x.abs().max(1e-7);
        let d = (f(x + h) - f(x - h)) / (2.0 * h);
        if d == 0.0 || !d.is_finite() {
            break;
        }
        let next = x - y / d;
        if !next.is_finite() || next <= lo {
            break;
        }
        if (next - x).abs() < 1e-12 * next.abs().max(1.0) {
            return Ok(next);
        }
        x = next;
    }
    // Bisection over sign changes on a grid of the interval.
    let steps = 400;
    let mut prev_x = lo + (hi - lo) / steps as f64 * 0.5;
    let mut prev_y = f(prev_x);
    for i in 1..=steps {
        let xi = lo + (hi - lo) * (i as f64 + 0.5) / steps as f64;
        let yi = f(xi);
        if prev_y.is_finite() && yi.is_finite() && prev_y.signum() != yi.signum() {
            let (mut a, mut b, mut fa) = (prev_x, xi, prev_y);
            for _ in 0..200 {
                let m = (a + b) / 2.0;
                let fm = f(m);
                if fm.abs() < 1e-12 || (b - a).abs() < 1e-15 {
                    return Ok(m);
                }
                if fa.signum() == fm.signum() {
                    a = m;
                    fa = fm;
                } else {
                    b = m;
                }
            }
            return Ok((a + b) / 2.0);
        }
        prev_x = xi;
        prev_y = yi;
    }
    Err(CellError::Num)
}

fn rate(n: f64, pmt: f64, pv: f64, fv: f64, t: f64, guess: f64) -> Result<f64, CellError> {
    if n <= 0.0 {
        return Err(CellError::Num);
    }
    let t = if t != 0.0 { 1.0 } else { 0.0 };
    let f = |r: f64| {
        if r.abs() < 1e-12 {
            pv + pmt * n + fv
        } else {
            let g = (1.0 + r).powf(n);
            pv * g + pmt * (1.0 + r * t) * (g - 1.0) / r + fv
        }
    };
    solve(f, guess, -0.999_999, 10.0)
}

pub(crate) fn npv(r: f64, values: &[f64]) -> Result<f64, CellError> {
    if r == -1.0 {
        return Err(CellError::Div0);
    }
    Ok(values.iter().enumerate().map(|(i, v)| v / (1.0 + r).powi(i as i32 + 1)).sum())
}

fn irr(values: &[f64], guess: f64) -> Result<f64, CellError> {
    if !values.iter().any(|v| *v > 0.0) || !values.iter().any(|v| *v < 0.0) {
        return Err(CellError::Num);
    }
    let f = |r: f64| values.iter().enumerate().map(|(i, v)| v / (1.0 + r).powi(i as i32)).sum::<f64>();
    solve(f, guess, -0.999_999, 10.0)
}

fn cash_flows(ev: &mut Evaluator, args: &[Expr], vi: usize, di: usize) -> Result<(Vec<f64>, Vec<f64>), CellError> {
    let options = ev.options();
    let v = super::array_arg(ev, args, vi);
    let d = super::array_arg(ev, args, di);
    if v.data.len() != d.data.len() || v.data.is_empty() {
        return Err(CellError::Num);
    }
    let mut values = Vec::with_capacity(v.data.len());
    let mut dates = Vec::with_capacity(d.data.len());
    for (a, b) in v.data.iter().zip(&d.data) {
        values.push(match a {
            Value::Number(n) => *n,
            Value::Error(e) => return Err(*e),
            _ => return Err(CellError::Value),
        });
        dates.push(date_arg(b, &options)?.floor());
    }
    if dates.iter().any(|x| *x < dates[0]) {
        return Err(CellError::Num);
    }
    Ok((values, dates))
}

fn xnpv(r: f64, values: &[f64], dates: &[f64]) -> Result<f64, CellError> {
    if r <= -1.0 {
        return Err(CellError::Num);
    }
    Ok(values.iter().zip(dates).map(|(v, d)| v / (1.0 + r).powf((d - dates[0]) / 365.0)).sum())
}

fn xirr(values: &[f64], dates: &[f64], guess: f64) -> Result<f64, CellError> {
    if !values.iter().any(|v| *v > 0.0) || !values.iter().any(|v| *v < 0.0) {
        return Err(CellError::Num);
    }
    let f = |r: f64| values.iter().zip(dates).map(|(v, d)| v / (1.0 + r).powf((d - dates[0]) / 365.0)).sum::<f64>();
    solve(f, guess, -0.999_999, 10.0)
}

fn mirr(values: &[f64], finance: f64, reinvest: f64) -> Result<f64, CellError> {
    let n = values.len() as f64;
    if n < 2.0 {
        return Err(CellError::Div0);
    }
    let pos: f64 = values
        .iter()
        .enumerate()
        .filter(|(_, v)| **v > 0.0)
        .map(|(i, v)| v * (1.0 + reinvest).powf(n - 1.0 - i as f64))
        .sum();
    let neg: f64 = values
        .iter()
        .enumerate()
        .filter(|(_, v)| **v < 0.0)
        .map(|(i, v)| v / (1.0 + finance).powi(i as i32))
        .sum();
    if pos == 0.0 || neg == 0.0 {
        return Err(CellError::Div0);
    }
    Ok((-pos / neg).powf(1.0 / (n - 1.0)) - 1.0)
}

fn db(cost: f64, salvage: f64, life: f64, period: f64, month: f64) -> Result<f64, CellError> {
    if cost < 0.0 || salvage < 0.0 || life <= 0.0 || period <= 0.0 || month <= 0.0 || month > 12.0 {
        return Err(CellError::Num);
    }
    let period = period.trunc();
    if period > life + 1.0 || (month == 12.0 && period > life) {
        return Err(CellError::Num);
    }
    let rate = if cost == 0.0 {
        0.0
    } else {
        super::math::round(1.0 - (salvage / cost).powf(1.0 / life), 3.0)
    };
    let mut total = 0.0;
    let mut dep = 0.0;
    for p in 1..=period as i64 {
        dep = if p == 1 {
            cost * rate * month / 12.0
        } else if p as f64 == life + 1.0 {
            (cost - total) * rate * (12.0 - month) / 12.0
        } else {
            (cost - total) * rate
        };
        total += dep;
    }
    Ok(dep)
}

/// The most periods DDB, CUMIPMT and CUMPRINC step through: a thousand
/// times a monthly loan over a century.
const MAX_PERIODS: f64 = 1_000_000.0;

fn ddb(cost: f64, salvage: f64, life: f64, period: f64, factor: f64) -> Result<f64, CellError> {
    if cost < 0.0 || salvage < 0.0 || life <= 0.0 || period <= 0.0 || factor <= 0.0 || period > life {
        return Err(CellError::Num);
    }
    // One step per period: a period of 1e15 would never finish.
    if period > MAX_PERIODS {
        return Err(CellError::Num);
    }
    let mut book = cost;
    let mut dep = 0.0;
    let whole = period.ceil() as i64;
    for _ in 0..whole {
        dep = (book * factor / life).min((book - salvage).max(0.0));
        book -= dep;
    }
    Ok(dep)
}

fn cumulative(r: f64, n: f64, pv: f64, start: f64, end: f64, t: f64, interest: bool) -> Result<f64, CellError> {
    let (start, end) = (start.trunc(), end.trunc());
    if r <= 0.0 || n <= 0.0 || pv <= 0.0 || start < 1.0 || end < start || end > n || (t != 0.0 && t != 1.0) {
        return Err(CellError::Num);
    }
    // One step per period between start and end.
    if end - start > MAX_PERIODS {
        return Err(CellError::Num);
    }
    let mut total = 0.0;
    let mut p = start;
    while p <= end {
        let i = ipmt(r, p, n, pv, 0.0, t)?;
        total += if interest { i } else { pmt(r, n, pv, 0.0, t)? - i };
        p += 1.0;
    }
    Ok(total)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn close(a: f64, b: f64) -> bool {
        (a - b).abs() < 1e-6 * b.abs().max(1.0)
    }

    #[test]
    fn loans() {
        // A ₹10 lakh loan at 9% a year for 20 years, monthly.
        let p = pmt(0.09 / 12.0, 240.0, 1_000_000.0, 0.0, 0.0).unwrap();
        assert!(close(p, -8997.259_759_3), "{p}");
        assert!(close(fv(0.06 / 12.0, 10.0, -200.0, -500.0, 1.0), 2581.403_337_9));
        assert!(close(pv(0.08 / 12.0, 240.0, 500.0, 0.0, 0.0), -59_777.145_851));
        assert!(close(nper(0.12 / 12.0, -100.0, -1000.0, 10_000.0, 1.0).unwrap(), 59.673_865_2));
        assert!(close(ipmt(0.1 / 12.0, 1.0, 36.0, 8000.0, 0.0, 0.0).unwrap(), -66.666_666_7));
        assert!(close(rate(48.0, -200.0, 8000.0, 0.0, 0.0, 0.1).unwrap(), 0.007_701_472_5));
        assert!(close(irr(&[-70_000.0, 12_000.0, 15_000.0, 18_000.0, 21_000.0, 26_000.0], 0.1).unwrap(), 0.086_630_948));
        assert!(close(npv(0.1, &[-10_000.0, 3000.0, 4200.0, 6800.0]).unwrap(), 1188.443_412_8));
        assert!(close(db(1_000_000.0, 100_000.0, 6.0, 1.0, 7.0).unwrap(), 186_083.333_33));
        assert!(close(ddb(2400.0, 300.0, 10.0 * 365.0, 1.0, 2.0).unwrap(), 1.315_068_49));
        assert!(close(
            mirr(&[-120_000.0, 39_000.0, 30_000.0, 21_000.0, 37_000.0, 46_000.0], 0.1, 0.12).unwrap(),
            0.126_094_06
        ));
    }
}
