//! Parsing formula text (A1 notation, without the leading `=`).

use std::sync::Arc;

use super::ast::{Area, BinaryOp, CellAddr, Expr, Reference, UnaryOp};
use crate::refs::{letters_to_col, MAX_ROWS};
use crate::value::CellError;

/// Why a formula could not be read.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ParseError {
    /// Character offset in the formula.
    pub at: usize,
    pub message: String,
}

impl std::fmt::Display for ParseError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{} (at character {})", self.message, self.at + 1)
    }
}

/// Excel allows 64 levels of nesting; leave room above that.
const MAX_DEPTH: usize = 200;

/// Parse a formula (without its `=`).
pub fn parse(formula: &str) -> Result<Expr, ParseError> {
    let mut p = Parser {
        chars: formula.chars().collect(),
        pos: 0,
        depth: 0,
    };
    p.skip_spaces();
    if p.at_end() {
        return Err(p.error("the formula is empty"));
    }
    let expr = p.expr(0, false)?;
    p.skip_spaces();
    if !p.at_end() {
        return Err(p.error(&format!("unexpected '{}'", p.chars[p.pos])));
    }
    Ok(expr)
}

struct Parser {
    chars: Vec<char>,
    pos: usize,
    depth: usize,
}

fn is_word_char(c: char) -> bool {
    c.is_alphanumeric() || matches!(c, '_' | '.' | '\\' | '$' | '?')
}

impl Parser {
    fn error(&self, message: &str) -> ParseError {
        ParseError {
            at: self.pos,
            message: message.to_owned(),
        }
    }

    fn at_end(&self) -> bool {
        self.pos >= self.chars.len()
    }

    fn peek(&self) -> Option<char> {
        self.chars.get(self.pos).copied()
    }

    fn peek_at(&self, offset: usize) -> Option<char> {
        self.chars.get(self.pos + offset).copied()
    }

    fn skip_spaces(&mut self) -> bool {
        let start = self.pos;
        while self.peek().is_some_and(|c| c == ' ' || c == '\n' || c == '\r' || c == '\t') {
            self.pos += 1;
        }
        self.pos > start
    }

    fn expr(&mut self, min_prec: u8, comma_is_union: bool) -> Result<Expr, ParseError> {
        self.depth += 1;
        if self.depth > MAX_DEPTH {
            return Err(self.error("the formula nests too deeply"));
        }
        self.skip_spaces();
        let mut lhs = match self.peek() {
            Some('-') | Some('+') => {
                let op = if self.peek() == Some('-') { UnaryOp::Neg } else { UnaryOp::Plus };
                self.pos += 1;
                // Negation binds tighter than ^ but looser than references.
                let operand = self.expr(6, comma_is_union)?;
                Expr::Unary(op, Box::new(operand))
            }
            _ => self.primary()?,
        };
        loop {
            let spaced = self.skip_spaces();
            let Some(c) = self.peek() else { break };
            if c == '%' {
                if 6 < min_prec {
                    break;
                }
                self.pos += 1;
                lhs = Expr::Percent(Box::new(lhs));
                continue;
            }
            let op = match c {
                '+' => BinaryOp::Add,
                '-' => BinaryOp::Sub,
                '*' => BinaryOp::Mul,
                '/' => BinaryOp::Div,
                '^' => BinaryOp::Pow,
                '&' => BinaryOp::Concat,
                '=' => BinaryOp::Eq,
                '<' => match self.peek_at(1) {
                    Some('=') => BinaryOp::Le,
                    Some('>') => BinaryOp::Ne,
                    _ => BinaryOp::Lt,
                },
                '>' => {
                    if self.peek_at(1) == Some('=') {
                        BinaryOp::Ge
                    } else {
                        BinaryOp::Gt
                    }
                }
                ':' => BinaryOp::Range,
                ',' if comma_is_union => BinaryOp::Union,
                _ if spaced && is_reference_like(&lhs) && self.starts_reference() => BinaryOp::Intersect,
                _ => break,
            };
            let prec = op.precedence();
            if prec < min_prec {
                if op == BinaryOp::Intersect {
                    // Give the spaces back; they were only spacing.
                }
                break;
            }
            if op != BinaryOp::Intersect {
                self.pos += op.symbol().len();
            }
            let rhs = self.expr(prec + 1, comma_is_union)?;
            lhs = Expr::Binary(op, Box::new(lhs), Box::new(rhs));
        }
        self.depth -= 1;
        Ok(lhs)
    }

