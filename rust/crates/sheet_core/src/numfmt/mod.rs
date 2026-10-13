//! Excel number formats: parsing format codes and formatting values.
//!
//! Covers the whole format-code language as Excel displays cells:
//! up to four sections (positive; negative; zero; text) and bracketed
//! conditions, colours, locale currency tags and elapsed-time fields;
//! digit placeholders `0 # ?`, decimal points, thousands separators and
//! scaling commas, percent, scientific notation, fractions; dates and times
//! with 12/24-hour clocks and fractional seconds; literal text in quotes,
//! escapes, `_x` spacing and `*x` fills; `@` for text; and "General".
//!
//! Numbers are rounded the way Excel displays them: to 15 significant
//! digits first, then half away from zero at the shown precision.

mod parse;

use std::collections::HashMap;
use std::sync::{Arc, Mutex, OnceLock};

use crate::datetime;
use crate::value::{number_to_text, Value};
pub use parse::{AmPm, Section, Token};

/// The order the locale writes dates in, for the built-in short-date format.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum DateOrder {
    /// 13-10-2026 (India, the UK, most of the world).
    #[default]
    Dmy,
    /// 10/13/2026 (the United States).
    Mdy,
    /// 2026-10-13 (ISO, East Asia).
    Ymd,
}

/// Locale-dependent choices when formatting.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub struct FormatOptions {
    pub date_order: DateOrder,
    pub date1904: bool,
}

/// A formatted value and the colour its format asks for, if any.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Formatted {
    pub text: String,
    /// `0xRRGGBB`.
    pub color: Option<u32>,
}

impl Formatted {
    fn plain(text: impl Into<String>) -> Self {
        Formatted {
            text: text.into(),
            color: None,
        }
    }
}

/// A parsed format code.
#[derive(Clone, Debug)]
pub struct NumberFormat {
    sections: Vec<Section>,
}

fn cache() -> &'static Mutex<HashMap<String, Arc<NumberFormat>>> {
    static CACHE: OnceLock<Mutex<HashMap<String, Arc<NumberFormat>>>> = OnceLock::new();
    CACHE.get_or_init(Default::default)
}

impl NumberFormat {
    pub fn parse(code: &str) -> NumberFormat {
        NumberFormat {
            sections: parse::parse_sections(code),
        }
    }

    /// Parse with a small process-wide cache: a grid formats thousands of
    /// cells with a handful of codes.
    pub fn cached(code: &str) -> Arc<NumberFormat> {
        let mut map = cache().lock().unwrap_or_else(|e| e.into_inner());
        if let Some(found) = map.get(code) {
            return found.clone();
        }
        if map.len() > 512 {
            map.clear();
        }
        let parsed = Arc::new(NumberFormat::parse(code));
        map.insert(code.to_owned(), parsed.clone());
        parsed
    }

    /// Whether the format shows numbers as dates or times.
    pub fn is_date(&self) -> bool {
        self.sections.first().is_some_and(|s| s.is_date)
    }

    /// Whether the format is `@`: entries are kept as text.
    pub fn is_text(&self) -> bool {
        self.sections.len() == 1 && self.sections[0].tokens.iter().any(|t| matches!(t, Token::At))
    }

    pub fn is_percent(&self) -> bool {
        self.sections.first().is_some_and(|s| s.percent > 0)
    }

    pub fn is_general(&self) -> bool {
        self.sections.first().is_none_or(|s| s.tokens.iter().all(|t| matches!(t, Token::General)))
    }

    /// Decimal places the first section shows (for "increase decimals").
    pub fn decimals(&self) -> usize {
        self.sections.first().map_or(0, |s| s.decimals())
    }

    /// Format a value.
    pub fn format(&self, value: &Value, options: &FormatOptions) -> Formatted {
        match value {
            Value::Empty => Formatted::plain(""),
            Value::Bool(b) => Formatted::plain(if *b { "TRUE" } else { "FALSE" }),
            Value::Error(e) => Formatted::plain(e.as_str()),
            Value::Text(t) => self.format_text(t),
            Value::Number(n) => self.format_number(*n, options),
        }
    }

