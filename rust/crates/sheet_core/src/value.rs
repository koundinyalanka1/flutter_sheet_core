//! Cell values and Excel's rules for converting and comparing them.

use std::cmp::Ordering;
use std::sync::Arc;

/// An error value, as Excel shows them.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum CellError {
    Null,
    Div0,
    Value,
    Ref,
    Name,
    Num,
    NA,
    GettingData,
    Spill,
    Calc,
}

impl CellError {
    pub fn as_str(self) -> &'static str {
        match self {
            CellError::Null => "#NULL!",
            CellError::Div0 => "#DIV/0!",
            CellError::Value => "#VALUE!",
            CellError::Ref => "#REF!",
            CellError::Name => "#NAME?",
            CellError::Num => "#NUM!",
            CellError::NA => "#N/A",
            CellError::GettingData => "#GETTING_DATA",
            CellError::Spill => "#SPILL!",
            CellError::Calc => "#CALC!",
        }
    }

    pub fn parse(text: &str) -> Option<CellError> {
        Some(match text.to_ascii_uppercase().as_str() {
            "#NULL!" => CellError::Null,
            "#DIV/0!" => CellError::Div0,
            "#VALUE!" => CellError::Value,
            "#REF!" => CellError::Ref,
            "#NAME?" => CellError::Name,
            "#NUM!" => CellError::Num,
            "#N/A" => CellError::NA,
            "#GETTING_DATA" => CellError::GettingData,
            "#SPILL!" => CellError::Spill,
            "#CALC!" => CellError::Calc,
            _ => return None,
        })
    }

    /// The number ERROR.TYPE returns.
    pub fn type_number(self) -> f64 {
        match self {
            CellError::Null => 1.0,
            CellError::Div0 => 2.0,
            CellError::Value => 3.0,
            CellError::Ref => 4.0,
            CellError::Name => 5.0,
            CellError::Num => 6.0,
            CellError::NA => 7.0,
            CellError::GettingData => 8.0,
            CellError::Spill => 9.0,
            CellError::Calc => 14.0,
        }
    }

    /// The code BIFF8 (`.xls`) files store for this error.
    pub fn biff_code(self) -> u8 {
        match self {
            CellError::Null => 0x00,
            CellError::Div0 => 0x07,
            CellError::Value => 0x0F,
            CellError::Ref => 0x17,
            CellError::Name => 0x1D,
            CellError::Num => 0x24,
            CellError::NA => 0x2A,
            CellError::GettingData => 0x2B,
            CellError::Spill | CellError::Calc => 0x0F,
        }
    }

    pub fn from_biff_code(code: u8) -> CellError {
        match code {
            0x00 => CellError::Null,
            0x07 => CellError::Div0,
            0x17 => CellError::Ref,
            0x1D => CellError::Name,
            0x24 => CellError::Num,
            0x2A => CellError::NA,
            0x2B => CellError::GettingData,
            _ => CellError::Value,
        }
    }
}

impl std::fmt::Display for CellError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

/// What a cell holds (or a formula produced).
#[derive(Clone, Debug, PartialEq, Default)]
pub enum Value {
    #[default]
    Empty,
    Number(f64),
    Text(Arc<str>),
    Bool(bool),
    Error(CellError),
}

impl Value {
    pub fn text(s: impl Into<Arc<str>>) -> Value {
        Value::Text(s.into())
    }

    pub fn is_empty(&self) -> bool {
        matches!(self, Value::Empty)
    }

    pub fn as_error(&self) -> Option<CellError> {
        match self {
            Value::Error(e) => Some(*e),
            _ => None,
        }
    }

    /// Convert to a number as arithmetic does: booleans are 1 and 0, empty
    /// is 0, and text must read as a number.
    pub fn to_number(&self) -> Result<f64, CellError> {
        match self {
            Value::Number(n) => Ok(*n),
            Value::Bool(b) => Ok(if *b { 1.0 } else { 0.0 }),
            Value::Empty => Ok(0.0),
            Value::Text(t) => parse_number(t).ok_or(CellError::Value),
            Value::Error(e) => Err(*e),
        }
    }

    /// Convert to text as `&` does.
    pub fn to_text(&self) -> Result<Arc<str>, CellError> {
        match self {
            Value::Text(t) => Ok(t.clone()),
            Value::Number(n) => Ok(number_to_text(*n).into()),
            Value::Bool(b) => Ok(if *b { "TRUE" } else { "FALSE" }.into()),
            Value::Empty => Ok("".into()),
            Value::Error(e) => Err(*e),
        }
    }

    /// Convert to a boolean as IF and AND do.
    pub fn to_bool(&self) -> Result<bool, CellError> {
        match self {
            Value::Bool(b) => Ok(*b),
            Value::Number(n) => Ok(*n != 0.0),
            Value::Empty => Ok(false),
            Value::Text(t) => match t.to_ascii_uppercase().as_str() {
                "TRUE" => Ok(true),
                "FALSE" => Ok(false),
                _ => Err(CellError::Value),
            },
            Value::Error(e) => Err(*e),
        }
    }

