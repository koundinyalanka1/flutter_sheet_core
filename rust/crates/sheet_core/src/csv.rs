//! CSV and TSV files: one sheet of text fields.
//!
//! Reading detects the encoding and the delimiter (`,` `;` tab `|`, or an
//! Excel `sep=` line), and reads numbers, dates and booleans the way the
//! locale writes them. Each field's exact text is kept while its cell is
//! unchanged, so saving writes `007` back as `007`. Saving uses the same
//! delimiter, encoding, byte-order mark and line endings.

use std::sync::Arc;

use doc_encoding::{decode_detected, Encoding};

use crate::input::parse_date_time;
use crate::model::{Cell, CellExtra, Format, Sheet, Source, Workbook};
use crate::numfmt::DateOrder;
use crate::refs::{CellRef, MAX_COLS, MAX_ROWS};
use crate::style::CellStyle;
use crate::value::{parse_number, Value};

/// How a delimited file was written.
#[derive(Clone, Debug, PartialEq)]
pub struct CsvFormat {
    pub delimiter: char,
    pub encoding: Encoding,
    pub bom: bool,
    pub line_ending: &'static str,
    pub trailing_newline: bool,
    /// The file began with Excel's `sep=;` line.
    pub sep_line: bool,
}

impl Default for CsvFormat {
    fn default() -> Self {
        CsvFormat {
            delimiter: ',',
            encoding: Encoding::Utf8,
            bom: false,
            line_ending: "\r\n",
            trailing_newline: true,
            sep_line: false,
        }
    }
}

/// Split text into records of fields (RFC 4180 quoting). Stops after
/// `limit` records.
pub fn parse_records(text: &str, delimiter: char, limit: usize) -> Vec<Vec<String>> {
    let mut records = Vec::new();
    let mut record = Vec::new();
    let mut field = String::new();
    let mut chars = text.chars().peekable();
    let mut in_quotes = false;
    let mut at_field_start = true;
    while let Some(c) = chars.next() {
        if in_quotes {
            if c == '"' {
                if chars.peek() == Some(&'"') {
                    chars.next();
                    field.push('"');
                } else {
                    in_quotes = false;
                }
            } else {
                field.push(c);
            }
            continue;
        }
        match c {
            '"' if at_field_start => {
                in_quotes = true;
                at_field_start = false;
            }
            c if c == delimiter => {
                record.push(std::mem::take(&mut field));
                at_field_start = true;
            }
            '\r' | '\n' => {
                if c == '\r' && chars.peek() == Some(&'\n') {
                    chars.next();
                }
                record.push(std::mem::take(&mut field));
                records.push(std::mem::take(&mut record));
                at_field_start = true;
                if records.len() >= limit {
                    return records;
                }
            }
            c => {
                field.push(c);
                at_field_start = false;
            }
        }
    }
    if !field.is_empty() || !record.is_empty() {
        record.push(field);
        records.push(record);
    }
    records
}

/// The delimiter that splits the first lines most consistently.
pub fn detect_delimiter(text: &str, default: char) -> char {
    let mut best = (default, 0.0f64);
    for d in [',', ';', '\t', '|'] {
        let records = parse_records(text, d, 30);
        if records.is_empty() {
            continue;
        }
        let counts: Vec<usize> = records.iter().map(Vec::len).collect();
        let mut mode = 0;
        let mut mode_n = 0;
        for &c in &counts {
            let n = counts.iter().filter(|&&x| x == c).count();
            if n > mode_n || (n == mode_n && c > mode) {
                mode = c;
                mode_n = n;
            }
        }
        if mode < 2 {
            continue;
        }
        let consistency = mode_n as f64 / counts.len() as f64;
        let score = consistency * 10.0 + (mode as f64).min(50.0) / 50.0;
        if score > best.1 {
            best = (d, score);
        }
    }
    best.0
}

