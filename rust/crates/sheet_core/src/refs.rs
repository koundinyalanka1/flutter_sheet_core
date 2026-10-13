//! Cell references and ranges in A1 notation.
//!
//! Rows and columns are 0-based internally; A1 notation is 1-based for rows
//! and lettered for columns (A..Z, AA..XFD).

/// The largest sheet Excel allows: 1,048,576 rows by 16,384 columns.
pub const MAX_ROWS: u32 = 1 << 20;
pub const MAX_COLS: u32 = 1 << 14;

/// One cell's position.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Default)]
pub struct CellRef {
    pub row: u32,
    pub col: u32,
}

impl CellRef {
    pub const fn new(row: u32, col: u32) -> Self {
        Self { row, col }
    }

    /// `A1`-style text for this cell.
    pub fn to_a1(self) -> String {
        format!("{}{}", col_to_letters(self.col), self.row + 1)
    }

    /// Parse `A1` or `$A$1`. Returns the cell and whether its column and
    /// row are absolute.
    pub fn parse_a1(text: &str) -> Option<(CellRef, bool, bool)> {
        let bytes = text.as_bytes();
        let mut i = 0;
        let col_abs = bytes.first() == Some(&b'$');
        if col_abs {
            i += 1;
        }
        let letters_start = i;
        while i < bytes.len() && bytes[i].is_ascii_alphabetic() {
            i += 1;
        }
        let col = letters_to_col(&text[letters_start..i])?;
        let row_abs = bytes.get(i) == Some(&b'$');
        if row_abs {
            i += 1;
        }
        let digits = &text[i..];
        if digits.is_empty() || !digits.bytes().all(|b| b.is_ascii_digit()) || digits.starts_with('0') {
            return None;
        }
        let row: u32 = digits.parse().ok()?;
        if row == 0 || row > MAX_ROWS {
            return None;
        }
        Some((CellRef::new(row - 1, col), col_abs, row_abs))
    }

    /// The cell `rows` down and `cols` right, if it is on the sheet.
    pub fn offset(self, rows: i64, cols: i64) -> Option<CellRef> {
        let row = self.row as i64 + rows;
        let col = self.col as i64 + cols;
        if (0..MAX_ROWS as i64).contains(&row) && (0..MAX_COLS as i64).contains(&col) {
            Some(CellRef::new(row as u32, col as u32))
        } else {
            None
        }
    }
}

/// `0 -> "A"`, `25 -> "Z"`, `26 -> "AA"`.
pub fn col_to_letters(col: u32) -> String {
    let mut n = col as u64 + 1;
    let mut out = Vec::new();
    while n > 0 {
        let rem = ((n - 1) % 26) as u8;
        out.push(b'A' + rem);
        n = (n - 1) / 26;
    }
    out.reverse();
    String::from_utf8(out).unwrap_or_default()
}

/// `"A" -> 0`, `"aa" -> 26`; None for anything that is not 1-3 letters
/// within the sheet's width.
pub fn letters_to_col(letters: &str) -> Option<u32> {
    if letters.is_empty() || letters.len() > 3 {
        return None;
    }
    let mut n: u32 = 0;
    for b in letters.bytes() {
        if !b.is_ascii_alphabetic() {
            return None;
        }
        n = n * 26 + (b.to_ascii_uppercase() - b'A' + 1) as u32;
    }
    (n >= 1 && n <= MAX_COLS).then_some(n - 1)
}

/// A rectangle of cells, `start` at the top left and `end` at the bottom
/// right, both inclusive.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct Range {
    pub start: CellRef,
    pub end: CellRef,
}

impl Range {
    /// The rectangle spanned by two corners, in either order.
    pub fn new(a: CellRef, b: CellRef) -> Self {
        Self {
            start: CellRef::new(a.row.min(b.row), a.col.min(b.col)),
            end: CellRef::new(a.row.max(b.row), a.col.max(b.col)),
        }
    }

    pub fn cell(cell: CellRef) -> Self {
        Self { start: cell, end: cell }
    }

    /// Parse `A1`, `A1:B2` (with or without `$`).
    pub fn parse(text: &str) -> Option<Range> {
        match text.split_once(':') {
            Some((a, b)) => Some(Range::new(CellRef::parse_a1(a)?.0, CellRef::parse_a1(b)?.0)),
            None => Some(Range::cell(CellRef::parse_a1(text)?.0)),
        }
    }

    pub fn to_a1(self) -> String {
        if self.start == self.end {
            self.start.to_a1()
        } else {
            format!("{}:{}", self.start.to_a1(), self.end.to_a1())
        }
    }

    pub fn rows(self) -> u32 {
        self.end.row - self.start.row + 1
    }

    pub fn cols(self) -> u32 {
        self.end.col - self.start.col + 1
    }

    pub fn cell_count(self) -> u64 {
        self.rows() as u64 * self.cols() as u64
    }

    pub fn contains(self, cell: CellRef) -> bool {
        (self.start.row..=self.end.row).contains(&cell.row) && (self.start.col..=self.end.col).contains(&cell.col)
    }