    fn type_rank(&self) -> u8 {
        match self {
            Value::Empty | Value::Number(_) => 0,
            Value::Text(_) => 1,
            Value::Bool(_) => 2,
            Value::Error(_) => 3,
        }
    }
}

/// Round to the 15 significant digits Excel keeps.
pub fn round15(x: f64) -> f64 {
    if x == 0.0 || !x.is_finite() {
        return x;
    }
    format!("{x:.14e}").parse().unwrap_or(x)
}

/// Compare two values as Excel's comparison operators do: numbers before
/// text before booleans, text without regard to case, numbers to 15
/// significant digits, and an empty cell as 0, "" or FALSE to suit the
/// other side. Errors must be handled by the caller.
pub fn compare(a: &Value, b: &Value) -> Ordering {
    let (a, b) = match (a, b) {
        (Value::Empty, Value::Text(_)) => (&Value::Text("".into()), b),
        (Value::Text(_), Value::Empty) => (a, &Value::Text("".into())),
        (Value::Empty, Value::Bool(_)) => (&Value::Bool(false), b),
        (Value::Bool(_), Value::Empty) => (a, &Value::Bool(false)),
        _ => (a, b),
    };
    match (a, b) {
        (Value::Text(x), Value::Text(y)) => compare_text(x, y),
        (Value::Bool(x), Value::Bool(y)) => x.cmp(y),
        _ if a.type_rank() == 0 && b.type_rank() == 0 => {
            let x = round15(a.to_number().unwrap_or(0.0));
            let y = round15(b.to_number().unwrap_or(0.0));
            x.partial_cmp(&y).unwrap_or(Ordering::Equal)
        }
        _ => a.type_rank().cmp(&b.type_rank()),
    }
}

/// Case-insensitive text order.
pub fn compare_text(a: &str, b: &str) -> Ordering {
    let mut x = a.chars().flat_map(char::to_lowercase);
    let mut y = b.chars().flat_map(char::to_lowercase);
    loop {
        match (x.next(), y.next()) {
            (None, None) => return Ordering::Equal,
            (None, Some(_)) => return Ordering::Less,
            (Some(_), None) => return Ordering::Greater,
            (Some(p), Some(q)) => match p.cmp(&q) {
                Ordering::Equal => continue,
                other => return other,
            },
        }
    }
}

/// Text as a number, the way arithmetic and VALUE read it: optional sign,
/// thousands separators, a decimal point, an exponent, a trailing percent,
/// a leading currency symbol, or parentheses for a negative.
pub fn parse_number(text: &str) -> Option<f64> {
    let mut s = text.trim();
    if s.is_empty() {
        return None;
    }
    let mut negative = false;
    if s.starts_with('(') && s.ends_with(')') && s.len() > 2 {
        negative = true;
        s = s[1..s.len() - 1].trim();
    }
    let mut sign_seen = false;
    if let Some(rest) = s.strip_prefix('-') {
        negative = !negative;
        sign_seen = true;
        s = rest.trim_start();
    } else if let Some(rest) = s.strip_prefix('+') {
        sign_seen = true;
        s = rest.trim_start();
    }
    for symbol in ['$', '₹', '€', '£', '¥'] {
        if let Some(rest) = s.strip_prefix(symbol) {
            s = rest.trim_start();
            if !sign_seen {
                if let Some(rest) = s.strip_prefix('-') {
                    negative = !negative;
                    s = rest.trim_start();
                }
            }
            break;
        }
    }
    let mut percent = 0;
    while let Some(rest) = s.strip_suffix('%') {
        percent += 1;
        s = rest.trim_end();
    }
    if s.is_empty() {
        return None;
    }
    // Thousands separators must sit between groups of three digits.
    let cleaned: String = if s.contains(',') {
        let (int_part, rest) = match s.find(['.', 'e', 'E']) {
            Some(i) => (&s[..i], &s[i..]),
            None => (s, ""),
        };
        let groups: Vec<&str> = int_part.split(',').collect();
        let valid = groups.len() > 1
            && !groups[0].is_empty()
            && groups[0].len() <= 3
            && groups[1..].iter().all(|g| g.len() == 3);
        if !valid || rest.contains(',') {
            return None;
        }
        format!("{}{}", groups.concat(), rest)
    } else {
        s.to_owned()
    };
    let bytes = cleaned.as_bytes();
    let valid_chars = bytes.iter().enumerate().all(|(i, &b)| {
        b.is_ascii_digit()
            || matches!(b, b'.' | b'e' | b'E')
            // A sign only right after the exponent marker.
            || (matches!(b, b'+' | b'-') && i > 0 && matches!(bytes[i - 1], b'e' | b'E'))
    });
    if !valid_chars || !bytes.iter().any(u8::is_ascii_digit) || bytes[0] == b'e' || bytes[0] == b'E' {
        return None;
    }
    let mut value: f64 = cleaned.parse().ok()?;
    if !value.is_finite() {
        return None;
    }
    for _ in 0..percent {
        value /= 100.0;
    }
    Some(if negative { -value } else { value })
}

