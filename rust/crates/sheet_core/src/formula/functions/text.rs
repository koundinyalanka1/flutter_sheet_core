//! Text functions. Positions and lengths count characters.

use super::super::ast::Expr;
use super::super::eval::{Eval, Evaluator};
use super::criteria::wildcard_match;
use super::{tri_some, arity, lift, omitted, tri};
use crate::input::parse_date_time;
use crate::numfmt::{format_value, FormatOptions};
use crate::value::{parse_number, CellError, Value};

/// The longest text a cell holds.
const MAX_TEXT: usize = 32_767;

pub fn call(ev: &mut Evaluator, name: &str, args: &[Expr]) -> Option<Eval> {
    let options = ev.options();
    Some(match name {
        "LEN" => text1(ev, args, |t| Value::Number(t.chars().count() as f64)),
        "LOWER" => text1(ev, args, |t| Value::text(t.to_lowercase())),
        "UPPER" => text1(ev, args, |t| Value::text(t.to_uppercase())),
        "PROPER" => text1(ev, args, |t| Value::text(proper(t))),
        "TRIM" => text1(ev, args, |t| Value::text(t.split(' ').filter(|w| !w.is_empty()).collect::<Vec<_>>().join(" "))),
        "CLEAN" => text1(ev, args, |t| Value::text(t.chars().filter(|c| (*c as u32) >= 32).collect::<String>())),
        "T" => {
            if let Some(e) = arity(args, 1, 1) {
                return Some(e);
            }
            lift(ev, args, &mut |v| match &v[0] {
                Value::Text(_) | Value::Error(_) => v[0].clone(),
                _ => Value::text(""),
            })
        }
        "LEFT" | "RIGHT" => {
            let right = name == "RIGHT";
            if let Some(e) = arity(args, 1, 2) {
                return Some(e);
            }
            let given = args.len() == 2;
            lift(ev, args, &mut |v| {
                let t = match v[0].to_text() {
                    Ok(t) => t,
                    Err(e) => return Value::Error(e),
                };
                let n = if given {
                    match v[1].to_number() {
                        Ok(n) if n < 0.0 => return Value::Error(CellError::Value),
                        Ok(n) => n.trunc() as usize,
                        Err(e) => return Value::Error(e),
                    }
                } else {
                    1
                };
                let len = t.chars().count();
                let s: String = if right {
                    t.chars().skip(len.saturating_sub(n)).collect()
                } else {
                    t.chars().take(n).collect()
                };
                Value::text(s)
            })
        }
        "MID" => {
            if let Some(e) = arity(args, 3, 3) {
                return Some(e);
            }
            lift(ev, args, &mut |v| {
                let t = match v[0].to_text() {
                    Ok(t) => t,
                    Err(e) => return Value::Error(e),
                };
                let (start, n) = match (v[1].to_number(), v[2].to_number()) {
                    (Ok(s), Ok(n)) => (s.trunc(), n.trunc()),
                    (Err(e), _) | (_, Err(e)) => return Value::Error(e),
                };
                if start < 1.0 || n < 0.0 {
                    return Value::Error(CellError::Value);
                }
                Value::text(t.chars().skip(start as usize - 1).take(n as usize).collect::<String>())
            })
        }
        "CONCAT" => {
            let mut out = String::new();
            for a in args {
                for (v, _) in ev.flatten(a) {
                    match v.to_text() {
                        Ok(t) => out.push_str(&t),
                        Err(e) => return Some(Eval::error(e)),
                    }
                }
            }
            limited(out)
        }
        "CONCATENATE" => {
            if args.is_empty() {
                return Some(Eval::error(CellError::Value));
            }
            lift(ev, args, &mut |v| {
                let mut out = String::new();
                for x in v {
                    match x.to_text() {
                        Ok(t) => out.push_str(&t),
                        Err(e) => return Value::Error(e),
                    }
                }
                if out.chars().count() > MAX_TEXT {
                    Value::Error(CellError::Value)
                } else {
                    Value::text(out)
                }
            })
        }
        "TEXTJOIN" => text_join(ev, args),
        "REPT" => {
            if let Some(e) = arity(args, 2, 2) {
                return Some(e);
            }
            lift(ev, args, &mut |v| {
                let t = match v[0].to_text() {
                    Ok(t) => t,
                    Err(e) => return Value::Error(e),
                };
                match v[1].to_number() {
                    Ok(n) if n < 0.0 => Value::Error(CellError::Value),
                    Ok(n) => {
                        let n = n.trunc() as usize;
                        if t.chars().count().saturating_mul(n) > MAX_TEXT {
                            Value::Error(CellError::Value)
                        } else {
                            Value::text(t.repeat(n))
                        }
                    }
                    Err(e) => Value::Error(e),
                }
            })
        }
        "EXACT" => {
            if let Some(e) = arity(args, 2, 2) {
                return Some(e);
            }
            lift(ev, args, &mut |v| match (v[0].to_text(), v[1].to_text()) {
                (Ok(a), Ok(b)) => Value::Bool(a == b),
                (Err(e), _) | (_, Err(e)) => Value::Error(e),
            })
        }
        "FIND" | "SEARCH" => {
            let search = name == "SEARCH";
            if let Some(e) = arity(args, 2, 3) {
                return Some(e);
            }
            lift(ev, args, &mut |v| {
                let (needle, hay) = match (v[0].to_text(), v[1].to_text()) {
                    (Ok(a), Ok(b)) => (a, b),
                    (Err(e), _) | (_, Err(e)) => return Value::Error(e),
                };
                let start = match v.get(2) {
                    None => 1.0,
                    Some(x) => match x.to_number() {
                        Ok(n) => n.trunc(),
                        Err(e) => return Value::Error(e),
                    },
                };
                match find_text(&needle, &hay, start, search) {
                    Some(p) => Value::Number(p as f64),
                    None => Value::Error(CellError::Value),
                }
            })
        }
        "SUBSTITUTE" => {
            if let Some(e) = arity(args, 3, 4) {
                return Some(e);
            }
            lift(ev, args, &mut |v| {
                let texts: Result<Vec<_>, _> = v[..3].iter().map(Value::to_text).collect();
                let t = match texts {
                    Ok(t) => t,
                    Err(e) => return Value::Error(e),
                };
                let instance = match v.get(3) {
                    None => None,
                    Some(x) => match x.to_number() {
                        Ok(n) if n < 1.0 => return Value::Error(CellError::Value),
                        Ok(n) => Some(n.trunc() as usize),
                        Err(e) => return Value::Error(e),
                    },
                };
                let out = substitute(&t[0], &t[1], &t[2], instance);
                if out.chars().count() > MAX_TEXT {
                    Value::Error(CellError::Value)
                } else {
                    Value::text(out)
                }
            })
        }
        "REPLACE" => {
            if let Some(e) = arity(args, 4, 4) {
                return Some(e);
            }
            lift(ev, args, &mut |v| {
                let (t, new) = match (v[0].to_text(), v[3].to_text()) {
                    (Ok(a), Ok(b)) => (a, b),
                    (Err(e), _) | (_, Err(e)) => return Value::Error(e),
                };
                let (start, n) = match (v[1].to_number(), v[2].to_number()) {
                    (Ok(s), Ok(n)) => (s.trunc(), n.trunc()),
                    (Err(e), _) | (_, Err(e)) => return Value::Error(e),
                };
                if start < 1.0 || n < 0.0 {
                    return Value::Error(CellError::Value);
                }
                let chars: Vec<char> = t.chars().collect();
                let s = (start as usize - 1).min(chars.len());
                let e = (s + n as usize).min(chars.len());
                let mut out: String = chars[..s].iter().collect();
                out.push_str(&new);
                out.extend(&chars[e..]);
                Value::text(out)
            })
        }
        "CHAR" => num_to_text(ev, args, |n| {
            let n = n.trunc();
            if !(1.0..=255.0).contains(&n) {
                return Err(CellError::Value);
            }
            Ok(cp1252_char(n as u8).to_string())
        }),
        "UNICHAR" => num_to_text(ev, args, |n| {
            let n = n.trunc();
            if n < 1.0 {
                return Err(CellError::Value);
            }
            char::from_u32(n as u32).map(|c| c.to_string()).ok_or(CellError::Value)
        }),
        "CODE" | "UNICODE" => {
            let unicode = name == "UNICODE";
            text1(ev, args, move |t| match t.chars().next() {
                None => Value::Error(CellError::Value),
                Some(c) if unicode => Value::Number(c as u32 as f64),
                Some(c) => Value::Number(cp1252_code(c) as f64),
            })
        }
        "VALUE" => {
            if let Some(e) = arity(args, 1, 1) {
                return Some(e);
            }
            lift(ev, args, &mut |v| match &v[0] {
                Value::Number(n) => Value::Number(*n),
                Value::Empty => Value::Number(0.0),
                Value::Error(e) => Value::Error(*e),
                Value::Bool(_) => Value::Error(CellError::Value),
                Value::Text(t) => match text_to_number(t, &options) {
                    Some(n) => Value::Number(n),
                    None => Value::Error(CellError::Value),
                },
            })
        }
        "NUMBERVALUE" => number_value(ev, args),
        "TEXT" => {
            if let Some(e) = arity(args, 2, 2) {
                return Some(e);
            }
            lift(ev, args, &mut |v| {
                let code = match v[1].to_text() {
                    Ok(c) => c,
                    Err(e) => return Value::Error(e),
                };
                let value = match &v[0] {
                    Value::Text(t) => match text_to_number(t, &options) {
                        Some(n) => Value::Number(n),
                        None => v[0].clone(),
                    },
                    Value::Error(e) => return Value::Error(*e),
                    Value::Empty => Value::Number(0.0),
                    other => other.clone(),
                };
                Value::text(format_value(&value, &code, &options).text)
            })
        }
        "FIXED" => {
            if let Some(e) = arity(args, 1, 3) {
                return Some(e);
            }
            let decimals = if omitted(args, 1) { 2.0 } else { tri_some!(ev.number(args, 1)).trunc() };
            let no_commas = tri_some!(ev.bool_or(args, 2, false));
            let n = tri_some!(ev.number(args, 0));
            Eval::text(fixed(n, decimals, !no_commas, "", &options))
        }
        "DOLLAR" => {
            if let Some(e) = arity(args, 1, 2) {
                return Some(e);
            }
            let decimals = if omitted(args, 1) { 2.0 } else { tri_some!(ev.number(args, 1)).trunc() };
            let n = tri_some!(ev.number(args, 0));
            Eval::text(fixed(n, decimals, true, "$", &options))
        }
        "TEXTBEFORE" => text_split(ev, args, true),
        "TEXTAFTER" => text_split(ev, args, false),
        _ => return None,
    })
}

