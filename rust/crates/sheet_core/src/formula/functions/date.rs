//! Date and time functions over Excel serials.

use super::super::ast::Expr;
use super::super::eval::{Eval, Evaluator};
use super::{tri_some, arity, lift, omitted, tri};
use crate::datetime::{date_to_serial, days_from_civil, days_in_month, is_leap_year, serial_to_date, weekday};
use crate::input::parse_date_time;
use crate::numfmt::FormatOptions;
use crate::value::{parse_number, CellError, Value};

pub fn call(ev: &mut Evaluator, name: &str, args: &[Expr]) -> Option<Eval> {
    let o = ev.options();
    Some(match name {
        "TODAY" | "NOW" => {
            if let Some(e) = arity(args, 0, 0) {
                return Some(e);
            }
            ev.volatile = true;
            let now = ev.grid.now();
            Eval::number(if name == "TODAY" { now.floor() } else { now })
        }
        "DATE" => {
            if let Some(e) = arity(args, 3, 3) {
                return Some(e);
            }
            lift(ev, args, &mut |v| {
                let n: Result<Vec<f64>, _> = v.iter().map(Value::to_number).collect();
                match n {
                    Ok(n) => match date_to_serial(n[0].trunc() as i64, n[1].trunc() as i64, n[2].trunc() as i64, o.date1904) {
                        Some(s) => Value::Number(s),
                        None => Value::Error(CellError::Num),
                    },
                    Err(e) => Value::Error(e),
                }
            })
        }
        "TIME" => {
            if let Some(e) = arity(args, 3, 3) {
                return Some(e);
            }
            lift(ev, args, &mut |v| {
                let n: Result<Vec<f64>, _> = v.iter().map(Value::to_number).collect();
                match n {
                    Ok(n) => {
                        let total = n[0].trunc() * 3600.0 + n[1].trunc() * 60.0 + n[2].trunc();
                        if total < 0.0 || n[0] > 32767.0 {
                            Value::Error(CellError::Num)
                        } else {
                            Value::Number(total.rem_euclid(86_400.0) / 86_400.0)
                        }
                    }
                    Err(e) => Value::Error(e),
                }
            })
        }
        "YEAR" | "MONTH" | "DAY" => {
            let part = match name {
                "YEAR" => 0,
                "MONTH" => 1,
                _ => 2,
            };
            date1(ev, args, &o, move |s, o| {
                let (y, m, d) = serial_to_date(s, o.date1904).ok_or(CellError::Num)?;
                Ok([y as f64, m as f64, d as f64][part])
            })
        }
        "HOUR" | "MINUTE" | "SECOND" => {
            let part = match name {
                "HOUR" => 0,
                "MINUTE" => 1,
                _ => 2,
            };
            date1(ev, args, &o, move |s, _| {
                let secs = ((s - s.floor()) * 86_400.0).round() as u64 % 86_400;
                Ok([(secs / 3600) as f64, ((secs / 60) % 60) as f64, (secs % 60) as f64][part])
            })
        }
        "WEEKDAY" => {
            if let Some(e) = arity(args, 1, 2) {
                return Some(e);
            }
            let kind = if omitted(args, 1) { 1.0 } else { tri_some!(ev.number(args, 1)).trunc() };
            let s = tri_some!(date_arg(&ev.arg(args, 0), &o));
            let w = tri_some!(weekday(s, o.date1904).ok_or(CellError::Num)) as i64;
            let r = match kind as i64 {
                1 | 17 => w,
                2 | 11 => (w + 5) % 7 + 1,
                3 => (w + 5) % 7,
                12 => (w + 4) % 7 + 1,
                13 => (w + 3) % 7 + 1,
                14 => (w + 2) % 7 + 1,
                15 => (w + 1) % 7 + 1,
                16 => w % 7 + 1,
                _ => return Some(Eval::error(CellError::Num)),
            };
            Eval::number(r as f64)
        }
        "WEEKNUM" => {
            if let Some(e) = arity(args, 1, 2) {
                return Some(e);
            }
            let kind = if omitted(args, 1) { 1.0 } else { tri_some!(ev.number(args, 1)).trunc() };
            let s = tri_some!(date_arg(&ev.arg(args, 0), &o));
            if kind == 21.0 {
                return Some(iso_week(s, &o).into());
            }
            // The weekday (1 = Sunday) a week starts on.
            let start = match kind as i64 {
                1 | 17 => 1,
                2 | 11 => 2,
                12 => 3,
                13 => 4,
                14 => 5,
                15 => 6,
                16 => 7,
                _ => return Some(Eval::error(CellError::Num)),
            };
            let (y, _, _) = tri_some!(serial_to_date(s, o.date1904).ok_or(CellError::Num));
            let jan1 = tri_some!(date_to_serial(y, 1, 1, o.date1904).ok_or(CellError::Num));
            let jw = tri_some!(weekday(jan1, o.date1904).ok_or(CellError::Num)) as i64;
            let offset = (jw - start).rem_euclid(7);
            let doy = s.floor() - jan1;
            Eval::number(((doy as i64 + offset) / 7 + 1) as f64)
        }
        "ISOWEEKNUM" => {
            if let Some(e) = arity(args, 1, 1) {
                return Some(e);
            }
            let s = tri_some!(date_arg(&ev.arg(args, 0), &o));
            iso_week(s, &o).into()
        }
        "EDATE" | "EOMONTH" => {
            if let Some(e) = arity(args, 2, 2) {
                return Some(e);
            }
            let end = name == "EOMONTH";
            let s = tri_some!(date_arg(&ev.arg(args, 0), &o));
            let months = tri_some!(ev.number(args, 1)).trunc() as i64;
            let (y, m, d) = tri_some!(serial_to_date(s, o.date1904).ok_or(CellError::Num));
            let total = y * 12 + (m as i64 - 1) + months;
            let (ny, nm) = (total.div_euclid(12), total.rem_euclid(12) as u32 + 1);
            let last = days_in_month(ny, nm) as i64;
            let day = if end { last } else { (d as i64).min(last) };
            match date_to_serial(ny, nm as i64, day, o.date1904) {
                Some(r) => Eval::number(r),
                None => Eval::error(CellError::Num),
            }
        }
        "DATEDIF" => datedif(ev, args, &o),
        "DAYS" => {
            if let Some(e) = arity(args, 2, 2) {
                return Some(e);
            }
            let end = tri_some!(date_arg(&ev.arg(args, 0), &o));
            let start = tri_some!(date_arg(&ev.arg(args, 1), &o));
            Eval::number(end.floor() - start.floor())
        }
        "DAYS360" => {
            if let Some(e) = arity(args, 2, 3) {
                return Some(e);
            }
            let s = tri_some!(date_arg(&ev.arg(args, 0), &o));
            let e = tri_some!(date_arg(&ev.arg(args, 1), &o));
            let european = tri_some!(ev.bool_or(args, 2, false));
            days360(s, e, european, &o).into()
        }
        "NETWORKDAYS" | "NETWORKDAYS.INTL" => networkdays(ev, args, name.ends_with("INTL"), &o),
        "WORKDAY" | "WORKDAY.INTL" => workday(ev, args, name.ends_with("INTL"), &o),
        "YEARFRAC" => {
            if let Some(e) = arity(args, 2, 3) {
                return Some(e);
            }
            let s = tri_some!(date_arg(&ev.arg(args, 0), &o));
            let e = tri_some!(date_arg(&ev.arg(args, 1), &o));
            let basis = if omitted(args, 2) { 0.0 } else { tri_some!(ev.number(args, 2)).trunc() };
            yearfrac(s, e, basis as i64, &o).into()
        }
        "DATEVALUE" | "TIMEVALUE" => {
            if let Some(e) = arity(args, 1, 1) {
                return Some(e);
            }
            let date = name == "DATEVALUE";
            lift(ev, args, &mut |v| match &v[0] {
                Value::Text(t) => match parse_date_time(t, o.date_order, o.date1904) {
                    Some((n, _)) => Value::Number(if date { n.floor() } else { n - n.floor() }),
                    None => Value::Error(CellError::Value),
                },
                Value::Error(e) => Value::Error(*e),
                _ => Value::Error(CellError::Value),
            })
        }
        _ => return None,
    })
}

