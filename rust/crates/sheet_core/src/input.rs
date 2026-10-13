//! Reading what someone types into a cell, the way Excel does.

use crate::datetime::{date_to_serial, MONTH_NAMES};
use crate::numfmt::{DateOrder, FormatOptions};
use crate::value::{parse_number, CellError};

/// What typed text means.
#[derive(Clone, Debug, PartialEq)]
pub enum Input {
    Empty,
    /// A formula, without its `=`.
    Formula(String),
    /// A number, and the number format the way it was typed suggests
    /// (`12%` suggests a percentage, `13/10/2026` a date), if any.
    Number(f64, Option<String>),
    Text(String),
    Bool(bool),
    Error(CellError),
}

/// Interpret typed text.
pub fn parse_input(text: &str, options: &FormatOptions) -> Input {
    if text.is_empty() {
        return Input::Empty;
    }
    if let Some(formula) = text.strip_prefix('=') {
        if !formula.trim().is_empty() {
            return Input::Formula(formula.to_owned());
        }
    }
    // A leading apostrophe keeps the rest as text ('0123 stays 0123).
    if let Some(rest) = text.strip_prefix('\'') {
        return Input::Text(rest.to_owned());
    }
    let trimmed = text.trim();
    match trimmed.to_ascii_uppercase().as_str() {
        "TRUE" => return Input::Bool(true),
        "FALSE" => return Input::Bool(false),
        _ => {}
    }
    if let Some(e) = CellError::parse(trimmed) {
        return Input::Error(e);
    }
    if let Some(n) = parse_number(trimmed) {
        return Input::Number(n, number_format_hint(trimmed));
    }
    if let Some((serial, format)) = parse_date_time(trimmed, options.date_order, options.date1904) {
        return Input::Number(serial, Some(format));
    }
    Input::Text(text.to_owned())
}

/// The number format suggested by how a number was typed.
fn number_format_hint(text: &str) -> Option<String> {
    let decimals = text
        .split(['.'])
        .nth(1)
        .map(|f| f.chars().take_while(|c| c.is_ascii_digit()).count())
        .unwrap_or(0);
    let frac = if decimals > 0 { format!(".{}", "0".repeat(decimals)) } else { String::new() };
    if text.contains('%') {
        return Some(format!("0{frac}%"));
    }
    for symbol in ['₹', '$', '€', '£', '¥'] {
        if text.contains(symbol) {
            return Some(format!("\"{symbol}\"#,##0{frac}"));
        }
    }
    if text.contains(',') {
        return Some(format!("#,##0{frac}"));
    }
    if text.contains(['e', 'E']) {
        return Some("0.00E+00".into());
    }
    None
}

fn month_from_name(word: &str) -> Option<u32> {
    let lower = word.to_ascii_lowercase();
    if lower.len() < 3 {
        return None;
    }
    MONTH_NAMES
        .iter()
        .position(|m| {
            let m = m.to_ascii_lowercase();
            m.starts_with(&lower) || (lower.starts_with(&m[..3]) && m.starts_with(lower.trim_end_matches('.')))
        })
        .map(|i| i as u32 + 1)
}

fn expand_year(y: i64, digits: usize) -> i64 {
    if digits <= 2 {
        // Excel's two-digit year window: 00-29 is 2000-2029.
        if y < 30 {
            2000 + y
        } else {
            1900 + y
        }
    } else {
        y
    }
}

/// Parse a time of day: `14:30`, `2:30 pm`, `14:30:15.5`, `2 PM`.
/// Returns the fraction of a day and the format it suggests.
pub fn parse_time(text: &str) -> Option<(f64, String)> {
    let t = text.trim();
    let lower = t.to_ascii_lowercase();
    let (body, ampm) = if let Some(b) = lower.strip_suffix("am").or_else(|| lower.strip_suffix("a")) {
        (b.trim().to_owned(), Some(false))
    } else if let Some(b) = lower.strip_suffix("pm").or_else(|| lower.strip_suffix("p")) {
        (b.trim().to_owned(), Some(true))
    } else {
        (lower.clone(), None)
    };
    let parts: Vec<&str> = body.split(':').collect();
    if parts.is_empty() || parts.len() > 3 || (parts.len() == 1 && ampm.is_none()) {
        return None;
    }
    let num = |s: &str| -> Option<f64> {
        let s = s.trim();
        (!s.is_empty() && s.chars().all(|c| c.is_ascii_digit() || c == '.')).then(|| s.parse().ok()).flatten()
    };
    let mut h = num(parts[0])?;
    let m = parts.get(1).map_or(Some(0.0), |p| num(p))?;
    let s = parts.get(2).map_or(Some(0.0), |p| num(p))?;
    if h.fract() != 0.0 || m.fract() != 0.0 || m >= 60.0 || s >= 60.0 {
        return None;
    }
    if let Some(pm) = ampm {
        if !(1.0..=12.0).contains(&h) {
            return None;
        }
        if h == 12.0 {
            h = 0.0;
        }
        if pm {
            h += 12.0;
        }
    } else if h >= 10_000.0 {
        return None;
    }
    let format = match (ampm.is_some(), parts.len()) {
        (true, 3) => "h:mm:ss AM/PM",
        (true, _) => "h:mm AM/PM",
        (false, 3) if parts[2].contains('.') => "h:mm:ss.0",
        (false, 3) => "h:mm:ss",
        _ => "h:mm",
    };
    Some(((h * 3600.0 + m * 60.0 + s) / 86_400.0, format.to_owned()))
}