/// A function of one text argument, element by element.
fn text1(ev: &mut Evaluator, args: &[Expr], f: impl Fn(&str) -> Value) -> Eval {
    if let Some(e) = arity(args, 1, 1) {
        return e;
    }
    lift(ev, args, &mut |v| match v[0].to_text() {
        Ok(t) => f(&t),
        Err(e) => Value::Error(e),
    })
}

fn num_to_text(ev: &mut Evaluator, args: &[Expr], f: impl Fn(f64) -> Result<String, CellError>) -> Eval {
    if let Some(e) = arity(args, 1, 1) {
        return e;
    }
    lift(ev, args, &mut |v| match v[0].to_number().and_then(&f) {
        Ok(s) => Value::text(s),
        Err(e) => Value::Error(e),
    })
}

fn limited(s: String) -> Eval {
    if s.chars().count() > MAX_TEXT {
        Eval::error(CellError::Value)
    } else {
        Eval::text(s)
    }
}

fn proper(t: &str) -> String {
    let mut out = String::with_capacity(t.len());
    let mut prev_letter = false;
    for c in t.chars() {
        if c.is_alphabetic() {
            if prev_letter {
                out.extend(c.to_lowercase());
            } else {
                out.extend(c.to_uppercase());
            }
            prev_letter = true;
        } else {
            out.push(c);
            prev_letter = false;
        }
    }
    out
}