/// A number as text the way Excel converts it (`=A1&""`): up to 15
/// significant digits, no trailing zeros, scientific notation for very
/// large or very small magnitudes.
pub fn number_to_text(n: f64) -> String {
    if n == 0.0 {
        return "0".into();
    }
    if !n.is_finite() {
        return "#NUM!".into();
    }
    let rounded = round15(n);
    let exponent = rounded.abs().log10().floor() as i32;
    if exponent >= 15 || exponent < -9 {
        let s = format!("{rounded:.14E}");
        let (mantissa, exp) = s.split_once('E').unwrap_or((&s, "0"));
        let mantissa = trim_fraction(mantissa);
        let exp: i32 = exp.parse().unwrap_or(0);
        return format!("{mantissa}E{}{:02}", if exp < 0 { '-' } else { '+' }, exp.abs());
    }
    let decimals = (14 - exponent).clamp(0, 30) as usize;
    trim_fraction(&format!("{rounded:.decimals$}")).to_owned()
}

fn trim_fraction(s: &str) -> &str {
    if s.contains('.') {
        s.trim_end_matches('0').trim_end_matches('.')
    } else {
        s
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn errors_round_trip() {
        for e in [
            CellError::Null,
            CellError::Div0,
            CellError::Value,
            CellError::Ref,
            CellError::Name,
            CellError::Num,
            CellError::NA,
            CellError::Spill,
            CellError::Calc,
        ] {
            assert_eq!(CellError::parse(e.as_str()), Some(e));
        }
        assert_eq!(CellError::parse("#n/a"), Some(CellError::NA));
        assert_eq!(CellError::from_biff_code(0x07), CellError::Div0);
    }

    #[test]
    fn numbers_from_text() {
        let cases = [
            ("12", 12.0),
            (" 3.5 ", 3.5),
            ("-4", -4.0),
            ("+4", 4.0),
            ("1,234,567.5", 1234567.5),
            ("12%", 0.12),
            ("1e3", 1000.0),
            ("1.5E-2", 0.015),
            ("$100", 100.0),
            ("₹ 2,500", 2500.0),
            ("(42)", -42.0),
            ("-$5", -5.0),
            ("$-5", -5.0),
            (".5", 0.5),
        ];
        for (text, expected) in cases {
            assert_eq!(parse_number(text), Some(expected), "{text}");
        }
        for bad in ["", "abc", "1,23", "12,3456", "1.2.3", "e5", "--1", "$", "%", "1 2", "TRUE", "1,234,56"] {
            assert_eq!(parse_number(bad), None, "{bad}");
        }
    }

    #[test]
    fn numbers_to_text() {
        let cases = [
            (0.0, "0"),
            (1.0, "1"),
            (-2.5, "-2.5"),
            (1.0 / 3.0, "0.333333333333333"),
            (2.0 / 3.0, "0.666666666666667"),
            (0.1 + 0.2, "0.3"),
            (123456789012345.0, "123456789012345"),
            (1e15, "1E+15"),
            (1.5e20, "1.5E+20"),
            (0.00000001, "0.00000001"),
            (1e-10, "1E-10"),
            (100.25, "100.25"),
        ];
        for (n, expected) in cases {
            assert_eq!(number_to_text(n), expected, "{n}");
        }
    }

    #[test]
    fn excel_comparison_rules() {
        let n = |x: f64| Value::Number(x);
        let t = |s: &str| Value::text(s);
        assert_eq!(compare(&n(1.0), &n(2.0)), Ordering::Less);
        assert_eq!(compare(&n(0.1 + 0.2), &n(0.3)), Ordering::Equal);
        assert_eq!(compare(&t("apple"), &t("APPLE")), Ordering::Equal);
        assert_eq!(compare(&t("a"), &t("B")), Ordering::Less);
        assert_eq!(compare(&n(1e9), &t("0")), Ordering::Less);
        assert_eq!(compare(&t("zzz"), &Value::Bool(false)), Ordering::Less);
        assert_eq!(compare(&Value::Empty, &n(0.0)), Ordering::Equal);
        assert_eq!(compare(&Value::Empty, &t("")), Ordering::Equal);
        assert_eq!(compare(&Value::Empty, &Value::Bool(false)), Ordering::Equal);
    }

    #[test]
    fn coercions() {
        assert_eq!(Value::text(" 7 ").to_number(), Ok(7.0));
        assert_eq!(Value::text("x").to_number(), Err(CellError::Value));
        assert_eq!(Value::Bool(true).to_number(), Ok(1.0));
        assert_eq!(Value::Number(2.0).to_text().unwrap().as_ref(), "2");
        assert_eq!(Value::text("true").to_bool(), Ok(true));
        assert_eq!(Value::Number(0.0).to_bool(), Ok(false));
        assert_eq!(Value::Error(CellError::NA).to_number(), Err(CellError::NA));
    }
}