/// Read a CSV or TSV file into a one-sheet workbook.
pub fn read(bytes: &[u8], name: &str, tsv: bool, date_order: DateOrder) -> Workbook {
    let (decoded, detection) = decode_detected(bytes, Encoding::WINDOWS_1252);
    let mut text = decoded.text.as_str();
    let line_ending = if text.contains("\r\n") {
        "\r\n"
    } else if text.contains('\n') {
        "\n"
    } else if text.contains('\r') {
        "\r"
    } else {
        "\r\n"
    };
    let trailing_newline = text.ends_with('\n') || text.ends_with('\r') || text.is_empty();
    let mut sep_line = false;
    let mut delimiter = if tsv { '\t' } else { ',' };
    if let Some(rest) = text.strip_prefix("sep=") {
        if let Some(d) = rest.chars().next() {
            if !matches!(d, '\r' | '\n') {
                delimiter = d;
                sep_line = true;
                text = rest[d.len_utf8()..].trim_start_matches(['\r', '\n']);
            }
        }
    }
    if !sep_line && !tsv {
        delimiter = detect_delimiter(text, delimiter);
    }
    let records = parse_records(text, delimiter, MAX_ROWS as usize);

    let mut wb = Workbook {
        date_order,
        format: Some(if tsv { Format::Tsv } else { Format::Csv }),
        ..Workbook::default()
    };
    let options = wb.format_options();
    let stem = std::path::Path::new(name)
        .file_stem()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_else(|| "Sheet1".into());
    let sheet_name: String = stem
        .chars()
        .filter(|c| !matches!(c, ':' | '\\' | '/' | '?' | '*' | '[' | ']'))
        .take(31)
        .collect();
    let mut sheet = Sheet::new(if Workbook::valid_sheet_name(&sheet_name) { sheet_name } else { "Sheet1".into() });
    let mut too_wide = false;
    for (r, record) in records.iter().enumerate() {
        for (c, field) in record.iter().enumerate() {
            if c >= MAX_COLS as usize {
                too_wide = true;
                break;
            }
            if field.is_empty() {
                continue;
            }
            let (value, format) = field_value(field, &options);
            let mut cell = Cell::value(value);
            if let Some(f) = format {
                let mut style = CellStyle::base();
                style.num_fmt = f;
                cell.style = wb.styles.intern(style);
            }
            if !matches!(cell.value, Value::Text(_)) {
                cell.extra = Some(Box::new(CellExtra {
                    source_text: Some(Arc::from(field.as_str())),
                    ..CellExtra::default()
                }));
            }
            sheet.cells.entry(r as u32).or_default().insert(c as u32, cell);
        }
    }
    sheet.invalidate_used();
    if too_wide {
        wb.warnings.push(format!("Only the first {MAX_COLS} columns were read."));
    }
    if decoded.errors > 0 {
        wb.warnings.push(format!("{} characters could not be read in {}.", decoded.errors, detection.encoding.name()));
    }
    wb.styles.original = wb.styles.len();
    wb.sheets.push(sheet);
    wb.source = Source::Csv(CsvFormat {
        delimiter,
        encoding: detection.encoding,
        bom: detection.bom_len > 0,
        line_ending,
        trailing_newline,
        sep_line,
    });
    wb
}

/// What a field holds, and the number format its look suggests.
fn field_value(field: &str, options: &crate::numfmt::FormatOptions) -> (Value, Option<String>) {
    let t = field.trim();
    if t.is_empty() {
        return (Value::text(field), None);
    }
    match t.to_ascii_uppercase().as_str() {
        "TRUE" => return (Value::Bool(true), None),
        "FALSE" => return (Value::Bool(false), None),
        _ => {}
    }
    // Long digit strings (phone numbers, account and Aadhaar numbers) stay
    // text, as do numbers with leading zeros, so nothing is lost.
    let digits = t.chars().filter(|c| c.is_ascii_digit()).count();
    let leading_zero = t.len() > 1 && t.starts_with('0') && !t.starts_with("0.") && !t.starts_with("0,");
    if digits > 15 || leading_zero {
        return (Value::text(field), None);
    }
    if let Some(n) = parse_number(t) {
        let fmt = if t.ends_with('%') {
            Some("0%".to_owned())
        } else {
            None
        };
        return (Value::Number(n), fmt);
    }
    if let Some((n, fmt)) = parse_date_time(t, options.date_order, options.date1904) {
        return (Value::Number(n), Some(fmt));
    }
    (Value::text(field), None)
}