/// FIND (case-sensitive) or SEARCH (case-insensitive, wildcards). Returns
/// the 1-based character position.
fn find_text(needle: &str, hay: &str, start: f64, search: bool) -> Option<usize> {
    let hay_chars: Vec<char> = hay.chars().collect();
    if start < 1.0 || start as usize > hay_chars.len() + 1 {
        return None;
    }
    let start = start as usize - 1;
    if needle.is_empty() {
        return Some(start + 1);
    }
    if search && needle.contains(['*', '?', '~']) {
        // Try each start position; the pattern must match a prefix.
        let pattern = format!("{needle}*");
        for i in start..hay_chars.len() {
            let rest: String = hay_chars[i..].iter().collect();
            if wildcard_match(&pattern, &rest) {
                return Some(i + 1);
            }
        }
        return None;
    }
    let fold = |c: char| if search { c.to_lowercase().next().unwrap_or(c) } else { c };
    let n: Vec<char> = needle.chars().map(fold).collect();
    let h: Vec<char> = hay_chars.iter().map(|c| fold(*c)).collect();
    if n.len() > h.len() {
        return None;
    }
    (start..=h.len() - n.len()).find(|&i| h[i..i + n.len()] == n[..]).map(|i| i + 1)
}

fn substitute(text: &str, old: &str, new: &str, instance: Option<usize>) -> String {
    if old.is_empty() {
        return text.to_owned();
    }
    match instance {
        None => text.replace(old, new),
        Some(k) => {
            let mut out = String::with_capacity(text.len());
            let mut rest = text;
            let mut seen = 0;
            while let Some(p) = rest.find(old) {
                seen += 1;
                if seen == k {
                    out.push_str(&rest[..p]);
                    out.push_str(new);
                    out.push_str(&rest[p + old.len()..]);
                    return out;
                }
                out.push_str(&rest[..p + old.len()]);
                rest = &rest[p + old.len()..];
            }
            out.push_str(rest);
            out
        }
    }
}

