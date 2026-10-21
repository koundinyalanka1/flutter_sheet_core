//! Opening and saving workbooks in any supported format.

use crate::error::SheetError;
use crate::model::{Format, Imported, Source, Workbook};
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
        // A flat OpenDocument spreadsheet is XML whatever its name: it is
        // read as the package it stands for, and saved as one.
        if let Some(mimetype) = doc_odf::flat_mimetype(&bytes) {
            if !mimetype.starts_with(doc_odf::SPREADSHEET) {
                return Err(SheetError::Unsupported("This OpenDocument file is not a spreadsheet.".into()));
            }
            let package = doc_odf::unflatten(&bytes).map_err(|e| SheetError::Corrupt(format!("This spreadsheet is damaged: {e}.")))?;
            let mut wb = crate::ods::read(package, date_order)?;
            wb.imported = Some(Imported::Fods);
            wb.warnings.insert(0, "This is a flat OpenDocument spreadsheet (.fods). It will be saved as an OpenDocument spreadsheet (.ods).".into());
            return Ok(wb);
        }
        // Excel 97-2003 (and WPS) workbooks, and password-protected XLSX,
        // are compound files.
        if doc_cfb::is_cfb(&bytes) {
            return crate::xls::read(bytes, date_order);
        }
        match detect_format(&bytes, name) {
            Some(Format::Xlsx) | Some(Format::Xlsm) => crate::xlsx::read(bytes, date_order),
            Some(Format::Ods) => crate::ods::read(bytes, date_order),
            Some(Format::Csv) => crate::csv::read(&bytes, name, false, date_order),
            Some(Format::Tsv) => crate::csv::read(&bytes, name, true, date_order),
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

    /// Whether `bytes`, about to be saved in `format`, read back: the package
    /// opens and its workbook part comes out whole (CRC checked). A save runs
    /// this before it replaces the file.
    pub fn check_saved(format: Format, bytes: &[u8]) -> Result<(), SheetError> {
        let corrupt = |e: doc_opc::OpcError| SheetError::Corrupt(format!("The saved workbook did not read back: {e}"));
        match format {
            Format::Xlsx | Format::Xlsm => {
                let opc = doc_opc::Package::open(bytes.to_vec()).map_err(corrupt)?;
                let main = opc.main_part().map_err(corrupt)?;
                opc.read(&main).map_err(corrupt)?;
            }
            Format::Ods => {
                let opc = doc_opc::Package::open(bytes.to_vec()).map_err(corrupt)?;
                opc.read("content.xml").map_err(corrupt)?;
            }
            Format::Csv | Format::Tsv => {}
        }
        Ok(())
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