    fn format_text(&self, text: &str) -> Formatted {
        // The fourth section, or any section that has `@`, formats text.
        let section = if self.sections.len() >= 4 {
            Some(&self.sections[3])
        } else {
            self.sections.iter().find(|s| s.tokens.iter().any(|t| matches!(t, Token::At)))
        };
        let Some(section) = section else {
            return Formatted::plain(text);
        };
        let mut out = String::new();
        for token in &section.tokens {
            match token {
                Token::At => out.push_str(text),
                Token::Literal(l) => out.push_str(l),
                Token::Space(_) => out.push(' '),
                _ => {}
            }
        }
        Formatted {
            text: out,
            color: section.color,
        }
    }

    /// Choose the section for a number: conditions if any, otherwise
    /// positive; negative; zero. Returns the section and whether a minus
    /// sign must be written (the section does not express negativity).
    fn pick(&self, n: f64) -> (&Section, bool) {
        let numeric: Vec<&Section> = self.sections.iter().take(3).collect();
        if numeric.is_empty() {
            return (&parse::GENERAL_SECTION, n < 0.0);
        }
        if numeric.iter().any(|s| s.condition.is_some()) {
            // A section chosen by a condition that targets negatives ([<0])
            // shows them as written; any other shows a minus sign.
            for section in &numeric {
                match section.condition {
                    Some(cond) if cond.matches(n) => return (section, n < 0.0 && !cond.op_is_negative()),
                    Some(_) => continue,
                    None => return (section, n < 0.0),
                }
            }
            // Excel shows an overflow when nothing matches.
            return (&parse::HASHES_SECTION, false);
        }
        match (numeric.len(), n) {
            (1, _) => (numeric[0], n < 0.0),
            (_, n) if n > 0.0 => (numeric[0], false),
            (_, n) if n < 0.0 => (numeric[1], false),
            (2, _) => (numeric[0], false),
            (_, _) => (numeric[2], false),
        }
    }

    fn format_number(&self, n: f64, options: &FormatOptions) -> Formatted {
        if !n.is_finite() {
            return Formatted::plain("#NUM!");
        }
        let (section, minus) = self.pick(n);
        let text = if section.is_date {
            format_date(section, n, options)
        } else if section.tokens.iter().all(|t| matches!(t, Token::General | Token::Literal(_) | Token::Space(_))) {
            let mut out = String::new();
            for token in &section.tokens {
                match token {
                    Token::General => out.push_str(&general(n.abs())),
                    Token::Literal(l) => out.push_str(l),
                    Token::Space(_) => out.push(' '),
                    _ => {}
                }
            }
            if minus && n != 0.0 {
                out.insert(0, '-');
            }
            out
        } else {
            format_numeric(section, n.abs(), minus)
        };
        Formatted {
            text,
            color: section.color,
        }
    }
}

/// Format a value with a format code (cached parse).
pub fn format_value(value: &Value, code: &str, options: &FormatOptions) -> Formatted {
    NumberFormat::cached(code).format(value, options)
}

/// Excel's "General" display of a non-negative number in a standard-width
/// cell: up to 11 characters, switching to scientific notation for very
/// large or very small magnitudes.
pub fn general(n: f64) -> String {
    let a = n.abs();
    if a == 0.0 {
        return "0".into();
    }
    if a >= 1e11 || a < 1e-9 {
        let s = format!("{a:.5E}");
        let (mantissa, exp) = s.split_once('E').unwrap_or((&s, "0"));
        let mantissa = mantissa.trim_end_matches('0').trim_end_matches('.');
        let exp: i32 = exp.parse().unwrap_or(0);
        return format!("{mantissa}E{}{:02}", if exp < 0 { '-' } else { '+' }, exp.abs());
    }
    let int_digits = if a >= 1.0 { a.log10().floor() as i32 + 1 } else { 1 };
    let decimals = (10 - int_digits).max(0) as usize;
    let rounded = round_half_away(a, decimals as i32);
    let s = format!("{rounded:.decimals$}");
    let s = if s.contains('.') { s.trim_end_matches('0').trim_end_matches('.').to_owned() } else { s };
    if s.len() > 11 {
        // Rounding carried into a new digit; fall back to text conversion.
        return number_to_text(a);
    }
    s
}