/// A date argument: a serial, or text that reads as a date or number.
pub(crate) fn date_arg(v: &Value, o: &FormatOptions) -> Result<f64, CellError> {
    let n = match v {
        Value::Number(n) => *n,
        Value::Empty => 0.0,
        Value::Bool(b) => *b as u8 as f64,
        Value::Error(e) => return Err(*e),
        Value::Text(t) => match parse_number(t) {
            Some(n) => n,
            None => parse_date_time(t, o.date_order, o.date1904).map(|(n, _)| n).ok_or(CellError::Value)?,
        },
    };
    if n < 0.0 {
        Err(CellError::Num)
    } else {
        Ok(n)
    }
}

fn date1(ev: &mut Evaluator, args: &[Expr], o: &FormatOptions, f: impl Fn(f64, &FormatOptions) -> Result<f64, CellError>) -> Eval {
    if let Some(e) = arity(args, 1, 1) {
        return e;
    }
    lift(ev, args, &mut |v| match date_arg(&v[0], o).and_then(|s| f(s, o)) {
        Ok(n) => Value::Number(n),
        Err(e) => Value::Error(e),
    })
}

fn day_of_year(y: i64, m: u32, d: u32) -> i64 {
    days_from_civil(y, m, d as i64) - days_from_civil(y, 1, 1) + 1
}