/// Windows-1252 code points 128-159, the rest map to Latin-1.
const CP1252_HIGH: [u32; 32] = [
    0x20AC, 0x81, 0x201A, 0x0192, 0x201E, 0x2026, 0x2020, 0x2021, 0x02C6, 0x2030, 0x0160, 0x2039, 0x0152, 0x8D, 0x017D,
    0x8F, 0x90, 0x2018, 0x2019, 0x201C, 0x201D, 0x2022, 0x2013, 0x2014, 0x02DC, 0x2122, 0x0161, 0x203A, 0x0153, 0x9D,
    0x017E, 0x0178,
];

fn cp1252_char(b: u8) -> char {
    if (0x80..0xA0).contains(&b) {
        char::from_u32(CP1252_HIGH[(b - 0x80) as usize]).unwrap_or('?')
    } else {
        b as char
    }
}

fn cp1252_code(c: char) -> u32 {
    let u = c as u32;
    if u < 0x80 || (0xA0..=0xFF).contains(&u) {
        return u;
    }
    CP1252_HIGH.iter().position(|&x| x == u).map_or(63, |i| 0x80 + i as u32)
}

/// Text read as a number the way VALUE does: plain numbers, percentages,
/// currency, then dates and times.
fn text_to_number(t: &str, options: &FormatOptions) -> Option<f64> {
    parse_number(t).or_else(|| parse_date_time(t, options.date_order, options.date1904).map(|(n, _)| n))
}

fn number_value(ev: &mut Evaluator, args: &[Expr]) -> Eval {
    if let Some(e) = arity(args, 1, 3) {
        return e;
    }
    let text = tri!(ev.text(args, 0));
    let dec = if omitted(args, 1) { ".".into() } else { tri!(ev.text(args, 1)) };
    let group = if omitted(args, 2) { ",".into() } else { tri!(ev.text(args, 2)) };
    let dec = dec.chars().next().unwrap_or('.');
    let group = group.chars().next().unwrap_or(',');
    if dec == group {
        return Eval::error(CellError::Value);
    }
    let mut cleaned = String::new();
    for c in text.chars() {
        if c.is_whitespace() || c == group {
            continue;
        }
        cleaned.push(if c == dec { '.' } else { c });
    }
    if cleaned.is_empty() {
        return Eval::number(0.0);
    }
    let mut percent = 0;
    while cleaned.ends_with('%') {
        cleaned.pop();
        percent += 1;
    }
    match cleaned.parse::<f64>() {
        Ok(n) if n.is_finite() => Eval::number(n / 100f64.powi(percent)),
        _ => Eval::error(CellError::Value),
    }
}

/// FIXED and DOLLAR: a rounded number with thousands separators.
fn fixed(n: f64, decimals: f64, commas: bool, symbol: &str, options: &FormatOptions) -> String {
    let d = decimals.clamp(-127.0, 127.0) as i32;
    let rounded = super::math::round(n, d as f64);
    let frac = if d > 0 { format!(".{}", "0".repeat(d as usize)) } else { String::new() };
    let int = if commas { "#,##0" } else { "0" };
    let code = if symbol.is_empty() {
        format!("{int}{frac}")
    } else {
        format!("\"{symbol}\"{int}{frac};(\"{symbol}\"{int}{frac})")
    };
    let mut s = format_value(&Value::Number(rounded), &code, options).text;
    if symbol.is_empty() && rounded < 0.0 && !s.starts_with('-') {
        s.insert(0, '-');
    }
    s
}