/// Round half away from zero at `decimals` places, after first rounding to
/// the 15 significant digits Excel keeps, so 2.675 shows as 2.68.
pub fn round_half_away(x: f64, decimals: i32) -> f64 {
    let x = crate::value::round15(x);
    let digits = decimal_digits(x.abs());
    let (mantissa, exp) = digits;
    // mantissa: significant digits; value = 0.mantissa * 10^exp
    let keep = exp + decimals;
    if keep < 0 {
        return 0.0f64.copysign(x);
    }
    let keep = keep as usize;
    if keep >= mantissa.len() {
        return x;
    }
    let mut kept: Vec<u8> = mantissa[..keep].to_vec();
    let round_up = mantissa[keep] >= b'5';
    let mut exp = exp;
    if round_up {
        let mut i = kept.len();
        loop {
            if i == 0 {
                kept.insert(0, b'1');
                exp += 1;
                break;
            }
            i -= 1;
            if kept[i] == b'9' {
                kept[i] = b'0';
            } else {
                kept[i] += 1;
                break;
            }
        }
    }
    if kept.is_empty() {
        return 0.0f64.copysign(x);
    }
    let text = format!("0.{}e{}", String::from_utf8_lossy(&kept), exp);
    let value: f64 = text.parse().unwrap_or(x);
    value.copysign(x)
}

/// The significant digits of a positive number (the 15 Excel keeps) and
/// its decimal exponent, such that `x = 0.d1d2d3... * 10^exp`.
fn decimal_digits(x: f64) -> (Vec<u8>, i32) {
    if x == 0.0 {
        return (vec![b'0'], 1);
    }
    let s = format!("{x:.14e}");
    let (mantissa, exp) = s.split_once('e').unwrap_or((&s, "0"));
    let exp: i32 = exp.parse().unwrap_or(0);
    let digits: Vec<u8> = mantissa.bytes().filter(u8::is_ascii_digit).collect();
    let trimmed_len = digits.iter().rposition(|&d| d != b'0').map_or(1, |p| p + 1);
    (digits[..trimmed_len].to_vec(), exp + 1)
}

/// The digits of `x` rounded to `decimals` places: (integer digits,
/// fraction digits), no sign, integer part "0" when below one.
fn rounded_digits(x: f64, decimals: usize) -> (String, String) {
    let r = round_half_away(x, decimals as i32);
    let s = format!("{r:.decimals$}");
    match s.split_once('.') {
        Some((i, f)) => (i.to_owned(), f.to_owned()),
        None => (s, String::new()),
    }
}