fn iso_week(s: f64, o: &FormatOptions) -> Result<f64, CellError> {
    let (y, m, d) = serial_to_date(s, o.date1904).ok_or(CellError::Num)?;
    let w = weekday(s, o.date1904).ok_or(CellError::Num)? as i64;
    let iso_wd = (w + 5) % 7 + 1; // Monday = 1
    let doy = day_of_year(y, m, d);
    let weeks_in = |year: i64| {
        let jan1 = (days_from_civil(year, 1, 1) + 3).rem_euclid(7) + 1; // Monday = 1
        if jan1 == 4 || (jan1 == 3 && is_leap_year(year)) {
            53
        } else {
            52
        }
    };
    let week = (doy - iso_wd + 10) / 7;
    Ok(if week < 1 {
        weeks_in(y - 1) as f64
    } else if week > weeks_in(y) {
        1.0
    } else {
        week as f64
    })
}

fn datedif(ev: &mut Evaluator, args: &[Expr], o: &FormatOptions) -> Eval {
    if let Some(e) = arity(args, 3, 3) {
        return e;
    }
    let s = tri!(date_arg(&ev.arg(args, 0), o)).floor();
    let e = tri!(date_arg(&ev.arg(args, 1), o)).floor();
    let unit = tri!(ev.text(args, 2)).to_ascii_uppercase();
    if s > e {
        return Eval::error(CellError::Num);
    }
    let (y1, m1, d1) = tri!(serial_to_date(s, o.date1904).ok_or(CellError::Num));
    let (y2, m2, d2) = tri!(serial_to_date(e, o.date1904).ok_or(CellError::Num));
    let months = (y2 - y1) * 12 + m2 as i64 - m1 as i64 - (d2 < d1) as i64;
    let r = match unit.as_str() {
        "Y" => months / 12,
        "M" => months,
        "D" => (e - s) as i64,
        "YM" => months.rem_euclid(12),
        "MD" => {
            if d2 >= d1 {
                (d2 - d1) as i64
            } else {
                let (py, pm) = if m2 == 1 { (y2 - 1, 12) } else { (y2, m2 - 1) };
                days_in_month(py, pm) as i64 - d1 as i64 + d2 as i64
            }
        }
        "YD" => {
            let mut start = date_to_serial(y2, m1 as i64, d1 as i64, o.date1904).unwrap_or(s);
            if start > e {
                start = date_to_serial(y2 - 1, m1 as i64, d1 as i64, o.date1904).unwrap_or(s);
            }
            (e - start) as i64
        }
        _ => return Eval::error(CellError::Num),
    };
    Eval::number(r as f64)
}