    /// Whether the text at the cursor begins a reference or name.
    fn starts_reference(&self) -> bool {
        self.peek().is_some_and(|c| c.is_alphabetic() || matches!(c, '$' | '\'' | '_' | '('))
    }

    fn primary(&mut self) -> Result<Expr, ParseError> {
        let Some(c) = self.peek() else {
            return Err(self.error("the formula ends too early"));
        };
        match c {
            '(' => {
                self.pos += 1;
                let inner = self.expr(0, true)?;
                self.skip_spaces();
                if self.peek() != Some(')') {
                    return Err(self.error("a '(' is not closed"));
                }
                self.pos += 1;
                Ok(Expr::Paren(Box::new(inner)))
            }
            '"' => self.string().map(|s| Expr::Text(s.into())),
            '{' => self.array(),
            '#' => {
                let e = self.error_literal()?;
                Ok(match e {
                    CellError::Ref => Expr::Ref(Reference {
                        sheet: None,
                        last_sheet: None,
                        area: None,
                    }),
                    other => Expr::Error(other),
                })
            }
            '0'..='9' | '.' => {
                if let Some(r) = self.try_row_range(None, None)? {
                    return Ok(r);
                }
                self.number().map(Expr::Number)
            }
            '$' => {
                if let Some(r) = self.try_row_range(None, None)? {
                    return Ok(r);
                }
                self.reference_or_name()
            }
            '\'' => self.reference_or_name(),
            c if c.is_alphabetic() || c == '_' || c == '\\' => self.reference_or_name(),
            other => Err(self.error(&format!("unexpected '{other}'"))),
        }
    }

    fn string(&mut self) -> Result<String, ParseError> {
        self.pos += 1; // opening quote
        let mut out = String::new();
        loop {
            match self.peek() {
                None => return Err(self.error("a text value is missing its closing quote")),
                Some('"') if self.peek_at(1) == Some('"') => {
                    out.push('"');
                    self.pos += 2;
                }
                Some('"') => {
                    self.pos += 1;
                    return Ok(out);
                }
                Some(c) => {
                    out.push(c);
                    self.pos += 1;
                }
            }
        }
    }

    fn number(&mut self) -> Result<f64, ParseError> {
        let start = self.pos;
        while self.peek().is_some_and(|c| c.is_ascii_digit()) {
            self.pos += 1;
        }
        if self.peek() == Some('.') {
            self.pos += 1;
            while self.peek().is_some_and(|c| c.is_ascii_digit()) {
                self.pos += 1;
            }
        }
        if matches!(self.peek(), Some('e') | Some('E'))
            && (self.peek_at(1).is_some_and(|c| c.is_ascii_digit())
                || (matches!(self.peek_at(1), Some('+') | Some('-')) && self.peek_at(2).is_some_and(|c| c.is_ascii_digit())))
        {
            self.pos += 2;
            while self.peek().is_some_and(|c| c.is_ascii_digit()) {
                self.pos += 1;
            }
        }
        let text: String = self.chars[start..self.pos].iter().collect();
        text.parse::<f64>()
            .ok()
            .filter(|n| n.is_finite())
            .ok_or_else(|| ParseError {
                at: start,
                message: format!("'{text}' is not a number"),
            })
    }

    fn error_literal(&mut self) -> Result<CellError, ParseError> {
        const ERRORS: [&str; 10] = [
            "#GETTING_DATA",
            "#DIV/0!",
            "#VALUE!",
            "#SPILL!",
            "#NULL!",
            "#NAME?",
            "#CALC!",
            "#NUM!",
            "#REF!",
            "#N/A",
        ];
        let rest: String = self.chars[self.pos..].iter().take(14).collect();
        let upper = rest.to_ascii_uppercase();
        for e in ERRORS {
            if upper.starts_with(e) {
                self.pos += e.chars().count();
                return Ok(CellError::parse(e).unwrap_or(CellError::Value));
            }
        }
        Err(self.error("unknown error value"))
    }

