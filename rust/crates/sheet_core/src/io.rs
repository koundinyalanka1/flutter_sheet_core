//! Opening and saving workbooks in any supported format.

use crate::error::SheetError;
use crate::model::{Format, Source, Workbook};
use crate::numfmt::DateOrder;

/// Sniff the format of a file from its bytes and name.
pub fn detect_format(bytes: &[u8], name: &str) -> Option<Format> {
    let ext = std::path::Path::new(name)
        .extension()
        .map(|e| e.to_string_lossy().to_ascii_lowercase())
        .unwrap_or_default();
    if bytes.starts_with(b"PK\x03\x04") {
        // An ODF package names itself in its first entry.
        if bytes.len() > 38 && &bytes[30..38] == b"mimetype" {
            let head = &bytes[38..bytes.len().min(120)];
            if head.windows(11).any(|w| w == b"spreadsheet") {
                return Some(Format::Ods);
            }
        }
        return Some(match ext.as_str() {
            "ods" | "ots" => Format::Ods,
            "xlsm" | "xltm" => Format::Xlsm,
            _ => Format::Xlsx,
        });
    }
    match ext.as_str() {
        "tsv" | "tab" => Some(Format::Tsv),
        "csv" | "txt" => Some(Format::Csv),
        "xlsx" | "xlsm" | "xltx" | "xltm" => Some(Format::Xlsx),
        "ods" => Some(Format::Ods),
        _ => None,
    }
}

impl Workbook {
    /// Read a workbook from a file's bytes; `name` gives the extension.
    pub fn open(bytes: Vec<u8>, name: &str, date_order: DateOrder) -> Result<Workbook, SheetError> {
        match detect_format(&bytes, name) {
            Some(Format::Xlsx) | Some(Format::Xlsm) => crate::xlsx::read(bytes, date_order),
            Some(Format::Ods) => crate::ods::read(bytes, date_order),
            Some(Format::Csv) => Ok(crate::csv::read(&bytes, name, false, date_order)),
            Some(Format::Tsv) => Ok(crate::csv::read(&bytes, name, true, date_order)),
            None => Err(SheetError::Unsupported("This file is not a spreadsheet DocHelper can open.".into())),
        }
    }

    /// A new workbook for the locale.
    pub fn blank(date_order: DateOrder) -> Workbook {
        Workbook {
            date_order,
            ..Workbook::new()
        }
    }

    /// The file's bytes in a format. Nothing about the workbook changes
    /// until [`Workbook::saved`] is called with them.
    pub fn to_bytes(&mut self, format: Format) -> Result<Vec<u8>, SheetError> {
        match format {
            Format::Xlsx | Format::Xlsm => crate::xlsx::write(self, format),
            Format::Ods => crate::ods::write(self),
            Format::Csv => Ok(crate::csv::write(self, Some(self.csv_delimiter(','))).0),
            Format::Tsv => Ok(crate::csv::write(self, Some('\t')).0),
        }
    }

    fn csv_delimiter(&self, default: char) -> char {
        match &self.source {
            Source::Csv(f) if f.delimiter != '\t' => f.delimiter,
            _ => default,
        }
    }

    /// The workbook was written to its file in `format` as `bytes`: from now
    /// on saving builds on that file.
    pub fn saved(&mut self, format: Format, bytes: Vec<u8>) -> Result<(), SheetError> {
        match format {
            Format::Xlsx | Format::Xlsm => match &mut self.source {
                Source::Xlsx(pkg) => {
                    pkg.opc = doc_opc::Package::open(bytes).map_err(|e| SheetError::Corrupt(e.to_string()))?;
                }
                _ => {
                    let mut fresh = crate::xlsx::read(bytes, self.date_order)?;
                    for (mine, theirs) in self.sheets.iter_mut().zip(fresh.sheets.iter_mut()) {
                        mine.xlsx = theirs.xlsx.take();
                    }
                    self.source = std::mem::take(&mut fresh.source);
                }
            },
            Format::Ods => {
                let mut fresh = crate::ods::read(bytes, self.date_order)?;
                for (mine, theirs) in self.sheets.iter_mut().zip(fresh.sheets.iter_mut()) {
                    mine.ods = theirs.ods.take();
                }
                self.source = std::mem::take(&mut fresh.source);
            }
            Format::Csv | Format::Tsv => {
                let previous = match &self.source {
                    Source::Csv(f) => f.clone(),
                    _ => crate::csv::CsvFormat::default(),
                };
                self.source = Source::Csv(crate::csv::CsvFormat {
                    delimiter: if format == Format::Tsv { '\t' } else { self.csv_delimiter(',') },
                    ..previous
                });
            }
        }
        self.format = Some(format);
        self.styles.original = self.styles.len();
        for s in &mut self.sheets {
            s.modified = false;
        }
        self.structure_changed = false;
        self.styles_changed = false;
        self.dirty = false;
        Ok(())
    }
}