/// Parse a date, a time, or a date and time. Returns the serial and the
/// format that matches how it was written.
pub fn parse_date_time(text: &str, order: DateOrder, date1904: bool) -> Option<(f64, String)> {
    let t = text.trim();
    if t.is_empty() || t.len() > 40 {
        return None;
    }
    if let Some(time) = parse_time(t) {
        return Some(time);
    }
    // "date time": split at the last space before something with a colon.
    if let Some(space) = t.rfind(' ') {
        let (d, tm) = (&t[..space], &t[space + 1..]);
        let (d, tm) = if tm.eq_ignore_ascii_case("am") || tm.eq_ignore_ascii_case("pm") {
            match d.rfind(' ') {
                Some(s2) => (&t[..s2], &t[s2 + 1..]),
                None => (d, tm),
            }
        } else {
            (d, tm)
        };
        if tm.contains(':') {
            if let (Some((date, df)), Some((time, tf))) = (parse_date(d, order, date1904), parse_time(tm)) {
                return Some((date + time, format!("{df} {tf}")));
            }
        }
    }
    parse_date(t, order, date1904)
}

/// Parse a date on its own.
pub fn parse_date(text: &str, order: DateOrder, date1904: bool) -> Option<(f64, String)> {
    let t = text.trim().trim_end_matches(',');
    let parts: Vec<&str> = t
        .split(|c: char| c == '/' || c == '-' || c == '.' || c == ' ' || c == ',')
        .filter(|p| !p.is_empty())
        .collect();
    if parts.len() < 2 || parts.len() > 3 {
        return None;
    }
    let sep = if t.contains('/') {
        "/"
    } else if t.contains('-') {
        "-"
    } else if t.contains('.') && !t.contains(' ') {
        "."
    } else {
        " "
    };
    let is_num = |p: &str| p.chars().all(|c| c.is_ascii_digit());
    let current_year = crate::datetime::civil_from_days(
        (std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0)
            / 86_400) as i64,
    )
    .0;

    let named = parts.iter().position(|p| !is_num(p));
    let (y, m, d, format) = match named {
        None => {
            let nums: Vec<i64> = parts.iter().map(|p| p.parse().unwrap_or(-1)).collect();
            if parts[0].len() == 4 {
                // ISO-style year first.
                if parts.len() != 3 {
                    return None;
                }
                (nums[0], nums[1], nums[2], format!("yyyy{sep}mm{sep}dd"))
            } else if parts.len() == 2 {
                match order {
                    DateOrder::Mdy => (current_year, nums[0], nums[1], format!("m{sep}d")),
                    _ => (current_year, nums[1], nums[0], format!("d{sep}m")),
                }
            } else {
                let year = expand_year(nums[2], parts[2].len());
                let ylen = if parts[2].len() <= 2 { "yy" } else { "yyyy" };
                match order {
                    DateOrder::Mdy => (year, nums[0], nums[1], format!("m{sep}d{sep}{ylen}")),
                    DateOrder::Ymd => {
                        let year = expand_year(nums[0], parts[0].len());
                        (year, nums[1], nums[2], format!("yy{sep}mm{sep}dd"))
                    }
                    DateOrder::Dmy => (year, nums[1], nums[0], format!("dd{sep}mm{sep}{ylen}")),
                }
            }
        }
        Some(i) => {
            let month = month_from_name(parts[i])? as i64;
            let others: Vec<&str> = parts.iter().enumerate().filter(|(j, _)| *j != i).map(|(_, p)| *p).collect();
            if others.iter().any(|p| !is_num(p)) {
                return None;
            }
            let mmm = if parts[i].len() > 3 { "mmmm" } else { "mmm" };
            match (i, others.len()) {
                // 13 Oct 2026 / 13-Oct-26
                (1, 2) => {
                    let year = expand_year(others[1].parse().ok()?, others[1].len());
                    let ylen = if others[1].len() <= 2 { "yy" } else { "yyyy" };
                    (year, month, others[0].parse().ok()?, format!("d{sep}{mmm}{sep}{ylen}"))
                }
                // Oct 13, 2026
                (0, 2) => {
                    let year = expand_year(others[1].parse().ok()?, others[1].len());
                    (year, month, others[0].parse().ok()?, format!("{mmm} d, yyyy"))
                }
                // 13 Oct
                (1, 1) => (current_year, month, others[0].parse().ok()?, format!("d{sep}{mmm}")),
                // Oct 2026 / Oct-26 (a month and year)
                (0, 1) if others[0].len() == 4 || sep == "-" => {
                    let year = expand_year(others[0].parse().ok()?, others[0].len());
                    (year, month, 1, format!("{mmm}{sep}yy"))
                }
                (0, 1) => (current_year, month, others[0].parse().ok()?, format!("{mmm} d")),
                _ => return None,
            }
        }
    };
    if !(1..=12).contains(&m) || d < 1 || d > crate::datetime::days_in_month(y, m as u32) as i64 {
        return None;
    }
    let serial = date_to_serial(y, m, d, date1904)?;
    Some((serial, format))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn opts(order: DateOrder) -> FormatOptions {
        FormatOptions {
            date_order: order,
            date1904: false,
        }
    }

    #[test]
    fn typed_values() {
        let o = opts(DateOrder::Dmy);
        assert_eq!(parse_input("", &o), Input::Empty);
        assert_eq!(parse_input("=SUM(A1:A3)", &o), Input::Formula("SUM(A1:A3)".into()));
        assert_eq!(parse_input("=", &o), Input::Text("=".into()));
        assert_eq!(parse_input("'0123", &o), Input::Text("0123".into()));
        assert_eq!(parse_input("0123", &o), Input::Number(123.0, None));
        assert_eq!(parse_input("true", &o), Input::Bool(true));
        assert_eq!(parse_input("#N/A", &o), Input::Error(CellError::NA));
        assert_eq!(parse_input("12.5%", &o), Input::Number(0.125, Some("0.0%".into())));
        assert_eq!(parse_input("₹1,500", &o), Input::Number(1500.0, Some("\"₹\"#,##0".into())));
        assert_eq!(parse_input("1,234.50", &o), Input::Number(1234.5, Some("#,##0.00".into())));
        assert_eq!(parse_input("hello", &o), Input::Text("hello".into()));
        assert_eq!(parse_input(" 42 ", &o), Input::Number(42.0, None));
    }

    #[test]
    fn dates_follow_the_locale_order() {
        let dmy = opts(DateOrder::Dmy);
        let mdy = opts(DateOrder::Mdy);
        assert_eq!(parse_input("13/10/2026", &dmy), Input::Number(46_308.0, Some("dd/mm/yyyy".into())));
        assert_eq!(parse_input("10/13/2026", &mdy), Input::Number(46_308.0, Some("m/d/yyyy".into())));
        // 13 is not a month, so in the US order it stays text.
        assert_eq!(parse_input("13/10/2026", &mdy), Input::Text("13/10/2026".into()));
        assert_eq!(parse_input("2026-10-13", &mdy), Input::Number(46_308.0, Some("yyyy-mm-dd".into())));
        assert_eq!(parse_input("13-Oct-2026", &mdy), Input::Number(46_308.0, Some("d-mmm-yyyy".into())));
        assert_eq!(parse_input("13 October 2026", &dmy), Input::Number(46_308.0, Some("d mmmm yyyy".into())));
        assert_eq!(parse_input("Oct 13, 2026", &dmy), Input::Number(46_308.0, Some("mmm d, yyyy".into())));
        assert_eq!(parse_input("13/10/26", &dmy), Input::Number(46_308.0, Some("dd/mm/yy".into())));
        assert_eq!(parse_input("31/02/2026", &dmy), Input::Text("31/02/2026".into()));
    }

    #[test]
    fn times() {
        let o = opts(DateOrder::Dmy);
        assert_eq!(parse_input("14:30", &o), Input::Number(14.5 / 24.0, Some("h:mm".into())));
        assert_eq!(parse_input("2:30 pm", &o), Input::Number(14.5 / 24.0, Some("h:mm AM/PM".into())));
        assert_eq!(parse_input("12:00 AM", &o), Input::Number(0.0, Some("h:mm AM/PM".into())));
        assert_eq!(parse_input("9 PM", &o), Input::Number(21.0 / 24.0, Some("h:mm AM/PM".into())));
        assert_eq!(parse_input("25:00", &o), Input::Number(25.0 / 24.0, Some("h:mm".into())));
        assert_eq!(parse_input("12:61", &o), Input::Text("12:61".into()));
        match parse_input("13/10/2026 18:00", &o) {
            Input::Number(n, Some(f)) => {
                assert_eq!(n, 46_308.75);
                assert_eq!(f, "dd/mm/yyyy h:mm");
            }
            other => panic!("{other:?}"),
        }
    }
}
