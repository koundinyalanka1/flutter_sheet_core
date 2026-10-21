//! OpenFormula (ODF) syntax to Excel syntax.

use crate::refs::quote_sheet_name;

/// Translate an ODF formula attribute (`of:=SUM([.A1:.B3];[Sheet2.C1])`)
/// into Excel syntax without `=` (`SUM(A1:B3,Sheet2!C1)`).
pub fn of_to_excel(formula: &str) -> String {
    let f = formula.trim();
    if let Some(rest) = f.strip_prefix("msoxl:") {
        return rest.trim_start_matches('=').to_owned();
    }
    let body = f
        .strip_prefix("of:")
        .or_else(|| f.strip_prefix("oooc:"))
        .unwrap_or(f)
        .trim_start_matches('=');
    let chars: Vec<char> = body.chars().collect();
    let mut out = String::with_capacity(body.len());
    let mut i = 0;
    let mut brace = 0usize;
    while i < chars.len() {
        let c = chars[i];
        match c {
            '"' => {
                out.push('"');
                i += 1;
                while i < chars.len() {
                    out.push(chars[i]);
                    if chars[i] == '"' {
                        if chars.get(i + 1) == Some(&'"') {
                            out.push('"');
                            i += 2;
                            continue;
                        }
                        break;
                    }
                    i += 1;
                }
                i += 1;
            }
            '[' => {
                let start = i + 1;
                let mut j = start;
                let mut quoted = false;
                while j < chars.len() {
                    match chars[j] {
                        '\'' => quoted = !quoted,
                        ']' if !quoted => break,
                        _ => {}
                    }
                    j += 1;
                }
                let inner: String = chars[start..j.min(chars.len())].iter().collect();
                out.push_str(&reference(&inner));
                i = j + 1;
            }
            '{' => {
                brace += 1;
                out.push('{');
                i += 1;
            }
            '}' => {
                brace = brace.saturating_sub(1);
                out.push('}');
                i += 1;
            }
            ';' => {
                out.push(',');
                i += 1;
            }
            '|' if brace > 0 => {
                out.push(';');
                i += 1;
            }
            '~' => {
                out.push(',');
                i += 1;
            }
            '!' => {
                out.push(' ');
                i += 1;
            }
            _ => {
                // Function names with an interoperability prefix.
                let rest: String = chars[i..chars.len().min(i + 32)].iter().collect();
                let upper = rest.to_ascii_uppercase();
                let mut skipped = false;
                for prefix in ["COM.MICROSOFT.", "ORG.OPENOFFICE.", "_XLFN."] {
                    if upper.starts_with(prefix) && (i == 0 || !chars[i - 1].is_alphanumeric()) {
                        i += prefix.len();
                        skipped = true;
                        break;
                    }
                }
                if !skipped {
                    out.push(c);
                    i += 1;
                }
            }
        }
    }
    out
}

/// One bracketed reference: `.A1`, `Sheet2.B3`, `.A1:.B3`,
/// `$'My Sheet'.$A$1:.B2`, `Sheet1.A1:Sheet3.A1`.
fn reference(inner: &str) -> String {
    // Split at the ':' that is outside quotes.
    let mut parts: Vec<String> = Vec::new();
    let mut cur = String::new();
    let mut quoted = false;
    for c in inner.chars() {
        match c {
            '\'' => {
                quoted = !quoted;
                cur.push(c);
            }
            ':' if !quoted => parts.push(std::mem::take(&mut cur)),
            _ => cur.push(c),
        }
    }
    parts.push(cur);
    let split = |p: &str| -> (Option<String>, String) {
        // The sheet ends at the last '.' outside quotes.
        let mut q = false;
        let mut dot = None;
        for (i, c) in p.char_indices() {
            match c {
                '\'' => q = !q,
                '.' if !q => dot = Some(i),
                _ => {}
            }
        }
        match dot {
            Some(d) => {
                let sheet = p[..d].trim_start_matches('$');
                let cell = p[d + 1..].to_owned();
                let sheet = if sheet.is_empty() {
                    None
                } else if let Some(s) = sheet.strip_prefix('\'').and_then(|s| s.strip_suffix('\'')) {
                    Some(s.replace("''", "'"))
                } else {
                    Some(sheet.to_owned())
                };
                (sheet, cell)
            }
            None => (None, p.to_owned()),
        }
    };
    if parts.iter().any(|p| p.contains("#REF!")) {
        return "#REF!".into();
    }
    let (s1, c1) = split(&parts[0]);
    if parts.len() == 1 {
        return match s1 {
            Some(s) => format!("{}!{c1}", quote_sheet_name(&s)),
            None => c1,
        };
    }
    let (s2, c2) = split(&parts[1]);
    match (s1, s2) {
        (Some(a), Some(b)) if !a.eq_ignore_ascii_case(&b) => {
            let both = format!("{a}:{b}");
            let quoted = quote_sheet_name(&a) != a || quote_sheet_name(&b) != b;
            if quoted {
                format!("'{}'!{c1}:{c2}", both.replace('\'', "''"))
            } else {
                format!("{both}!{c1}:{c2}")
            }
        }
        (Some(a), _) => format!("{}!{c1}:{c2}", quote_sheet_name(&a)),
        (None, _) => format!("{c1}:{c2}"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::formula::{parse, unparse_odf};

    #[test]
    fn to_excel() {
        assert_eq!(of_to_excel("of:=SUM([.A1:.B3];[Sheet2.C1])"), "SUM(A1:B3,Sheet2!C1)");
        assert_eq!(of_to_excel("of:=[.D2]-[.C2]"), "D2-C2");
        assert_eq!(of_to_excel("of:=[$'My Sheet'.$A$1]*2"), "'My Sheet'!$A$1*2");
        assert_eq!(of_to_excel("of:=IF([.A1]>0;\"a;b\";\"c\")"), "IF(A1>0,\"a;b\",\"c\")");
        assert_eq!(of_to_excel("of:=SUM({1;2|3;4})"), "SUM({1,2;3,4})");
        assert_eq!(of_to_excel("of:=COM.MICROSOFT.XLOOKUP([.A1];[.B1:.B9];[.C1:.C9])"), "XLOOKUP(A1,B1:B9,C1:C9)");
        assert_eq!(of_to_excel("of:=SUM([Sheet1.A1:Sheet3.A1])"), "SUM(Sheet1:Sheet3!A1:A1)");
        assert_eq!(of_to_excel("of:=[.#REF!]+1"), "#REF!+1");
        assert_eq!(of_to_excel("msoxl:=A1+B1"), "A1+B1");
    }

    #[test]
    fn round_trip_through_odf() {
        for f in [
            "SUM(A1:B3,Sheet2!C1)",
            "'My Sheet'!$A$1*2",
            "IF(A1>0,\"a;b\",\"c\")",
            "SUM({1,2;3,4})",
            "VLOOKUP(A1,$A$1:$B$20,2,FALSE)",
        ] {
            let e = parse(f).unwrap();
            let odf = unparse_odf(&e);
            let back = crate::formula::unparse(&parse(&of_to_excel(&odf)).unwrap());
            let expected = crate::formula::unparse(&e).replace("FALSE", "FALSE()");
            assert_eq!(back, expected, "{f} via {odf}");
        }
    }
}