fn days360(s: f64, e: f64, european: bool, o: &FormatOptions) -> Result<f64, CellError> {
    let (y1, m1, mut d1) = serial_to_date(s, o.date1904).ok_or(CellError::Num)?;
    let (y2, mut m2, mut d2) = serial_to_date(e, o.date1904).ok_or(CellError::Num)?;
    let mut y2 = y2;
    if european {
        d1 = d1.min(30);
        d2 = d2.min(30);
    } else {
        let last_of_feb = |y: i64, m: u32, d: u32| m == 2 && d == days_in_month(y, 2);
        if d1 == 31 || last_of_feb(y1, m1, d1) {
            d1 = 30;
        }
        if d2 == 31 {
            if d1 < 30 {
                d2 = 1;
                m2 += 1;
                if m2 > 12 {
                    m2 = 1;
                    y2 += 1;
                }
            } else {
                d2 = 30;
            }
        }
    }
    Ok(((y2 - y1) * 360 + (m2 as i64 - m1 as i64) * 30 + (d2 as i64 - d1 as i64)) as f64)
}

/// Which weekdays (index 0 = Monday) are weekend days.
fn weekend_mask(v: Option<&Value>) -> Result<[bool; 7], CellError> {
    let mut mask = [false; 7];
    match v {
        None | Some(Value::Empty) => {
            mask[5] = true;
            mask[6] = true;
        }
        Some(Value::Text(t)) => {
            if t.len() != 7 || !t.bytes().all(|b| b == b'0' || b == b'1') || t.as_ref() == "1111111" {
                return Err(CellError::Value);
            }
            for (i, b) in t.bytes().enumerate() {
                mask[i] = b == b'1';
            }
        }
        Some(v) => {
            let code = v.to_number()?.trunc() as i64;
            match code {
                1..=7 => {
                    // 1: Sat+Sun, 2: Sun+Mon, ... 7: Fri+Sat.
                    let first = (code + 4) % 7; // index of the first weekend day
                    mask[first as usize] = true;
                    mask[((first + 1) % 7) as usize] = true;
                }
                11..=17 => {
                    // 11: Sunday only, 12: Monday only, ... 17: Saturday only.
                    mask[((code - 11 + 6) % 7) as usize] = true;
                }
                _ => return Err(CellError::Num),
            }
        }
    }
    Ok(mask)
}

/// Monday-based weekday index (0..6) of a serial.
fn weekday_index(serial: f64, o: &FormatOptions) -> usize {
    weekday(serial, o.date1904).map_or(0, |w| ((w as i64 + 5) % 7) as usize)
}

fn holidays(ev: &mut Evaluator, args: &[Expr], i: usize, o: &FormatOptions) -> Result<Vec<f64>, CellError> {
    if omitted(args, i) {
        return Ok(Vec::new());
    }
    let mut out = Vec::new();
    for (v, _) in ev.flatten(&args[i]) {
        match v {
            Value::Empty => {}
            other => out.push(date_arg(&other, o)?.floor()),
        }
    }
    out.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    out.dedup();
    Ok(out)
}

fn networkdays(ev: &mut Evaluator, args: &[Expr], intl: bool, o: &FormatOptions) -> Eval {
    if let Some(e) = arity(args, 2, if intl { 4 } else { 3 }) {
        return e;
    }
    let s = tri!(date_arg(&ev.arg(args, 0), o)).floor();
    let e = tri!(date_arg(&ev.arg(args, 1), o)).floor();
    let (mask, hol_index) = if intl {
        let w = if omitted(args, 2) { None } else { Some(ev.arg(args, 2)) };
        (tri!(weekend_mask(w.as_ref())), 3)
    } else {
        (tri!(weekend_mask(None)), 2)
    };
    let hols = tri!(holidays(ev, args, hol_index, o));
    let (lo, hi, sign) = if s <= e { (s, e, 1.0) } else { (e, s, -1.0) };
    let total_days = (hi - lo) as i64 + 1;
    let work_per_week = mask.iter().filter(|w| !**w).count() as i64;
    let full_weeks = total_days / 7;
    let mut count = full_weeks * work_per_week;
    let start_wd = weekday_index(lo, o);
    for k in 0..total_days % 7 {
        if !mask[(start_wd + k as usize) % 7] {
            count += 1;
        }
    }
    for h in hols {
        if h >= lo && h <= hi && !mask[weekday_index(h, o)] {
            count -= 1;
        }
    }
    Eval::number(count as f64 * sign)
}