/// Write the active sheet as delimited text. Returns the bytes and how many
/// characters the encoding could not hold.
pub fn write(wb: &Workbook, delimiter: Option<char>) -> (Vec<u8>, usize) {
    let format = match &wb.source {
        Source::Csv(f) => f.clone(),
        _ => CsvFormat::default(),
    };
    let delimiter = delimiter.unwrap_or(format.delimiter);
    let sheet = &wb.sheets[wb.active.min(wb.sheets.len().saturating_sub(1))];
    let mut out = String::new();
    if format.sep_line {
        out.push_str("sep=");
        out.push(delimiter);
        out.push_str(format.line_ending);
    }
    if let Some(used) = sheet.used_range() {
        for r in 0..=used.end.row {
            let mut fields: Vec<String> = Vec::new();
            for c in 0..=used.end.col {
                let text = match sheet.cell(CellRef::new(r, c)) {
                    None => String::new(),
                    Some(cell) => field_text(wb, cell),
                };
                fields.push(text);
            }
            while fields.last().is_some_and(String::is_empty) && fields.len() > 1 {
                fields.pop();
            }
            let line: Vec<String> = fields.iter().map(|f| quote(f, delimiter)).collect();
            out.push_str(&line.join(&delimiter.to_string()));
            if r < used.end.row || format.trailing_newline {
                out.push_str(format.line_ending);
            }
        }
    }
    let encoded = format.encoding.encode(&out);
    let mut bytes = Vec::with_capacity(encoded.bytes.len() + 4);
    if format.bom {
        bytes.extend_from_slice(format.encoding.bom());
    }
    bytes.extend_from_slice(&encoded.bytes);
    (bytes, encoded.unmappable)
}

fn field_text(wb: &Workbook, cell: &Cell) -> String {
    if let Some(src) = cell.extra.as_ref().and_then(|e| e.source_text.as_ref()) {
        return src.to_string();
    }
    match &cell.value {
        Value::Text(t) => t.to_string(),
        Value::Empty => String::new(),
        _ => crate::display::shown_text(wb, cell),
    }
}

fn quote(field: &str, delimiter: char) -> String {
    if field.contains(delimiter) || field.contains(['"', '\r', '\n']) || field.starts_with(' ') || field.ends_with(' ') {
        format!("\"{}\"", field.replace('"', "\"\""))
    } else {
        field.to_owned()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn records_and_quotes() {
        let r = parse_records("a,\"b,c\",\"say \"\"hi\"\"\"\r\n1,2\n\"multi\nline\",x", ',', 100);
        assert_eq!(r, vec![vec!["a", "b,c", "say \"hi\""], vec!["1", "2"], vec!["multi\nline", "x"]]);
    }

    #[test]
    fn delimiters() {
        assert_eq!(detect_delimiter("a;b;c\n1;2;3\n", ','), ';');
        assert_eq!(detect_delimiter("a,b\n1,2\n", ';'), ',');
        assert_eq!(detect_delimiter("a\tb\tc\n1\t2\t3", ','), '\t');
        assert_eq!(detect_delimiter("name;amount\n\"Ravi, K\";1,5\n", ','), ';');
    }

    #[test]
    fn round_trip_keeps_text() {
        let src = "Name,Phone,Amount,Date,Code\r\nRavi,09876543210,1500.50,13/10/2026,007\r\nAsha,,\"2,000\",01/01/2026,TRUE\r\n";
        let wb = read(src.as_bytes(), "people.csv", false, DateOrder::Dmy);
        let s = &wb.sheets[0];
        assert_eq!(s.name, "people");
        assert_eq!(s.value(CellRef::new(1, 1)), Value::text("09876543210"));
        assert_eq!(s.value(CellRef::new(1, 2)), Value::Number(1500.5));
        assert_eq!(s.value(CellRef::new(1, 3)), Value::Number(46_308.0));
        assert_eq!(s.value(CellRef::new(2, 2)), Value::Number(2000.0));
        assert_eq!(s.value(CellRef::new(2, 4)), Value::Bool(true));
        let (bytes, unmappable) = write(&wb, None);
        assert_eq!(unmappable, 0);
        assert_eq!(String::from_utf8(bytes).unwrap(), src);
    }

    #[test]
    fn encodings_and_line_endings_survive() {
        let mut src = vec![0xEF, 0xBB, 0xBF];
        src.extend_from_slice("a;b\nCafé;2\n".as_bytes());
        let wb = read(&src, "x.csv", false, DateOrder::Dmy);
        let (bytes, _) = write(&wb, None);
        assert_eq!(bytes, src);
        let latin = b"x,caf\xe9\r\n".to_vec();
        let wb = read(&latin, "y.csv", false, DateOrder::Dmy);
        assert_eq!(wb.sheets[0].value(CellRef::new(0, 1)), Value::text("café"));
        assert_eq!(write(&wb, None).0, latin);
    }
}