fn format_numeric(section: &Section, value: f64, minus: bool) -> String {
    let mut v = value;
    for _ in 0..section.percent {
        v *= 100.0;
    }
    for _ in 0..section.scale {
        v /= 1000.0;
    }
    if section.fraction.is_some() {
        return format_fraction(section, v, minus);
    }
    let (int_tokens, frac_tokens, exp_tokens) = section.split_parts();
    let frac_places = frac_tokens.iter().filter(|t| t.is_digit()).count();

    // Scientific notation: choose the exponent first.
    let mut exponent: Option<i32> = None;
    if section.exponent.is_some() {
        let int_places = int_tokens.iter().filter(|t| t.is_digit()).count().max(1) as i32;
        let e = if v == 0.0 { 0 } else { v.log10().floor() as i32 };
        let e = if int_tokens.iter().any(|t| matches!(t, Token::Hash)) && int_places > 1 {
            e.div_euclid(int_places) * int_places
        } else {
            e - (int_places - 1)
        };
        v /= 10f64.powi(e);
        // Rounding the mantissa can carry into another digit.
        let (i, _) = rounded_digits(v, frac_places);
        let e = if i.len() as i32 > int_places && !int_tokens.iter().any(|t| matches!(t, Token::Hash)) {
            v /= 10.0;
            e + 1
        } else {
            e
        };
        exponent = Some(e);
    }

    let (int_digits, frac_digits) = rounded_digits(v, frac_places);
    let all_zero = int_digits.bytes().all(|b| b == b'0') && frac_digits.bytes().all(|b| b == b'0');
    let mut out = String::new();
    if minus && !all_zero {
        out.push('-');
    }
    out.push_str(&fill_integer(int_tokens, &int_digits, section.thousands));
    out.push_str(&fill_fraction(frac_tokens, &frac_digits));
    if let (Some(e), Some(spec)) = (exponent, section.exponent) {
        out.push(if spec.upper { 'E' } else { 'e' });
        if e < 0 {
            out.push('-');
        } else if spec.plus {
            out.push('+');
        }
        let width = exp_tokens.iter().filter(|t| t.is_digit()).count();
        let digits = e.abs().to_string();
        for _ in digits.len()..width {
            out.push('0');
        }
        out.push_str(&digits);
        for token in exp_tokens.iter().filter(|t| !t.is_digit()) {
            push_literal(&mut out, token);
        }
    }
    out
}

fn push_literal(out: &mut String, token: &Token) {
    match token {
        Token::Literal(l) => out.push_str(l),
        Token::Space(_) => out.push(' '),
        _ => {}
    }
}

/// Fill integer placeholders from the right with `digits`; extra digits go
/// to the leftmost placeholder. With `thousands`, digits are grouped by
/// three.
fn fill_integer(tokens: &[Token], digits: &str, thousands: bool) -> String {
    let places = tokens.iter().filter(|t| t.is_digit()).count();
    // A lone "0" for a value below one is shown only where a 0 placeholder asks.
    let digits = if digits == "0" { "" } else { digits };
    if thousands {
        // Grouping applies to the digit run as a whole; literals around the
        // placeholders stay at the ends.
        let zeros = tokens.iter().filter(|t| matches!(t, Token::Zero)).count();
        let questions = tokens.iter().filter(|t| matches!(t, Token::Question)).count();
        let mut body = digits.to_owned();
        while body.len() < zeros {
            body.insert(0, '0');
        }
        let mut grouped = String::new();
        for (i, c) in body.chars().enumerate() {
            if i > 0 && (body.len() - i) % 3 == 0 {
                grouped.push(',');
            }
            grouped.push(c);
        }
        let pad = (zeros + questions).saturating_sub(body.len());
        let mut out = String::new();
        let first_digit = tokens.iter().position(Token::is_digit).unwrap_or(0);
        let last_digit = tokens.iter().rposition(Token::is_digit).unwrap_or(0);
        for token in &tokens[..first_digit] {
            push_literal(&mut out, token);
        }
        for _ in 0..pad.min(questions) {
            out.push(' ');
        }
        out.push_str(&grouped);
        for token in tokens.iter().skip(last_digit + 1) {
            push_literal(&mut out, token);
        }
        return out;
    }
    let mut remaining: Vec<char> = digits.chars().collect();
    let mut pieces: Vec<String> = Vec::new();
    let mut seen = 0;
    for token in tokens.iter().rev() {
        if token.is_digit() {
            seen += 1;
            let piece = if seen == places && !remaining.is_empty() {
                // Leftmost placeholder takes every remaining digit.
                let all: String = remaining.drain(..).collect();
                all
            } else if let Some(d) = remaining.pop() {
                d.to_string()
            } else {
                match token {
                    Token::Zero => "0".into(),
                    Token::Question => " ".into(),
                    _ => String::new(),
                }
            };
            pieces.push(piece);
        } else {
            let mut s = String::new();
            push_literal(&mut s, token);
            pieces.push(s);
        }
    }
    if places == 0 && !remaining.is_empty() {
        pieces.push(remaining.drain(..).collect());
    }
    pieces.reverse();
    pieces.concat()
}