fn text_split(ev: &mut Evaluator, args: &[Expr], before: bool) -> Eval {
    if let Some(e) = arity(args, 2, 6) {
        return e;
    }
    let text = tri!(ev.text(args, 0));
    let delim = tri!(ev.text(args, 1));
    let instance = if omitted(args, 2) { 1.0 } else { tri!(ev.number(args, 2)).trunc() };
    let ignore_case = !omitted(args, 3) && tri!(ev.number(args, 3)) != 0.0;
    let match_end = !omitted(args, 4) && tri!(ev.number(args, 4)) != 0.0;
    if instance == 0.0 {
        return Eval::error(CellError::Value);
    }
    let fold = |s: &str| if ignore_case { s.to_lowercase() } else { s.to_owned() };
    let (h, d) = (fold(&text), fold(&delim));
    // Byte positions of every match (the text is matched as folded; for
    // simple case folding the byte lengths agree).
    let mut positions: Vec<usize> = if d.is_empty() {
        Vec::new()
    } else {
        h.match_indices(&d).map(|(i, _)| i).collect()
    };
    if match_end {
        positions.push(h.len());
    }
    if d.is_empty() {
        positions = vec![if instance > 0.0 { 0 } else { h.len() }];
    }
    let k = instance.abs() as usize;
    let pick = if instance > 0.0 {
        positions.get(k - 1).copied()
    } else {
        positions.len().checked_sub(k).map(|i| positions[i])
    };
    let Some(p) = pick else {
        return if omitted(args, 5) { Eval::error(CellError::NA) } else { ev.eval(&args[5]) };
    };
    if h.len() != text.len() {
        // Folding changed lengths; fall back to character positions.
        let chars: Vec<char> = text.chars().collect();
        let cp = h[..p].chars().count();
        let dl = d.chars().count();
        return Eval::text(if before {
            chars[..cp.min(chars.len())].iter().collect::<String>()
        } else {
            chars[(cp + dl).min(chars.len())..].iter().collect::<String>()
        });
    }
    let dl = if p == h.len() { 0 } else { d.len() };
    Eval::text(if before { text[..p].to_owned() } else { text[(p + dl).min(text.len())..].to_owned() })
}

fn text_join(ev: &mut Evaluator, args: &[Expr]) -> Eval {
    if args.len() < 3 {
        return Eval::error(CellError::Value);
    }
    let delims: Vec<String> = {
        let mut d = Vec::new();
        for (v, _) in ev.flatten(&args[0]) {
            match v.to_text() {
                Ok(t) => d.push(t.to_string()),
                Err(e) => return Eval::error(e),
            }
        }
        if d.is_empty() {
            d.push(String::new());
        }
        d
    };
    let ignore_empty = tri!(ev.boolean(args, 1));
    let mut parts: Vec<String> = Vec::new();
    for a in &args[2..] {
        let v = ev.eval(a);
        let values: Vec<Value> = match v {
            Eval::Ref(_) | Eval::Array(_) => {
                let arr = ev.to_array(v);
                arr.data
            }
            Eval::Value(x) => vec![x],
        };
        for x in values {
            match x.to_text() {
                Ok(t) => {
                    if !(ignore_empty && t.is_empty()) {
                        parts.push(t.to_string())
                    }
                }
                Err(e) => return Eval::error(e),
            }
        }
    }
    let mut out = String::new();
    for (i, p) in parts.iter().enumerate() {
        if i > 0 {
            out.push_str(&delims[(i - 1) % delims.len()]);
        }
        out.push_str(p);
    }
    limited(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn helpers() {
        assert_eq!(proper("hello wORLD o'neil 2nd"), "Hello World O'Neil 2Nd");
        assert_eq!(find_text("b", "abcb", 1.0, false), Some(2));
        assert_eq!(find_text("b", "abcb", 3.0, false), Some(4));
        assert_eq!(find_text("B", "abcb", 1.0, false), None);
        assert_eq!(find_text("B", "abcb", 1.0, true), Some(2));
        assert_eq!(find_text("c?", "abcb", 1.0, true), Some(3));
        assert_eq!(find_text("", "abc", 2.0, false), Some(2));
        assert_eq!(substitute("a-b-c", "-", "+", None), "a+b+c");
        assert_eq!(substitute("a-b-c", "-", "+", Some(2)), "a-b+c");
        assert_eq!(cp1252_char(0x80), '€');
        assert_eq!(cp1252_code('€'), 0x80);
        assert_eq!(cp1252_code('A'), 65);
    }
}