    fn array(&mut self) -> Result<Expr, ParseError> {
        self.pos += 1; // {
        let mut rows: Vec<Vec<Expr>> = vec![Vec::new()];
        loop {
            self.skip_spaces();
            let element = match self.peek() {
                Some('"') => Expr::Text(self.string()?.into()),
                Some('#') => Expr::Error(self.error_literal()?),
                Some('-') => {
                    self.pos += 1;
                    Expr::Number(-self.number()?)
                }
                Some('+') => {
                    self.pos += 1;
                    Expr::Number(self.number()?)
                }
                Some(c) if c.is_ascii_digit() || c == '.' => Expr::Number(self.number()?),
                Some(c) if c.is_alphabetic() => {
                    let word = self.word();
                    match word.to_ascii_uppercase().as_str() {
                        "TRUE" => Expr::Bool(true),
                        "FALSE" => Expr::Bool(false),
                        _ => return Err(self.error("arrays may only hold constants")),
                    }
                }
                _ => return Err(self.error("arrays may only hold constants")),
            };
            rows.last_mut().expect("a row").push(element);
            self.skip_spaces();
            match self.peek() {
                Some(',') => self.pos += 1,
                Some(';') => {
                    self.pos += 1;
                    rows.push(Vec::new());
                }
                Some('}') => {
                    self.pos += 1;
                    break;
                }
                _ => return Err(self.error("an array is not closed with '}'")),
            }
        }
        let width = rows[0].len();
        if rows.iter().any(|r| r.len() != width) {
            return Err(self.error("array rows must have the same length"));
        }
        Ok(Expr::Array(rows))
    }

    fn word(&mut self) -> String {
        let start = self.pos;
        while self.peek().is_some_and(is_word_char) {
            self.pos += 1;
        }
        self.chars[start..self.pos].iter().collect()
    }

    fn quoted_sheet(&mut self) -> Result<String, ParseError> {
        self.pos += 1; // '
        let mut out = String::new();
        loop {
            match self.peek() {
                None => return Err(self.error("a sheet name is missing its closing quote")),
                Some('\'') if self.peek_at(1) == Some('\'') => {
                    out.push('\'');
                    self.pos += 2;
                }
                Some('\'') => {
                    self.pos += 1;
                    return Ok(out);
                }
                Some(c) => {
                    out.push(c);
                    self.pos += 1;
                }
            }
        }
    }

    /// A reference (with an optional sheet prefix), a function call, a
    /// boolean, a structured reference or a defined name.
    fn reference_or_name(&mut self) -> Result<Expr, ParseError> {
        let start = self.pos;
        // Sheet prefix: 'Name'! or Name! or Name1:Name2!
        let sheet: Option<String>;
        let mut last_sheet: Option<String> = None;
        if self.peek() == Some('\'') {
            let name = self.quoted_sheet()?;
            let (first, last) = match name.split_once(':') {
                Some((a, b)) => (a.to_owned(), Some(b.to_owned())),
                None => (name, None),
            };
            if self.peek() != Some('!') {
                return Err(self.error("a quoted sheet name must be followed by '!'"));
            }
            self.pos += 1;
            sheet = Some(first);
            last_sheet = last;
        } else {
            let word_start = self.pos;
            let word = self.word();
            match self.peek() {
                Some('!') => {
                    self.pos += 1;
                    sheet = Some(word);
                }
                Some(':') => {
                    // A 3-D sheet range? Look for Word:Word!
                    let save = self.pos;
                    self.pos += 1;
                    let second = self.word();
                    if !second.is_empty() && self.peek() == Some('!') {
                        self.pos += 1;
                        sheet = Some(word);
                        last_sheet = Some(second);
                    } else {
                        self.pos = save;
                        return self.after_word(word_start, word);
                    }
                }
                _ => return self.after_word(word_start, word),
            }
        }
        // After a sheet prefix: an area, or #REF!.
        if self.peek() == Some('#') {
            let e = self.error_literal()?;
            if e != CellError::Ref {
                return Err(self.error("expected a cell reference"));
            }
            return Ok(Expr::Ref(Reference {
                sheet,
                last_sheet,
                area: None,
            }));
        }
        if let Some(r) = self.try_row_range(sheet.clone(), last_sheet.clone())? {
            return Ok(r);
        }
        let word_start = self.pos;
        let word = self.word();
        match self.area_from_word(&word)? {
            Some(area) => Ok(Expr::Ref(Reference {
                sheet,
                last_sheet,
                area: Some(area),
            })),
            None => {
                // Sheet-scoped defined name: Sheet1!MyName.
                if !word.is_empty() && !word.chars().next().is_some_and(|c| c.is_ascii_digit()) {
                    let _ = word_start;
                    return Ok(Expr::Name(format!(
                        "{}!{}",
                        crate::refs::quote_sheet_name(sheet.as_deref().unwrap_or("")),
                        word
                    )));
                }
                Err(ParseError {
                    at: start,
                    message: "expected a cell reference after the sheet name".into(),
                })
            }
        }
    }