fn workday(ev: &mut Evaluator, args: &[Expr], intl: bool, o: &FormatOptions) -> Eval {
    if let Some(e) = arity(args, 2, if intl { 4 } else { 3 }) {
        return e;
    }
    let s = tri!(date_arg(&ev.arg(args, 0), o)).floor();
    let days = tri!(ev.number(args, 1)).trunc() as i64;
    let (mask, hol_index) = if intl {
        let w = if omitted(args, 2) { None } else { Some(ev.arg(args, 2)) };
        (tri!(weekend_mask(w.as_ref())), 3)
    } else {
        (tri!(weekend_mask(None)), 2)
    };
    let hols = tri!(holidays(ev, args, hol_index, o));
    let step = if days >= 0 { 1.0 } else { -1.0 };
    let mut remaining = days.abs();
    let mut d = s;
    let mut guard = 0u64;
    while remaining > 0 {
        d += step;
        guard += 1;
        if guard > 20_000_000 || d < 0.0 || d > crate::datetime::MAX_DATE_SERIAL {
            return Eval::error(CellError::Num);
        }
        if mask[weekday_index(d, o)] || hols.binary_search_by(|h| h.partial_cmp(&d).unwrap()).is_ok() {
            continue;
        }
        remaining -= 1;
    }
    Eval::number(d)
}

fn yearfrac(s: f64, e: f64, basis: i64, o: &FormatOptions) -> Result<f64, CellError> {
    let (s, e) = if s <= e { (s.floor(), e.floor()) } else { (e.floor(), s.floor()) };
    let (y1, m1, d1) = serial_to_date(s, o.date1904).ok_or(CellError::Num)?;
    let (y2, m2, d2) = serial_to_date(e, o.date1904).ok_or(CellError::Num)?;
    let days = e - s;
    let last_of_feb = |y: i64, m: u32, d: u32| m == 2 && d == days_in_month(y, 2);
    match basis {
        0 => {
            let (mut a, mut b) = (d1 as i64, d2 as i64);
            if a == 31 && b == 31 {
                a = 30;
                b = 30;
            } else if a == 31 {
                a = 30;
            } else if a == 30 && b == 31 {
                b = 30;
            } else if last_of_feb(y1, m1, d1) && last_of_feb(y2, m2, d2) {
                a = 30;
                b = 30;
            } else if last_of_feb(y1, m1, d1) {
                a = 30;
            }
            Ok(((y2 - y1) * 360 + (m2 as i64 - m1 as i64) * 30 + (b - a)) as f64 / 360.0)
        }
        1 => {
            if days == 0.0 {
                return Ok(0.0);
            }
            let within_year = y1 == y2 || (y2 == y1 + 1 && (m1 > m2 || (m1 == m2 && d1 >= d2)));
            if within_year {
                let year_len = if y1 == y2 && is_leap_year(y1) {
                    366.0
                } else {
                    // A 29 February between the dates (inclusive) makes it 366.
                    let has_feb29 = (y1..=y2).any(|y| {
                        is_leap_year(y) && {
                            let f = days_from_civil(y, 2, 29) as f64;
                            let a = days_from_civil(y1, m1, d1 as i64) as f64;
                            let b = days_from_civil(y2, m2, d2 as i64) as f64;
                            f >= a && f <= b
                        }
                    });
                    if has_feb29 {
                        366.0
                    } else {
                        365.0
                    }
                };
                Ok(days / year_len)
            } else {
                let years = (y2 - y1 + 1) as f64;
                let total: i64 = (y1..=y2).map(|y| if is_leap_year(y) { 366 } else { 365 }).sum();
                Ok(days / (total as f64 / years))
            }
        }
        2 => Ok(days / 360.0),
        3 => Ok(days / 365.0),
        4 => {
            let (a, b) = (d1.min(30) as i64, d2.min(30) as i64);
            Ok(((y2 - y1) * 360 + (m2 as i64 - m1 as i64) * 30 + (b - a)) as f64 / 360.0)
        }
        _ => Err(CellError::Num),
    }
}
