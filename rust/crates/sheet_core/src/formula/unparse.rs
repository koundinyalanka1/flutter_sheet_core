//! Writing a parsed formula back as text.

use super::ast::{Area, CellAddr, Expr, Reference, UnaryOp};
use crate::refs::{col_to_letters, quote_sheet_name};

/// Functions added after Excel 2007. In OOXML files they are stored with an
/// `_xlfn.` prefix; without it Excel reads the formula as `#NAME?`.
const XLFN_FUNCTIONS: &[&str] = &[
    "AGGREGATE", "ARABIC", "BASE", "BETA.DIST", "BETA.INV", "BINOM.DIST", "BINOM.DIST.RANGE", "BINOM.INV",
    "BITAND", "BITLSHIFT", "BITOR", "BITRSHIFT", "BITXOR", "CEILING.MATH", "CEILING.PRECISE", "CHISQ.DIST",
    "CHISQ.DIST.RT", "CHISQ.INV", "CHISQ.INV.RT", "CHISQ.TEST", "CHOOSECOLS", "CHOOSEROWS", "COMBINA", "CONCAT",
    "CONFIDENCE.NORM", "CONFIDENCE.T", "COT", "COTH", "COVARIANCE.P", "COVARIANCE.S", "CSC", "CSCH", "DAYS",
    "DECIMAL", "DROP", "ERF.PRECISE", "ERFC.PRECISE", "EXPAND", "EXPON.DIST", "F.DIST", "F.DIST.RT", "F.INV",
    "F.INV.RT", "F.TEST", "FILTER", "FILTERXML", "FLOOR.MATH", "FLOOR.PRECISE", "FORECAST.ETS",
    "FORECAST.ETS.CONFINT", "FORECAST.ETS.SEASONALITY", "FORECAST.ETS.STAT", "FORECAST.LINEAR", "FORMULATEXT",
    "GAMMA", "GAMMA.DIST", "GAMMA.INV", "GAMMALN.PRECISE", "GAUSS", "HSTACK", "IFNA", "IFS", "IMCOSH", "IMCOT",
    "IMCSC", "IMCSCH", "IMSEC", "IMSECH", "IMSINH", "IMTAN", "ISFORMULA", "ISOMITTED", "ISOWEEKNUM", "LAMBDA",
    "LET", "LOGNORM.DIST", "LOGNORM.INV", "MAKEARRAY", "MAP", "MAXIFS", "MINIFS", "MODE.MULT", "MODE.SNGL",
    "MUNIT", "NEGBINOM.DIST", "NETWORKDAYS.INTL", "NORM.DIST", "NORM.INV", "NORM.S.DIST", "NORM.S.INV",
    "NUMBERVALUE", "PDURATION", "PERCENTILE.EXC", "PERCENTILE.INC", "PERCENTRANK.EXC", "PERCENTRANK.INC",
    "PERMUTATIONA", "PHI", "POISSON.DIST", "QUARTILE.EXC", "QUARTILE.INC", "RANDARRAY", "RANK.AVG", "RANK.EQ",
    "REDUCE", "RRI", "SCAN", "SEC", "SECH", "SEQUENCE", "SHEET", "SHEETS", "SKEW.P", "SORT", "SORTBY",
    "STDEV.P", "STDEV.S", "SWITCH", "T.DIST", "T.DIST.2T", "T.DIST.RT", "T.INV", "T.INV.2T", "T.TEST", "TAKE",
    "TEXTAFTER", "TEXTBEFORE", "TEXTJOIN", "TEXTSPLIT", "TOCOL", "TOROW", "UNICHAR", "UNICODE", "UNIQUE",
    "VALUETOTEXT", "ARRAYTOTEXT", "VAR.P", "VAR.S", "VSTACK", "WEIBULL.DIST", "WORKDAY.INTL", "WRAPCOLS",
    "WRAPROWS", "XLOOKUP", "XMATCH", "XOR", "Z.TEST",
    // The implicit-intersection `@` and spill `#` operators, as stored.
    "SINGLE", "ANCHORARRAY",
];

/// Worksheet-only functions, stored as `_xlfn._xlws.NAME`.
const XLWS_FUNCTIONS: &[&str] = &["FILTER", "SORT"];

/// Whether OOXML stores this function with the `_xlfn.` prefix.
pub fn needs_xlfn_prefix(name: &str) -> bool {
    XLFN_FUNCTIONS.contains(&name) || XLWS_FUNCTIONS.contains(&name)
}

