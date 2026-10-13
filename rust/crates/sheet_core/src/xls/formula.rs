//! BIFF8 formulas (parsed tokens, "rgce") back to formula text, so a
//! workbook from Excel 97-2003 keeps its formulas when it becomes an XLSX.

use crate::refs::CellRef;

/// What references need from the workbook: sheet names for 3-D
/// references and defined names.
pub struct Names<'a> {
    pub sheets: &'a [String],
    /// EXTERNSHEET entries: (supporting book is this one, first, last).
    pub xti: &'a [(bool, u16, u16)],
    pub defined: &'a [String],
    /// External names by (EXTERNSHEET index, name index), for add-in
    /// functions.
    pub external: &'a dyn Fn(u16, u16) -> Option<String>,
}

fn col_name(c: u32) -> String {
    let mut n = c + 1;
    let mut s = String::new();
    while n > 0 {
        let r = (n - 1) % 26;
        s.insert(0, (b'A' + r as u8) as char);
        n = (n - 1) / 26;
    }
    s
}

fn quote_sheet(name: &str) -> String {
    let plain = name.chars().all(|c| c.is_alphanumeric() || c == '_' || c == '.') && !name.chars().next().is_some_and(|c| c.is_ascii_digit());
    if plain {
        name.to_owned()
    } else {
        format!("'{}'", name.replace('\'', "''"))
    }
}

/// A reference with its relative flags (from a 16-bit column field).
fn cell(row: i64, col_field: u16, base: Option<CellRef>) -> Option<String> {
    let row_rel = col_field & 0x4000 != 0;
    let col_rel = col_field & 0x8000 != 0;
    let (r, c) = match base {
        // Shared formulas: relative parts are offsets from the cell.
        Some(b) => {
            let r = if row_rel { b.row as i64 + (row as i16) as i64 } else { row };
            let c = if col_rel { b.col as i64 + ((col_field & 0xFF) as u8 as i8) as i64 } else { (col_field & 0xFF) as i64 };
            (r.rem_euclid(65536), c.rem_euclid(256))
        }
        None => (row, (col_field & 0x3FFF) as i64),
    };
    if r < 0 || c < 0 {
        return None;
    }
    Some(format!("{}{}{}{}", if col_rel { "" } else { "$" }, col_name(c as u32), if row_rel { "" } else { "$" }, r + 1))
}

fn u16_at(b: &[u8], i: usize) -> Option<u16> {
    Some(u16::from_le_bytes([*b.get(i)?, *b.get(i + 1)?]))
}

/// A BIFF error code as text.
pub fn error_text(code: u8) -> &'static str {
    match code {
        0x00 => "#NULL!",
        0x07 => "#DIV/0!",
        0x0F => "#VALUE!",
        0x17 => "#REF!",
        0x1D => "#NAME?",
        0x24 => "#NUM!",
        _ => "#N/A",
    }
}