/// Fill fraction placeholders (and the decimal point) from the left.
fn fill_fraction(tokens: &[Token], digits: &str) -> String {
    let digit_chars: Vec<char> = digits.chars().collect();
    // Trailing zeros are shown only where a 0 placeholder asks for them.
    let last_significant = digit_chars.iter().rposition(|&c| c != '0');
    let mut out = String::new();
    let mut i = 0;
    for token in tokens {
        match token {
            Token::Point => out.push('.'),
            t if t.is_digit() => {
                let d = digit_chars.get(i).copied().unwrap_or('0');
                let significant = last_significant.is_some_and(|last| i <= last);
                match t {
                    Token::Zero => out.push(d),
                    Token::Hash if significant => out.push(d),
                    Token::Question => out.push(if significant { d } else { ' ' }),
                    _ => {}
                }
                i += 1;
            }
            other => push_literal(&mut out, other),
        }
    }
    out
}

fn gcd(a: u64, b: u64) -> u64 {
    if b == 0 {
        a
    } else {
        gcd(b, a % b)
    }
}

/// The closest fraction to `x` (0 <= x < 1 for mixed numbers) with a
/// denominator up to `max_den`.
fn best_fraction(x: f64, max_den: u64) -> (u64, u64) {
    let mut best = (0u64, 1u64);
    let mut best_err = f64::INFINITY;
    for den in 1..=max_den.max(1) {
        let num = (x * den as f64).round();
        let err = (x - num / den as f64).abs();
        if err < best_err - 1e-12 {
            best_err = err;
            best = (num as u64, den);
            if err == 0.0 {
                break;
            }
        }
    }
    let g = gcd(best.0, best.1).max(1);
    (best.0 / g, best.1 / g)
}

fn format_fraction(section: &Section, value: f64, minus: bool) -> String {
    let spec = section.fraction.expect("fraction section");
    let (whole_tokens, num_tokens, den_tokens) = section.fraction_parts();
    let has_whole = whole_tokens.iter().any(Token::is_digit);
    let (mut whole, mut frac) = if has_whole {
        (value.trunc(), value.fract())
    } else {
        (0.0, value)
    };
    let (mut num, den) = match spec.fixed_denominator {
        Some(d) => ((frac * d as f64).round() as u64, d),
        None => {
            let max_den = 10u64.pow(spec.denominator_digits.clamp(1, 7) as u32) - 1;
            if has_whole {
                best_fraction(frac, max_den)
            } else {
                let (n, d) = best_fraction(frac.fract(), max_den);
                (n + frac.trunc() as u64 * d, d)
            }
        }
    };
    if has_whole && num >= den && den > 0 {
        whole += (num / den) as f64;
        num %= den;
        frac = 0.0;
    }
    let _ = frac;
    let mut out = String::new();
    if minus && (whole != 0.0 || num != 0) {
        out.push('-');
    }
    if has_whole {
        let digits = format!("{whole:.0}");
        let show_whole = whole != 0.0 || num == 0;
        let digits = if show_whole { digits } else { String::new() };
        out.push_str(&fill_integer(whole_tokens, if digits.is_empty() { "0" } else { &digits }, false));
        if !show_whole && whole_tokens.iter().all(|t| !matches!(t, Token::Zero)) {
            // "# ?/?" with no whole number: no leading zero.
            out = out.trim_end().to_owned();
            if minus && num != 0 && !out.starts_with('-') {
                out.insert(0, '-');
            }
        }
    }
    if num == 0 && has_whole {
        // Excel blanks the fraction (keeping its width with ? placeholders).
        let width = num_tokens.len() + 1 + den_tokens.len();
        if num_tokens.iter().chain(den_tokens.iter()).any(|t| matches!(t, Token::Question)) {
            out.push_str(&" ".repeat(width));
        }
        if out.trim().is_empty() {
            return "0".into();
        }
        return out.trim_end().to_owned() + &" ".repeat(out.len() - out.trim_end().len());
    }
    out.push_str(&fill_integer(num_tokens, &num.to_string(), false));
    out.push('/');
    let den_text = den.to_string();
    let den_places = den_tokens.iter().filter(|t| t.is_digit()).count();
    out.push_str(&den_text);
    for _ in den_text.len()..den_places {
        out.push(' ');
    }
    out
}