/// The prefix OOXML stores before this function's name, if any.
pub fn ooxml_prefix(name: &str) -> &'static str {
    if XLWS_FUNCTIONS.contains(&name) {
        "_xlfn._xlws."
    } else if XLFN_FUNCTIONS.contains(&name) {
        "_xlfn."
    } else {
        ""
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Mode {
    Excel,
    Ooxml,
    Odf,
}

/// The formula as text, without the leading `=`.
pub fn unparse(expr: &Expr) -> String {
    let mut out = String::new();
    write(expr, &mut out, Mode::Excel);
    out
}

/// The formula as OOXML stores it: newer functions get `_xlfn.`.
pub fn unparse_ooxml(expr: &Expr) -> String {
    let mut out = String::new();
    write(expr, &mut out, Mode::Ooxml);
    out
}

/// The formula in OpenFormula syntax, as OpenDocument stores it (with its
/// `of:=` prefix): `of:=SUM([.A1:.B3];[Sheet2.C1])`.
pub fn unparse_odf(expr: &Expr) -> String {
    let mut out = String::from("of:=");
    write(expr, &mut out, Mode::Odf);
    out
}

fn odf_sheet(name: &str) -> String {
    let plain = !name.is_empty() && name.chars().all(|c| c.is_alphanumeric() || c == '_');
    if plain {
        name.to_owned()
    } else {
        format!("'{}'", name.replace('\'', "''"))
    }
}

/// A reference in OpenFormula's bracketed form.
pub fn odf_reference_text(r: &Reference) -> String {
    let mut out = String::from("[");
    let sheet = |out: &mut String, s: Option<&String>| {
        if let Some(s) = s {
            out.push('$');
            out.push_str(&odf_sheet(s));
        }
        out.push('.');
    };
    let full = |a: CellAddr, row: bool, col: bool, last: bool| -> CellAddr {
        let mut a = a;
        if !row {
            a.row = if last { crate::refs::MAX_ROWS - 1 } else { 0 };
        }
        if !col {
            a.col = if last { crate::refs::MAX_COLS - 1 } else { 0 };
        }
        a
    };
    let (a, b) = match r.area {
        None => {
            out.push_str("#REF!]");
            return out;
        }
        Some(Area::Cell(a)) => (a, None),
        Some(Area::Range(a, b)) => (a, Some(b)),
        Some(Area::Cols(a, b)) => (full(a, false, true, false), Some(full(b, false, true, true))),
        Some(Area::Rows(a, b)) => (full(a, true, false, false), Some(full(b, true, false, true))),
    };
    sheet(&mut out, r.sheet.as_ref());
    addr(a, &mut out, true, true);
    if let Some(b) = b {
        out.push(':');
        match &r.last_sheet {
            Some(last) => sheet(&mut out, Some(last)),
            None => out.push('.'),
        }
        addr(b, &mut out, true, true);
    } else if let Some(last) = &r.last_sheet {
        out.push(':');
        sheet(&mut out, Some(last));
        addr(a, &mut out, true, true);
    }
    out.push(']');
    out
}

/// A number in formula text: exact, short, and in the form Excel writes.
pub fn number_literal(n: f64) -> String {
    let a = n.abs();
    if n == n.trunc() && a < 1e15 {
        return format!("{}", n as i64);
    }
    if (1e-9..1e15).contains(&a) {
        let s = format!("{n}");
        return s;
    }
    let s = format!("{n:E}");
    // Rust writes 1E-10; Excel writes 1E-10 too, but 1E10 as 1E+10.
    match s.split_once('E') {
        Some((m, e)) if !e.starts_with('-') => format!("{m}E+{e}"),
        _ => s,
    }
}

fn addr(a: CellAddr, out: &mut String, row: bool, col: bool) {
    if col {
        if a.col_abs {
            out.push('$');
        }
        out.push_str(&col_to_letters(a.col));
    }
    if row {
        if a.row_abs {
            out.push('$');
        }
        out.push_str(&(a.row + 1).to_string());
    }
}

/// A reference as text (`Sheet1!$A$1:B2`).
pub fn reference_text(r: &Reference) -> String {
    let mut out = String::new();
    if let Some(sheet) = &r.sheet {
        match &r.last_sheet {
            Some(last) => {
                let both = format!("{sheet}:{last}");
                let quoted = quote_sheet_name(sheet) != *sheet || quote_sheet_name(last) != *last;
                if quoted {
                    out.push('\'');
                    out.push_str(&both.replace('\'', "''"));
                    out.push('\'');
                } else {
                    out.push_str(&both);
                }
            }
            None => out.push_str(&quote_sheet_name(sheet)),
        }
        out.push('!');
    }
    match r.area {
        None => out.push_str("#REF!"),
        Some(Area::Cell(a)) => addr(a, &mut out, true, true),
        Some(Area::Range(a, b)) => {
            addr(a, &mut out, true, true);
            out.push(':');
            addr(b, &mut out, true, true);
        }
        Some(Area::Cols(a, b)) => {
            addr(a, &mut out, false, true);
            out.push(':');
            addr(b, &mut out, false, true);
        }
        Some(Area::Rows(a, b)) => {
            addr(a, &mut out, true, false);
            out.push(':');
            addr(b, &mut out, true, false);
        }
    }
    out
}

fn write(expr: &Expr, out: &mut String, mode: Mode) {
    enum Part<'a> {
        Expr(&'a Expr),
        Text(&'a str),
    }
    let ooxml = mode == Mode::Ooxml;
    let odf = mode == Mode::Odf;
    let symbol = |op: super::ast::BinaryOp| match (odf, op) {
        (true, super::ast::BinaryOp::Union) => "~",
        (true, super::ast::BinaryOp::Intersect) => "!",
        _ => op.symbol(),
    };
    let mut pending = vec![Part::Expr(expr)];
    while let Some(part) = pending.pop() {
        let expr = match part {
            Part::Expr(e) => e,
            Part::Text(s) => {
                out.push_str(s);
                continue;
            }
        };
        match expr {
            Expr::Number(n) => out.push_str(&number_literal(*n)),
            Expr::Text(t) => {
                out.push('"');
                out.push_str(&t.replace('"', "\"\""));
                out.push('"');
            }
            Expr::Bool(b) if odf => out.push_str(if *b { "TRUE()" } else { "FALSE()" }),
            Expr::Bool(b) => out.push_str(if *b { "TRUE" } else { "FALSE" }),
            Expr::Error(e) => out.push_str(e.as_str()),
            Expr::Ref(r) if odf => out.push_str(&odf_reference_text(r)),
            Expr::Ref(r) => out.push_str(&reference_text(r)),
            Expr::Name(n) | Expr::Structured(n) => out.push_str(n),
            Expr::Unary(op, e) => {
                out.push(if *op == UnaryOp::Neg { '-' } else { '+' });
                pending.push(Part::Expr(e));
            }
            Expr::Binary(op, a, b) => {
                pending.push(Part::Expr(b));
                pending.push(Part::Text(symbol(*op)));
                pending.push(Part::Expr(a));
            }
            Expr::Chain(first, rest) => {
                for (op, e) in rest.iter().rev() {
                    pending.push(Part::Expr(e));
                    pending.push(Part::Text(symbol(*op)));
                }
                pending.push(Part::Expr(first));
            }
            Expr::Percent(e) => {
                pending.push(Part::Text("%"));
                pending.push(Part::Expr(e));
            }
            Expr::Call(name, args) => {
                if ooxml {
                    out.push_str(ooxml_prefix(name));
                }
                out.push_str(name);
                out.push('(');
                pending.push(Part::Text(")"));
                for (i, arg) in args.iter().enumerate().rev() {
                    pending.push(Part::Expr(arg));
                    if i > 0 {
                        pending.push(Part::Text(if odf { ";" } else { "," }));
                    }
                }
            }
            Expr::Array(rows) => {
                out.push('{');
                pending.push(Part::Text("}"));
                for (r, row) in rows.iter().enumerate().rev() {
                    for (c, e) in row.iter().enumerate().rev() {
                        pending.push(Part::Expr(e));
                        if c > 0 {
                            pending.push(Part::Text(if odf { ";" } else { "," }));
                        }
                    }
                    if r > 0 {
                        pending.push(Part::Text(if odf { "|" } else { ";" }));
                    }
                }
            }
            Expr::Missing => {}
            Expr::Paren(e) => {
                out.push('(');
                pending.push(Part::Text(")"));
                pending.push(Part::Expr(e));
            }
        }
    }
}