    /// A word with no sheet prefix: function, boolean, cell or range,
    /// structured reference, or name.
    fn after_word(&mut self, word_start: usize, word: String) -> Result<Expr, ParseError> {
        if word.is_empty() {
            return Err(ParseError {
                at: word_start,
                message: "expected a value".into(),
            });
        }
        self.skip_spaces_if(|p| p.peek_after_spaces() == Some('('));
        if self.peek() == Some('(') {
            self.pos += 1;
            let name = canonical_function_name(&word);
            let args = self.arguments()?;
            return Ok(Expr::Call(name, args));
        }
        if self.peek() == Some('[') {
            // Table1[Column] or Table1[[#This Row],[Column]]: kept as text.
            let mut depth = 0;
            let mut text = word;
            while let Some(c) = self.peek() {
                text.push(c);
                self.pos += 1;
                match c {
                    '[' => depth += 1,
                    ']' => {
                        depth -= 1;
                        if depth == 0 {
                            break;
                        }
                    }
                    _ => {}
                }
            }
            return Ok(Expr::Structured(text));
        }
        match word.to_ascii_uppercase().as_str() {
            "TRUE" => return Ok(Expr::Bool(true)),
            "FALSE" => return Ok(Expr::Bool(false)),
            _ => {}
        }
        if let Some(area) = self.area_from_word(&word)? {
            return Ok(Expr::Ref(Reference {
                sheet: None,
                last_sheet: None,
                area: Some(area),
            }));
        }
        Ok(Expr::Name(word))
    }

    fn peek_after_spaces(&self) -> Option<char> {
        self.chars[self.pos..].iter().copied().find(|c| *c != ' ')
    }

    fn skip_spaces_if(&mut self, cond: impl Fn(&Self) -> bool) {
        if cond(self) {
            self.skip_spaces();
        }
    }

    /// Given a word already read (`A1`, `$B$2`, `C`), finish an area: a
    /// cell, a range with `:`, or a column range.
    fn area_from_word(&mut self, word: &str) -> Result<Option<Area>, ParseError> {
        if let Some(first) = parse_cell_addr(word) {
            if self.peek() == Some(':') {
                let save = self.pos;
                self.pos += 1;
                let second_word = self.word();
                if let Some(second) = parse_cell_addr(&second_word) {
                    return Ok(Some(Area::Range(first, second)));
                }
                self.pos = save;
            }
            return Ok(Some(Area::Cell(first)));
        }
        if let Some(first) = parse_col_addr(word) {
            if self.peek() == Some(':') {
                let save = self.pos;
                self.pos += 1;
                let second_word = self.word();
                if let Some(second) = parse_col_addr(&second_word) {
                    return Ok(Some(Area::Cols(first, second)));
                }
                self.pos = save;
            }
        }
        Ok(None)
    }