fn format_date(section: &Section, n: f64, options: &FormatOptions) -> String {
    let elapsed = section.tokens.iter().any(|t| {
        matches!(
            t,
            Token::ElapsedHours(_) | Token::ElapsedMinutes(_) | Token::ElapsedSeconds(_)
        )
    });
    if n < 0.0 || (!elapsed && n > datetime::MAX_DATE_SERIAL + 0.999_999) {
        return "#".repeat(11);
    }
    let sub_digits = section
        .tokens
        .iter()
        .find_map(|t| match t {
            Token::SubSecond(d) => Some(*d as u32),
            _ => None,
        })
        .unwrap_or(0);
    // Round to the shown precision of seconds before splitting fields.
    let unit = 86_400.0 * 10f64.powi(sub_digits as i32);
    let ticks = (n * unit).round();
    let rounded = ticks / unit;
    let (y, mo, d) = datetime::serial_to_date(rounded, options.date1904).unwrap_or((1900, 1, 0));
    let total_seconds = (rounded * 86_400.0 * 10f64.powi(sub_digits as i32)).round();
    let scale = 10f64.powi(sub_digits as i32);
    let day_ticks = (rounded.fract() * 86_400.0 * scale).round() as u64;
    let secs_of_day = day_ticks / scale as u64;
    let sub = day_ticks % scale as u64;
    let (h, mi, s) = (secs_of_day / 3600, (secs_of_day / 60) % 60, secs_of_day % 60);
    let twelve = section.tokens.iter().any(|t| matches!(t, Token::AmPm(_)));
    let mut out = String::new();
    for token in &section.tokens {
        match token {
            Token::Year(len) => {
                if *len <= 2 {
                    out.push_str(&format!("{:02}", y.rem_euclid(100)));
                } else {
                    out.push_str(&format!("{y:04}"));
                }
            }
            Token::Month(len) => match len {
                1 => out.push_str(&mo.to_string()),
                2 => out.push_str(&format!("{mo:02}")),
                3 => out.push_str(&datetime::MONTH_NAMES[(mo - 1) as usize][..3]),
                4 => out.push_str(datetime::MONTH_NAMES[(mo - 1) as usize]),
                _ => out.push_str(&datetime::MONTH_NAMES[(mo - 1) as usize][..1]),
            },
            Token::Day(len) => match len {
                1 => out.push_str(&d.to_string()),
                2 => out.push_str(&format!("{d:02}")),
                _ => {
                    let wd = datetime::weekday(rounded, options.date1904).unwrap_or(1) as usize - 1;
                    let name = datetime::DAY_NAMES[wd];
                    out.push_str(if *len == 3 { &name[..3] } else { name });
                }
            },
            Token::Hour(len) => {
                let hour = if twelve {
                    match h % 12 {
                        0 => 12,
                        x => x,
                    }
                } else {
                    h
                };
                if *len >= 2 {
                    out.push_str(&format!("{hour:02}"));
                } else {
                    out.push_str(&hour.to_string());
                }
            }
            Token::Minute(len) => {
                if *len >= 2 {
                    out.push_str(&format!("{mi:02}"));
                } else {
                    out.push_str(&mi.to_string());
                }
            }
            Token::Second(len) => {
                if *len >= 2 {
                    out.push_str(&format!("{s:02}"));
                } else {
                    out.push_str(&s.to_string());
                }
            }
            Token::SubSecond(len) => {
                out.push('.');
                out.push_str(&format!("{:0width$}", sub, width = *len as usize));
            }
            Token::ElapsedHours(len) => {
                let hours = (total_seconds / scale / 3600.0).floor() as i64;
                out.push_str(&format!("{hours:0width$}", width = *len as usize));
            }
            Token::ElapsedMinutes(len) => {
                let minutes = (total_seconds / scale / 60.0).floor() as i64;
                out.push_str(&format!("{minutes:0width$}", width = *len as usize));
            }
            Token::ElapsedSeconds(len) => {
                let seconds = (total_seconds / scale).floor() as i64;
                out.push_str(&format!("{seconds:0width$}", width = *len as usize));
            }
            Token::AmPm(style) => {
                let pm = h >= 12;
                out.push_str(match (style, pm) {
                    (AmPm::Upper, false) => "AM",
                    (AmPm::Upper, true) => "PM",
                    (AmPm::Lower, false) => "am",
                    (AmPm::Lower, true) => "pm",
                    (AmPm::ShortUpper, false) => "A",
                    (AmPm::ShortUpper, true) => "P",
                    (AmPm::ShortLower, false) => "a",
                    (AmPm::ShortLower, true) => "p",
                });
            }
            Token::General => out.push_str(&general(n)),
            other => push_literal(&mut out, other),
        }
    }
    out
}