    pub fn intersects(self, other: Range) -> bool {
        self.start.row <= other.end.row
            && other.start.row <= self.end.row
            && self.start.col <= other.end.col
            && other.start.col <= self.end.col
    }

    pub fn intersection(self, other: Range) -> Option<Range> {
        self.intersects(other).then(|| Range {
            start: CellRef::new(self.start.row.max(other.start.row), self.start.col.max(other.start.col)),
            end: CellRef::new(self.end.row.min(other.end.row), self.end.col.min(other.end.col)),
        })
    }

    /// The smallest range covering both.
    pub fn union(self, other: Range) -> Range {
        Range {
            start: CellRef::new(self.start.row.min(other.start.row), self.start.col.min(other.start.col)),
            end: CellRef::new(self.end.row.max(other.end.row), self.end.col.max(other.end.col)),
        }
    }

    /// Cells row by row.
    pub fn iter(self) -> impl Iterator<Item = CellRef> {
        (self.start.row..=self.end.row)
            .flat_map(move |row| (self.start.col..=self.end.col).map(move |col| CellRef::new(row, col)))
    }
}

/// Quote a sheet name for use in a formula when it needs quotes
/// (`'My Sheet'!A1`, `Sheet1!A1`).
pub fn quote_sheet_name(name: &str) -> String {
    let plain = !name.is_empty()
        && name.chars().all(|c| c.is_alphanumeric() || c == '_' || c == '.')
        && !name.chars().next().is_some_and(|c| c.is_ascii_digit())
        && CellRef::parse_a1(name).is_none()
        && !looks_like_r1c1(name)
        && !matches!(name.to_ascii_uppercase().as_str(), "TRUE" | "FALSE");
    if plain {
        name.to_owned()
    } else {
        format!("'{}'", name.replace('\'', "''"))
    }
}

fn looks_like_r1c1(name: &str) -> bool {
    let upper = name.to_ascii_uppercase();
    let b = upper.as_bytes();
    if b.first() != Some(&b'R') && b.first() != Some(&b'C') {
        return false;
    }
    upper.trim_start_matches(|c: char| c == 'R' || c == 'C' || c.is_ascii_digit()).is_empty()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn column_letters() {
        for (col, letters) in [(0, "A"), (25, "Z"), (26, "AA"), (51, "AZ"), (52, "BA"), (701, "ZZ"), (702, "AAA"), (16383, "XFD")] {
            assert_eq!(col_to_letters(col), letters);
            assert_eq!(letters_to_col(letters), Some(col));
        }
        assert_eq!(letters_to_col("xfd"), Some(16383));
        assert_eq!(letters_to_col("XFE"), None);
        assert_eq!(letters_to_col("ABCD"), None);
        assert_eq!(letters_to_col(""), None);
    }

    #[test]
    fn a1_parsing() {
        assert_eq!(CellRef::parse_a1("A1"), Some((CellRef::new(0, 0), false, false)));
        assert_eq!(CellRef::parse_a1("$B$3"), Some((CellRef::new(2, 1), true, true)));
        assert_eq!(CellRef::parse_a1("c$10"), Some((CellRef::new(9, 2), false, true)));
        assert_eq!(CellRef::parse_a1("XFD1048576"), Some((CellRef::new(1048575, 16383), false, false)));
        for bad in ["", "A", "1", "A0", "A01", "A1048577", "1A", "A1B", "$$A1"] {
            assert_eq!(CellRef::parse_a1(bad), None, "{bad}");
        }
        assert_eq!(CellRef::new(9, 27).to_a1(), "AB10");
    }

    #[test]
    fn ranges() {
        let r = Range::parse("C3:A1").unwrap();
        assert_eq!(r.to_a1(), "A1:C3");
        assert_eq!((r.rows(), r.cols(), r.cell_count()), (3, 3, 9));
        assert!(r.contains(CellRef::new(1, 1)));
        assert!(!r.contains(CellRef::new(3, 0)));
        let other = Range::parse("B2:D5").unwrap();
        assert_eq!(r.intersection(other).unwrap().to_a1(), "B2:C3");
        assert_eq!(r.union(other).to_a1(), "A1:D5");
        assert_eq!(Range::parse("B2").unwrap().to_a1(), "B2");
        assert_eq!(Range::parse("A1:B2").unwrap().iter().count(), 4);
    }

    #[test]
    fn sheet_name_quoting() {
        assert_eq!(quote_sheet_name("Sheet1"), "Sheet1");
        assert_eq!(quote_sheet_name("My Sheet"), "'My Sheet'");
        assert_eq!(quote_sheet_name("Bob's"), "'Bob''s'");
        assert_eq!(quote_sheet_name("A1"), "'A1'");
        assert_eq!(quote_sheet_name("2024"), "'2024'");
        assert_eq!(quote_sheet_name("R1C1"), "'R1C1'");
        assert_eq!(quote_sheet_name("Données"), "Données");
    }
}