    /// `1:3` or `$1:$3`, if that is what is at the cursor.
    fn try_row_range(&mut self, sheet: Option<String>, last_sheet: Option<String>) -> Result<Option<Expr>, ParseError> {
        let save = self.pos;
        let first = self.row_addr();
        if let Some(first) = first {
            if self.peek() == Some(':') {
                self.pos += 1;
                if let Some(second) = self.row_addr() {
                    if !self.peek().is_some_and(is_word_char) {
                        return Ok(Some(Expr::Ref(Reference {
                            sheet,
                            last_sheet,
                            area: Some(Area::Rows(first, second)),
                        })));
                    }
                }
            }
        }
        self.pos = save;
        Ok(None)
    }

    fn row_addr(&mut self) -> Option<CellAddr> {
        let start = self.pos;
        let abs = self.peek() == Some('$');
        if abs {
            self.pos += 1;
        }
        let digits_start = self.pos;
        while self.peek().is_some_and(|c| c.is_ascii_digit()) {
            self.pos += 1;
        }
        let digits: String = self.chars[digits_start..self.pos].iter().collect();
        match digits.parse::<u32>() {
            Ok(row) if (1..=MAX_ROWS).contains(&row) && !digits.starts_with('0') => Some(CellAddr {
                row: row - 1,
                col: 0,
                row_abs: abs,
                col_abs: false,
            }),
            _ => {
                self.pos = start;
                None
            }
        }
    }

    fn arguments(&mut self) -> Result<Vec<Expr>, ParseError> {
        let mut args = Vec::new();
        self.skip_spaces();
        if self.peek() == Some(')') {
            self.pos += 1;
            return Ok(args);
        }
        loop {
            self.skip_spaces();
            let arg = match self.peek() {
                Some(',') | Some(')') => Expr::Missing,
                _ => self.expr(0, false)?,
            };
            args.push(arg);
            self.skip_spaces();
            match self.peek() {
                Some(',') => self.pos += 1,
                Some(')') => {
                    self.pos += 1;
                    return Ok(args);
                }
                Some(';') => self.pos += 1,
                _ => return Err(self.error("expected ',' or ')' in the function's arguments")),
            }
            if args.len() > 255 {
                return Err(self.error("a function can take at most 255 arguments"));
            }
        }
    }
}

fn is_reference_like(e: &Expr) -> bool {
    match e {
        Expr::Ref(_) | Expr::Name(_) | Expr::Structured(_) => true,
        Expr::Paren(inner) => is_reference_like(inner),
        Expr::Binary(BinaryOp::Range | BinaryOp::Intersect | BinaryOp::Union, _, _) => true,
        Expr::Call(name, _) => matches!(name.as_str(), "INDEX" | "OFFSET" | "INDIRECT" | "CHOOSE" | "IF"),
        _ => false,
    }
}

/// `A1`, `$A1`, `A$1`, `$A$1`.
pub fn parse_cell_addr(word: &str) -> Option<CellAddr> {
    let (cell, col_abs, row_abs) = crate::refs::CellRef::parse_a1(word)?;
    Some(CellAddr {
        row: cell.row,
        col: cell.col,
        row_abs,
        col_abs,
    })
}

/// `A` or `$A`, for column ranges.
fn parse_col_addr(word: &str) -> Option<CellAddr> {
    let (abs, letters) = match word.strip_prefix('$') {
        Some(rest) => (true, rest),
        None => (false, word),
    };
    let col = letters_to_col(letters)?;
    Some(CellAddr {
        row: 0,
        col,
        row_abs: false,
        col_abs: abs,
    })
}

/// Upper case, without the `_xlfn.`/`_xlws.` prefixes OOXML files add to
/// functions newer than Excel 2007.
pub fn canonical_function_name(word: &str) -> String {
    let upper = word.to_ascii_uppercase();
    for prefix in ["_XLFN._XLWS.", "_XLFN.", "_XLWS.", "_XLUDF."] {
        if let Some(rest) = upper.strip_prefix(prefix) {
            return rest.to_owned();
        }
    }
    upper
}

/// Convenience for building text expressions.
pub fn text(s: &str) -> Expr {
    Expr::Text(Arc::from(s))
}