/// The code of a built-in number format id, as Excel shows it in this
/// locale. None for ids that are not built in.
pub fn builtin_format(id: u32, options: &FormatOptions) -> Option<String> {
    let short_date = match options.date_order {
        DateOrder::Dmy => "dd-mm-yyyy",
        DateOrder::Mdy => "m/d/yyyy",
        DateOrder::Ymd => "yyyy-mm-dd",
    };
    Some(
        match id {
            0 => "General",
            1 => "0",
            2 => "0.00",
            3 => "#,##0",
            4 => "#,##0.00",
            5 => "\"$\"#,##0_);(\"$\"#,##0)",
            6 => "\"$\"#,##0_);[Red](\"$\"#,##0)",
            7 => "\"$\"#,##0.00_);(\"$\"#,##0.00)",
            8 => "\"$\"#,##0.00_);[Red](\"$\"#,##0.00)",
            9 => "0%",
            10 => "0.00%",
            11 => "0.00E+00",
            12 => "# ?/?",
            13 => "# ??/??",
            14 => short_date,
            15 => "d-mmm-yy",
            16 => "d-mmm",
            17 => "mmm-yy",
            18 => "h:mm AM/PM",
            19 => "h:mm:ss AM/PM",
            20 => "h:mm",
            21 => "h:mm:ss",
            22 => return Some(format!("{short_date} h:mm")),
            37 => "#,##0 ;(#,##0)",
            38 => "#,##0 ;[Red](#,##0)",
            39 => "#,##0.00;(#,##0.00)",
            40 => "#,##0.00;[Red](#,##0.00)",
            41 => "_(* #,##0_);_(* (#,##0);_(* \"-\"_);_(@_)",
            42 => "_(\"$\"* #,##0_);_(\"$\"* (#,##0);_(\"$\"* \"-\"_);_(@_)",
            43 => "_(* #,##0.00_);_(* (#,##0.00);_(* \"-\"??_);_(@_)",
            44 => "_(\"$\"* #,##0.00_);_(\"$\"* (#,##0.00);_(\"$\"* \"-\"??_);_(@_)",
            45 => "mm:ss",
            46 => "[h]:mm:ss",
            47 => "mm:ss.0",
            48 => "##0.0E+0",
            49 => "@",
            _ => return None,
        }
        .to_owned(),
    )
}

/// The Indian lakh/crore grouping (12,34,56,789.00) as a format code Excel
/// understands: conditional sections place the commas.
pub const INDIAN_NUMBER_FORMAT: &str =
    "[>=10000000]##\\,##\\,##\\,##0.00;[>=100000]##\\,##\\,##0.00;##,##0.00";

/// The same with the rupee sign.
pub const INDIAN_RUPEE_FORMAT: &str =
    "[>=10000000]\"₹\"##\\,##\\,##\\,##0.00;[>=100000]\"₹\"##\\,##\\,##0.00;\"₹\"##,##0.00";

#[cfg(test)]
mod tests;