/// Decompile `rgce` (tokens) to formula text, without `=`. `base` is the
/// cell for relative (shared) references. None when the formula uses
/// something not handled here.
pub fn decompile(rgce: &[u8], base: CellRef, shared: bool, names: &Names) -> Option<String> {
    let mut stack: Vec<String> = Vec::new();
    let mut i = 0;
    let b = rgce;
    let rel_base = if shared { Some(base) } else { None };
    while i < b.len() {
        let t = b[i];
        i += 1;
        let ptg = if t & 0x60 != 0 { (t & 0x1F) | 0x20 } else { t };
        match ptg {
            0x01 | 0x02 => return None,
            0x03..=0x11 => {
                let rhs = stack.pop()?;
                let lhs = stack.pop()?;
                let op = match ptg {
                    0x03 => "+",
                    0x04 => "-",
                    0x05 => "*",
                    0x06 => "/",
                    0x07 => "^",
                    0x08 => "&",
                    0x09 => "<",
                    0x0A => "<=",
                    0x0B => "=",
                    0x0C => ">=",
                    0x0D => ">",
                    0x0E => "<>",
                    0x0F => " ",
                    0x10 => ",",
                    _ => ":",
                };
                stack.push(format!("{lhs}{op}{rhs}"));
            }
            0x12 => {
                let v = stack.pop()?;
                stack.push(format!("+{v}"));
            }
            0x13 => {
                let v = stack.pop()?;
                stack.push(format!("-{v}"));
            }
            0x14 => {
                let v = stack.pop()?;
                stack.push(format!("{v}%"));
            }
            0x15 => {
                let v = stack.pop()?;
                stack.push(format!("({v})"));
            }
            0x16 => stack.push(String::new()),
            0x17 => {
                let n = *b.get(i)? as usize;
                let flags = *b.get(i + 1)?;
                i += 2;
                let s = if flags & 1 != 0 {
                    let units: Vec<u16> = (0..n).map(|k| u16_at(b, i + k * 2).unwrap_or(0)).collect();
                    i += n * 2;
                    String::from_utf16_lossy(&units)
                } else {
                    let s: String = b.get(i..i + n)?.iter().map(|&c| c as char).collect();
                    i += n;
                    s
                };
                stack.push(format!("\"{}\"", s.replace('"', "\"\"")));
            }
            0x19 => {
                let kind = *b.get(i)?;
                let data = u16_at(b, i + 1)? as usize;
                i += 3;
                if kind & 0x04 != 0 {
                    // CHOOSE's jump table.
                    i += 2 * (data + 1);
                }
                if kind & 0x10 != 0 {
                    let v = stack.pop()?;
                    stack.push(format!("SUM({v})"));
                }
            }
            0x1C => {
                let e = *b.get(i)?;
                i += 1;
                stack.push(error_text(e).to_owned());
            }
            0x1D => {
                let v = *b.get(i)?;
                i += 1;
                stack.push(if v != 0 { "TRUE" } else { "FALSE" }.to_owned());
            }
            0x1E => {
                let v = u16_at(b, i)?;
                i += 2;
                stack.push(v.to_string());
            }
            0x1F => {
                let v = f64::from_le_bytes(b.get(i..i + 8)?.try_into().ok()?);
                i += 8;
                stack.push(crate::value::number_to_text(v));
            }
            0x20 => {
                // Constant arrays keep their values after the tokens; rare in
                // ordinary sheets.
                return None;
            }
            0x21 | 0x22 => {
                let (argc, tab) = if ptg == 0x21 {
                    let tab = u16_at(b, i)?;
                    i += 2;
                    let (_, n) = super::ftab::function(tab)?;
                    if n < 0 {
                        return None;
                    }
                    (n as usize, tab)
                } else {
                    let argc = (*b.get(i)? & 0x7F) as usize;
                    let tab = u16_at(b, i + 1)? & 0x7FFF;
                    i += 3;
                    (argc, tab)
                };
                if stack.len() < argc {
                    return None;
                }
                let args = stack.split_off(stack.len() - argc);
                if tab == 255 {
                    // A user or add-in function: its name comes first.
                    let (name, rest) = args.split_first()?;
                    stack.push(format!("{name}({})", rest.join(",")));
                } else {
                    let (name, _) = super::ftab::function(tab)?;
                    stack.push(format!("{name}({})", args.join(",")));
                }
            }
            0x23 => {
                let n = u16_at(b, i)? as usize;
                i += 4;
                stack.push(names.defined.get(n.checked_sub(1)?)?.clone());
            }
            0x24 | 0x2C => {
                let r = u16_at(b, i)?;
                let c = u16_at(b, i + 2)?;
                i += 4;
                let base = if ptg == 0x2C { Some(base) } else { rel_base };
                stack.push(cell(r as i64, c, base)?);
            }
            0x25 | 0x2D => {
                let r1 = u16_at(b, i)?;
                let r2 = u16_at(b, i + 2)?;
                let c1 = u16_at(b, i + 4)?;
                let c2 = u16_at(b, i + 6)?;
                i += 8;
                let base = if ptg == 0x2D { Some(base) } else { rel_base };
                stack.push(format!("{}:{}", cell(r1 as i64, c1, base)?, cell(r2 as i64, c2, base)?));
            }
            0x26 | 0x27 | 0x28 => i += 6,
            0x29 => i += 2,
            0x2A => {
                i += 4;
                stack.push("#REF!".into());
            }
            0x2B => {
                i += 8;
                stack.push("#REF!".into());
            }
            0x39 => {
                let ixti = u16_at(b, i)?;
                let n = u16_at(b, i + 2)?;
                i += 6;
                stack.push((names.external)(ixti, n)?);
            }
            0x3A | 0x3B => {
                let ixti = u16_at(b, i)? as usize;
                i += 2;
                let &(internal, first, last) = names.xti.get(ixti)?;
                if !internal {
                    return None;
                }
                let s1 = names.sheets.get(first as usize)?;
                let prefix = if last != first {
                    let s2 = names.sheets.get(last as usize)?;
                    quote_sheet(&format!("{s1}:{s2}"))
                } else {
                    quote_sheet(s1)
                };
                if ptg == 0x3A {
                    let r = u16_at(b, i)?;
                    let c = u16_at(b, i + 2)?;
                    i += 4;
                    stack.push(format!("{prefix}!{}", cell(r as i64, c, rel_base)?));
                } else {
                    let r1 = u16_at(b, i)?;
                    let r2 = u16_at(b, i + 2)?;
                    let c1 = u16_at(b, i + 4)?;
                    let c2 = u16_at(b, i + 6)?;
                    i += 8;
                    stack.push(format!("{prefix}!{}:{}", cell(r1 as i64, c1, rel_base)?, cell(r2 as i64, c2, rel_base)?));
                }
            }
            0x3C => {
                i += 6;
                stack.push("#REF!".into());
            }
            0x3D => {
                i += 10;
                stack.push("#REF!".into());
            }
            _ => return None,
        }
    }
    if stack.len() == 1 {
        stack.pop()
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn names() -> (Vec<String>, Vec<(bool, u16, u16)>, Vec<String>) {
        (vec!["Sheet1".into(), "My Data".into()], vec![(true, 1, 1)], vec!["Rate".into()])
    }

    #[test]
    fn decompiles_common_formulas() {
        let (sheets, xti, defined) = names();
        let ext = |_: u16, _: u16| None;
        let n = Names { sheets: &sheets, xti: &xti, defined: &defined, external: &ext };
        let at = CellRef { row: 4, col: 3 };
        // =SUM(B2:C3)*Rate
        let mut t = vec![0x25];
        t.extend_from_slice(&[1, 0, 2, 0, 1, 0xC0, 2, 0xC0]);
        t.extend_from_slice(&[0x22, 1, 4, 0]);
        t.extend_from_slice(&[0x23, 1, 0, 0, 0]);
        t.push(0x05);
        assert_eq!(decompile(&t, at, false, &n).as_deref(), Some("SUM(B2:C3)*Rate"));
        // ='My Data'!$A$1&"x"
        let mut t = vec![0x3A, 0, 0, 0, 0, 0, 0];
        t.extend_from_slice(&[0x17, 1, 0, b'x']);
        t.push(0x08);
        assert_eq!(decompile(&t, at, false, &n).as_deref(), Some("'My Data'!$A$1&\"x\""));
        // =IF(A1>0,ROUND(A1,2),"-") with the jump attributes.
        let t = [
            0x24, 0, 0, 0, 0xC0, 0x1E, 0, 0, 0x0D, 0x19, 0x02, 8, 0, 0x24, 0, 0, 0, 0xC0, 0x1E, 2, 0, 0x21, 27, 0, 0x19, 0x08, 4, 0, 0x17, 1, 0, b'-', 0x19, 0x08, 3, 0, 0x22, 3, 1, 0,
        ];
        assert_eq!(decompile(&t, at, false, &n).as_deref(), Some("IF(A1>0,ROUND(A1,2),\"-\")"));
        // A shared formula: one row up, same column (RefN with -1 rows).
        let t = [0x2C, 0xFF, 0xFF, 0, 0xC0];
        assert_eq!(decompile(&t, at, true, &n).as_deref(), Some("D4"));
    }
}
