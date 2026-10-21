//! Excel 97-2003 workbooks (`.xls`, BIFF8; also WPS Spreadsheets `.et`)
//! read into the workbook model: sheets, values, formulas (decompiled back
//! to text), fonts, fills, borders, alignment, number formats, column
//! widths, row heights, merged cells and frozen panes.
//!
//! The format cannot be written back, so such a workbook is saved as an
//! Excel workbook (`.xlsx`); the app says so.

mod formula;
mod ftab;

use std::collections::HashMap;
use std::sync::Arc;

use doc_cfb::Cfb;

use crate::error::SheetError;
use crate::model::{Cell, CellExtra, ColInfo, DefinedName, Formula, Imported, RowInfo, Sheet, SheetState, Workbook};
use crate::numfmt::{builtin_format, DateOrder, FormatOptions};
use crate::refs::{CellRef, Range};
use crate::style::{Align, Border, CellStyle, ColorSpec, Edge, Fill, Font, StyleTable, Theme, Underline};
use crate::value::{CellError, Value};

fn corrupt(m: &str) -> SheetError {
    SheetError::Corrupt(m.into())
}

fn u16_at(b: &[u8], i: usize) -> u16 {
    b.get(i..i + 2).map_or(0, |s| u16::from_le_bytes([s[0], s[1]]))
}

fn u32_at(b: &[u8], i: usize) -> u32 {
    b.get(i..i + 4).map_or(0, |s| u32::from_le_bytes([s[0], s[1], s[2], s[3]]))
}

/// One record with its CONTINUE records' data.
struct Record<'a> {
    id: u16,
    /// The record's data, then each continuation's.
    parts: Vec<&'a [u8]>,
}

impl Record<'_> {
    fn data(&self) -> Vec<u8> {
        self.parts.concat()
    }

    fn first(&self) -> &[u8] {
        self.parts[0]
    }
}

fn records(stream: &[u8], from: usize) -> Vec<Record<'_>> {
    let mut out: Vec<Record> = Vec::new();
    let mut i = from;
    while i + 4 <= stream.len() {
        let id = u16_at(stream, i);
        let len = u16_at(stream, i + 2) as usize;
        let data = &stream[i + 4..(i + 4 + len).min(stream.len())];
        if id == 0x003C {
            if let Some(last) = out.last_mut() {
                last.parts.push(data);
                i += 4 + len;
                continue;
            }
        }
        out.push(Record { id, parts: vec![data] });
        i += 4 + len;
        if id == 0x000A {
            // EOF of this substream.
            break;
        }
    }
    out
}

/// Reads strings across record boundaries (the SST).
struct Segments<'a> {
    parts: &'a [&'a [u8]],
    part: usize,
    pos: usize,
}

impl<'a> Segments<'a> {
    fn byte(&mut self) -> Option<u8> {
        loop {
            let p = self.parts.get(self.part)?;
            if self.pos < p.len() {
                self.pos += 1;
                return Some(p[self.pos - 1]);
            }
            self.part += 1;
            self.pos = 0;
        }
    }

    fn u16(&mut self) -> Option<u16> {
        Some(u16::from_le_bytes([self.byte()?, self.byte()?]))
    }

    fn u32(&mut self) -> Option<u32> {
        Some(u32::from_le_bytes([self.byte()?, self.byte()?, self.byte()?, self.byte()?]))
    }

    fn skip(&mut self, mut n: usize) -> Option<()> {
        while n > 0 {
            self.byte()?;
            n -= 1;
        }
        Some(())
    }

    fn at_part_end(&self) -> bool {
        self.parts.get(self.part).is_some_and(|p| self.pos >= p.len())
    }

    /// An XLUnicodeRichExtendedString: characters may continue in the next
    /// record, which then starts with a fresh options byte.
    fn string(&mut self) -> Option<String> {
        let cch = self.u16()? as usize;
        let flags = self.byte()?;
        let mut wide = flags & 0x01 != 0;
        let runs = if flags & 0x08 != 0 { self.u16()? as usize } else { 0 };
        let ext = if flags & 0x04 != 0 { self.u32()? as usize } else { 0 };
        let mut units: Vec<u16> = Vec::with_capacity(cch);
        while units.len() < cch {
            if self.at_part_end() {
                // Continued: the next record restates the width.
                self.part += 1;
                self.pos = 0;
                wide = self.byte()? & 0x01 != 0;
            }
            if wide {
                units.push(self.u16()?);
            } else {
                units.push(self.byte()? as u16);
            }
        }
        self.skip(runs * 4)?;
        self.skip(ext)?;
        Some(String::from_utf16_lossy(&units))
    }
}

/// A string inside one record: 16-bit (XLUnicodeString) or 8-bit
/// (ShortXLUnicodeString) count, then options and characters.
fn read_str(b: &[u8], at: usize, short: bool) -> (String, usize) {
    let (cch, mut i) = if short { (*b.get(at).unwrap_or(&0) as usize, at + 1) } else { (u16_at(b, at) as usize, at + 2) };
    let flags = *b.get(i).unwrap_or(&0);
    i += 1;
    let runs = if flags & 0x08 != 0 {
        let r = u16_at(b, i) as usize;
        i += 2;
        r
    } else {
        0
    };
    let ext = if flags & 0x04 != 0 {
        let e = u32_at(b, i) as usize;
        i += 4;
        e
    } else {
        0
    };
    let s = if flags & 0x01 != 0 {
        let units: Vec<u16> = (0..cch).map(|k| u16_at(b, i + k * 2)).collect();
        i += cch * 2;
        String::from_utf16_lossy(&units)
    } else {
        let s: String = b.get(i..(i + cch).min(b.len())).unwrap_or(&[]).iter().map(|&c| c as char).collect();
        i += cch;
        s
    };
    (s, i + runs * 4 + ext)
}

fn rk(v: u32) -> f64 {
    let n = if v & 0x02 != 0 {
        ((v as i32) >> 2) as f64
    } else {
        f64::from_bits(((v & 0xFFFF_FFFC) as u64) << 32)
    };
    if v & 0x01 != 0 {
        n / 100.0
    } else {
        n
    }
}

fn error_value(code: u8) -> Value {
    Value::Error(match code {
        0x00 => CellError::Null,
        0x07 => CellError::Div0,
        0x0F => CellError::Value,
        0x17 => CellError::Ref,
        0x1D => CellError::Name,
        0x24 => CellError::Num,
        _ => CellError::NA,
    })
}

const BORDERS: [&str; 14] = [
    "", "thin", "medium", "dashed", "dotted", "thick", "double", "hair", "mediumDashed", "dashDot", "mediumDashDot", "dashDotDot", "mediumDashDotDot", "slantDashDot",
];

struct Globals {
    sst: Vec<Arc<str>>,
    fonts: Vec<Font>,
    formats: HashMap<u16, String>,
    xfs: Vec<CellStyle>,
    palette: Option<Vec<u32>>,
    sheets: Vec<(String, usize, SheetState, u8)>,
    xti: Vec<(bool, u16, u16)>,
    supbooks: Vec<(bool, Vec<String>)>,
    names: Vec<(String, Option<usize>, Vec<u8>)>,
    date1904: bool,
    active: usize,
}

fn color(palette: &Option<Vec<u32>>, icv: u16) -> Option<ColorSpec> {
    match icv {
        0..=7 => Some(ColorSpec::Indexed(icv as u8)),
        8..=63 => match palette {
            Some(p) => p.get(icv as usize - 8).map(|c| ColorSpec::Rgb(*c)),
            None => Some(ColorSpec::Indexed(icv as u8)),
        },
        64 | 0x7FFF | 81 => None,
        _ => None,
    }
}

/// Read a BIFF8 workbook from a compound file's bytes.
pub fn read(bytes: Vec<u8>, date_order: DateOrder) -> Result<Workbook, SheetError> {
    let cfb = Cfb::open(bytes).map_err(|e| SheetError::Corrupt(e.to_string()))?;
    if cfb.exists("EncryptedPackage") {
        return Err(SheetError::Encrypted);
    }
    let stream = match cfb.stream("Workbook") {
        Ok(s) => s,
        Err(_) if cfb.exists("Book") => {
            return Err(SheetError::Unsupported("This is an Excel 95 workbook, which DocHelper cannot open yet.".into()));
        }
        Err(_) => return Err(corrupt("This file has no workbook in it.")),
    };
    // The workbook globals.
    let recs = records(&stream, 0);
    let first = recs.first().ok_or_else(|| corrupt("empty workbook stream"))?;
    if first.id != 0x0809 || u16_at(first.first(), 0) != 0x0600 {
        return Err(SheetError::Unsupported("This Excel file is older than Excel 97, which DocHelper cannot open yet.".into()));
    }
    let mut g = Globals {
        sst: Vec::new(),
        fonts: Vec::new(),
        formats: HashMap::new(),
        xfs: Vec::new(),
        palette: None,
        sheets: Vec::new(),
        xti: Vec::new(),
        supbooks: Vec::new(),
        names: Vec::new(),
        date1904: false,
        active: 0,
    };
    let mut raw_xfs: Vec<Vec<u8>> = Vec::new();
    for r in &recs {
        let d = r.first();
        match r.id {
            0x002F => return Err(SheetError::Encrypted),
            0x0022 => g.date1904 = u16_at(d, 0) == 1,
            0x0031 => {
                let height = u16_at(d, 0);
                let grbit = u16_at(d, 2);
                let icv = u16_at(d, 4);
                let weight = u16_at(d, 6);
                let sss = u16_at(d, 8);
                let uls = *d.get(10).unwrap_or(&0);
                let (name, _) = read_str(d, 14, true);
                g.fonts.push(Font {
                    name,
                    size: height as u32 * 5,
                    bold: weight >= 600,
                    italic: grbit & 0x02 != 0,
                    underline: match uls {
                        1 | 0x21 => Underline::parse("single"),
                        2 | 0x22 => Underline::parse("double"),
                        _ => Underline::None,
                    },
                    strike: grbit & 0x08 != 0,
                    color: None,
                    vert_align: match sss {
                        1 => Some("superscript".into()),
                        2 => Some("subscript".into()),
                        _ => None,
                    },
                    family: None,
                    scheme: None,
                });
                // The colour is resolved after PALETTE.
                g.fonts.last_mut().unwrap().family = Some(icv.to_string());
            }
            0x041E => {
                let id = u16_at(d, 0);
                let data = r.data();
                let (code, _) = read_str(&data, 2, false);
                g.formats.insert(id, code);
            }
            0x00E0 => raw_xfs.push(d.to_vec()),
            0x0092 => {
                let n = u16_at(d, 0) as usize;
                g.palette = Some((0..n).map(|k| {
                    let o = 2 + k * 4;
                    (*d.get(o).unwrap_or(&0) as u32) << 16 | (*d.get(o + 1).unwrap_or(&0) as u32) << 8 | *d.get(o + 2).unwrap_or(&0) as u32
                }).collect());
            }
            0x0085 => {
                let pos = u32_at(d, 0) as usize;
                let state = match d.get(4).map(|v| v & 0x03) {
                    Some(1) => SheetState::Hidden,
                    Some(2) => SheetState::VeryHidden,
                    _ => SheetState::Visible,
                };
                let kind = *d.get(5).unwrap_or(&0);
                let (name, _) = read_str(d, 6, true);
                g.sheets.push((name, pos, state, kind));
            }
            0x00FC => {
                let mut seg = Segments { parts: &r.parts, part: 0, pos: 8 };
                let unique = u32_at(d, 4) as usize;
                for _ in 0..unique.min(10_000_000) {
                    match seg.string() {
                        Some(s) => g.sst.push(Arc::from(s.as_str())),
                        None => break,
                    }
                }
            }
            0x01AE => {
                // SUPBOOK: this workbook (internal) or an external one.
                let internal = d.len() == 4 && u16_at(d, 2) == 0x0401;
                g.supbooks.push((internal, Vec::new()));
            }
            0x0023 => {
                // EXTERNNAME, in the last SUPBOOK.
                let data = r.data();
                let (name, _) = read_str(&data, 6, true);
                if let Some(s) = g.supbooks.last_mut() {
                    s.1.push(name);
                }
            }
            0x0017 => {
                let n = u16_at(d, 0) as usize;
                for k in 0..n {
                    let o = 2 + k * 6;
                    let book = u16_at(d, o) as usize;
                    let internal = g.supbooks.get(book).is_some_and(|b| b.0);
                    g.xti.push((internal, u16_at(d, o + 2), u16_at(d, o + 4)));
                }
            }
            0x0018 => {
                let data = r.data();
                let grbit = u16_at(&data, 0);
                let cch = *data.get(3).unwrap_or(&0) as usize;
                let cce = u16_at(&data, 4) as usize;
                let itab = u16_at(&data, 8) as usize;
                let flags = *data.get(14).unwrap_or(&0);
                let mut i = 15;
                let name = if flags & 1 != 0 {
                    let units: Vec<u16> = (0..cch).map(|k| u16_at(&data, i + k * 2)).collect();
                    i += cch * 2;
                    String::from_utf16_lossy(&units)
                } else {
                    let s: String = data.get(i..i + cch).unwrap_or(&[]).iter().map(|&c| c as char).collect();
                    i += cch;
                    s
                };
                let name = if grbit & 0x0020 != 0 {
                    // Built-in names are stored as one code.
                    match name.chars().next().map(|c| c as u32) {
                        Some(0x06) => "_xlnm.Print_Area".into(),
                        Some(0x07) => "_xlnm.Print_Titles".into(),
                        Some(0x0D) => "_xlnm._FilterDatabase".into(),
                        Some(0x05) => "_xlnm.Extract".into(),
                        Some(0x04) => "_xlnm.Database".into(),
                        Some(0x03) => "_xlnm.Criteria".into(),
                        _ => format!("_xlnm.{name}"),
                    }
                } else {
                    name
                };
                let rgce = data.get(i..i + cce).unwrap_or(&[]).to_vec();
                g.names.push((name, itab.checked_sub(1), rgce));
            }
            0x003D => g.active = u16_at(d, 10) as usize,
            _ => {}
        }
    }
    // Fonts: colours now that the palette is known; index 4 is skipped.
    for f in &mut g.fonts {
        let icv: u16 = f.family.take().and_then(|v| v.parse().ok()).unwrap_or(0x7FFF);
        f.color = color(&g.palette, icv);
    }
    let options = FormatOptions { date_order, date1904: g.date1904 };
    for d in &raw_xfs {
        let ifnt = u16_at(d, 0) as usize;
        let ifmt = u16_at(d, 2);
        let alc = *d.get(6).unwrap_or(&0);
        let trot = *d.get(7).unwrap_or(&0);
        let indent = *d.get(8).unwrap_or(&0);
        let b1 = u32_at(d, 10);
        let b2 = u32_at(d, 14);
        let fills = u16_at(d, 18);
        let font = g.fonts.get(if ifnt >= 4 { ifnt - 1 } else { ifnt }).cloned().unwrap_or_default();
        let num_fmt = g.formats.get(&ifmt).cloned().or_else(|| builtin_format(ifmt as u32, &options)).unwrap_or_else(|| "General".into());
        let edge = |style: u32, icv: u32| -> Option<Edge> {
            let s = BORDERS.get(style as usize).copied().unwrap_or("");
            if s.is_empty() {
                return None;
            }
            Some(Edge { style: s.into(), color: color(&g.palette, icv as u16) })
        };
        let pattern = (b2 >> 26) & 0x3F;
        let fg = (fills & 0x7F) as u16;
        let bg = ((fills >> 7) & 0x7F) as u16;
        g.xfs.push(CellStyle {
            num_fmt,
            font,
            fill: match pattern {
                0 => Fill::none(),
                1 => Fill { pattern: "solid".into(), fg: color(&g.palette, fg), bg: color(&g.palette, bg) },
                _ => Fill { pattern: "gray50".into(), fg: color(&g.palette, fg), bg: color(&g.palette, bg) },
            },
            border: Border {
                left: edge(b1 & 0xF, (b1 >> 16) & 0x7F),
                right: edge((b1 >> 4) & 0xF, (b1 >> 23) & 0x7F),
                top: edge((b1 >> 8) & 0xF, b2 & 0x7F),
                bottom: edge((b1 >> 12) & 0xF, (b2 >> 7) & 0x7F),
                diagonal: None,
                diagonal_up: false,
                diagonal_down: false,
            },
            align: Align {
                horizontal: match alc & 0x07 {
                    1 => "left",
                    2 => "center",
                    3 => "right",
                    4 => "fill",
                    5 => "justify",
                    6 => "centerContinuous",
                    7 => "distributed",
                    _ => "",
                }
                .into(),
                vertical: match (alc >> 4) & 0x07 {
                    0 => "top",
                    1 => "center",
                    3 => "justify",
                    4 => "distributed",
                    _ => "",
                }
                .into(),
                wrap: alc & 0x08 != 0,
                indent: indent & 0x0F,
                rotation: trot as i16,
                shrink: indent & 0x10 != 0,
            },
            locked: u16_at(d, 4) & 0x01 != 0,
            hidden: u16_at(d, 4) & 0x02 != 0,
        });
    }
    if g.xfs.is_empty() {
        g.xfs.push(CellStyle::base());
    }
    let theme = Theme::default();
    let mut wb = Workbook {
        styles: StyleTable::new(g.xfs.clone(), theme),
        date1904: g.date1904,
        date_order,
        format: None,
        imported: Some(Imported::Xls),
        ..Workbook::default()
    };
    wb.warnings.push("This is an Excel 97-2003 workbook (.xls). It will be saved as an Excel workbook (.xlsx).".into());

    let sheet_names: Vec<String> = g.sheets.iter().map(|s| s.0.clone()).collect();
    let defined: Vec<String> = g.names.iter().map(|n| n.0.clone()).collect();
    let supbooks = g.supbooks.clone();
    let xti = g.xti.clone();
    let external = move |ixti: u16, n: u16| -> Option<String> {
        let book = xti.get(ixti as usize).map(|_| ())?;
        let _ = book;
        // Add-in functions live in an external SUPBOOK's names.
        supbooks.iter().filter(|b| !b.0).find_map(|b| b.1.get((n as usize).checked_sub(1)?).cloned())
    };
    let names = formula::Names { sheets: &sheet_names, xti: &g.xti, defined: &defined, external: &external };

    let mut kept_values = 0usize;
    for (name, pos, state, kind) in &g.sheets {
        let mut sheet = Sheet::new(name.clone());
        sheet.state = *state;
        sheet.modified = true;
        if *kind != 0 {
            wb.warnings.push(format!("'{name}' is a chart or macro sheet; it is left out."));
            continue;
        }
        let si = wb.sheets.len();
        read_sheet(&stream, *pos, &mut sheet, &g, &names, si, &mut kept_values);
        wb.sheets.push(sheet);
    }
    if wb.sheets.is_empty() {
        return Err(corrupt("The workbook has no sheets."));
    }
    for (name, sheet, rgce) in &g.names {
        if name.starts_with("_xlnm._FilterDatabase") {
            continue;
        }
        let text = formula::decompile(rgce, CellRef { row: 0, col: 0 }, false, &names);
        if let Some(t) = text {
            wb.names.push(DefinedName::new(name, *sheet, &t));
        }
    }
    if kept_values > 0 {
        wb.warnings.push(format!("{kept_values} formulas use features DocHelper cannot read from .xls files; their last values are kept."));
    }
    wb.active = g.active.min(wb.sheets.len() - 1);
    wb.structure_changed = true;
    wb.styles_changed = true;
    wb.dirty = false;
    Ok(wb)
}

fn read_sheet(stream: &[u8], pos: usize, sheet: &mut Sheet, g: &Globals, names: &formula::Names, _si: usize, kept: &mut usize) {
    let recs = records(stream, pos);
    // Shared and array formulas by their anchor cell.
    let mut shared: Vec<(Range, Vec<u8>)> = Vec::new();
    let mut arrays: Vec<(Range, Vec<u8>)> = Vec::new();
    let mut pending_string: Option<CellRef> = None;
    let mut formulas: Vec<(CellRef, Vec<u8>, usize)> = Vec::new();
    for (k, r) in recs.iter().enumerate() {
        let d = r.first();
        let at = |d: &[u8]| CellRef { row: u16_at(d, 0) as u32, col: u16_at(d, 2) as u32 };
        let put = |sheet: &mut Sheet, at: CellRef, value: Value, xf: u16| {
            sheet.set(at, Cell { value, style: xf as u32, ..Cell::default() });
        };
        match r.id {
            0x0203 => put(sheet, at(d), Value::Number(f64::from_le_bytes(d.get(6..14).and_then(|s| s.try_into().ok()).unwrap_or([0; 8]))), u16_at(d, 4)),
            0x027E => put(sheet, at(d), Value::Number(rk(u32_at(d, 6))), u16_at(d, 4)),
            0x00BD => {
                let row = u16_at(d, 0) as u32;
                let first = u16_at(d, 2) as u32;
                let n = (d.len().saturating_sub(6)) / 6;
                for k in 0..n {
                    let o = 4 + k * 6;
                    put(sheet, CellRef { row, col: first + k as u32 }, Value::Number(rk(u32_at(d, o + 2))), u16_at(d, o));
                }
            }
            0x00FD => {
                let i = u32_at(d, 6) as usize;
                let v = g.sst.get(i).cloned().unwrap_or_else(|| Arc::from(""));
                put(sheet, at(d), Value::Text(v), u16_at(d, 4));
            }
            0x0204 => {
                let data = r.data();
                let (s, _) = read_str(&data, 6, false);
                put(sheet, at(&data), Value::text(s), u16_at(&data, 4));
            }
            0x0205 => {
                let v = *d.get(6).unwrap_or(&0);
                let value = if d.get(7) == Some(&1) { error_value(v) } else { Value::Bool(v != 0) };
                put(sheet, at(d), value, u16_at(d, 4));
            }
            0x0201 => {
                let xf = u16_at(d, 4);
                if xf != 0 {
                    put(sheet, at(d), Value::Empty, xf);
                }
            }
            0x00BE => {
                let row = u16_at(d, 0) as u32;
                let first = u16_at(d, 2) as u32;
                let n = (d.len().saturating_sub(6)) / 2;
                for k in 0..n {
                    let xf = u16_at(d, 4 + k * 2);
                    if xf != 0 {
                        put(sheet, CellRef { row, col: first + k as u32 }, Value::Empty, xf);
                    }
                }
            }
            0x0006 => {
                let data = r.data();
                let cell = at(&data);
                let xf = u16_at(&data, 4);
                let num = &data[6.min(data.len())..14.min(data.len())];
                let value = if num.len() == 8 && num[6] == 0xFF && num[7] == 0xFF {
                    match num[0] {
                        0 => {
                            pending_string = Some(cell);
                            Value::text("")
                        }
                        1 => Value::Bool(num[2] != 0),
                        2 => error_value(num[2]),
                        _ => Value::text(""),
                    }
                } else {
                    Value::Number(f64::from_le_bytes(num.try_into().unwrap_or([0; 8])))
                };
                put(sheet, cell, value, xf);
                let cce = u16_at(&data, 20) as usize;
                let rgce = data.get(22..22 + cce).unwrap_or(&[]).to_vec();
                formulas.push((cell, rgce, k));
            }
            0x0207 => {
                if let Some(cell) = pending_string.take() {
                    let data = r.data();
                    let (s, _) = read_str(&data, 0, false);
                    if let Some(c) = sheet.cell_mut(cell) {
                        c.value = Value::text(s);
                    }
                }
            }
            0x04BC | 0x0221 => {
                let data = r.data();
                let range = Range {
                    start: CellRef { row: u16_at(&data, 0) as u32, col: *data.get(4).unwrap_or(&0) as u32 },
                    end: CellRef { row: u16_at(&data, 2) as u32, col: *data.get(5).unwrap_or(&0) as u32 },
                };
                if r.id == 0x04BC {
                    let cce = u16_at(&data, 8) as usize;
                    shared.push((range, data.get(10..10 + cce).unwrap_or(&[]).to_vec()));
                } else {
                    let cce = u16_at(&data, 12) as usize;
                    arrays.push((range, data.get(14..14 + cce).unwrap_or(&[]).to_vec()));
                }
            }
            0x0208 => {
                let row = u16_at(d, 0) as u32;
                let h = u16_at(d, 6);
                let grbit = u16_at(d, 12);
                let info = RowInfo {
                    height: if h & 0x8000 == 0 { Some(h as f64 / 20.0) } else { None },
                    hidden: grbit & 0x20 != 0,
                    custom_height: grbit & 0x40 != 0,
                    style: if grbit & 0x80 != 0 { Some((u16_at(d, 14) & 0x0FFF) as u32) } else { None },
                    extra: Vec::new(),
                };
                if info.height.is_some() || info.hidden || info.style.is_some() {
                    sheet.rows.insert(row, info);
                }
            }
            0x007D => {
                let first = u16_at(d, 0) as u32;
                let last = (u16_at(d, 2) as u32).min(16383);
                let width = u16_at(d, 4) as f64 / 256.0;
                let xf = u16_at(d, 6) as u32;
                let opts = u16_at(d, 8);
                for c in first..=last.min(first + 16383) {
                    sheet.cols.insert(
                        c,
                        ColInfo {
                            width: Some(width),
                            hidden: opts & 0x01 != 0,
                            style: (xf != 15 && xf != 0).then_some(xf),
                            custom_width: true,
                            extra: Vec::new(),
                        },
                    );
                }
            }
            0x0055 => sheet.default_col_width = u16_at(d, 0) as f64 + 0.71,
            0x0225 => sheet.default_row_height = u16_at(d, 2) as f64 / 20.0,
            0x00E5 => {
                let n = u16_at(d, 0) as usize;
                for k in 0..n {
                    let o = 2 + k * 8;
                    sheet.merges.push(Range {
                        start: CellRef { row: u16_at(d, o) as u32, col: u16_at(d, o + 4) as u32 },
                        end: CellRef { row: u16_at(d, o + 2) as u32, col: u16_at(d, o + 6) as u32 },
                    });
                }
            }
            0x023E => {
                let grbit = u16_at(d, 0);
                sheet.show_grid = grbit & 0x0002 != 0;
            }
            0x0041 => {
                let x = u16_at(d, 0) as u32;
                let y = u16_at(d, 2) as u32;
                if x > 0 || y > 0 {
                    sheet.freeze = Some(CellRef { row: y, col: x });
                }
            }
            _ => {}
        }
    }
    // Formulas, now that shared and array formulas are known.
    for (cell, rgce, _) in formulas {
        let (text, array) = if rgce.first() == Some(&0x01) {
            let anchor = CellRef { row: u16_at(&rgce, 1) as u32, col: u16_at(&rgce, 3) as u32 };
            if let Some((range, tokens)) = arrays.iter().find(|(r, _)| r.start == anchor) {
                if cell == anchor {
                    (formula::decompile(tokens, cell, false, names), Some(*range))
                } else {
                    // A member of the array: its value only.
                    if let Some(c) = sheet.cell_mut(cell) {
                        c.extra = Some(Box::new(CellExtra { array_member: Some(anchor), ..CellExtra::default() }));
                    }
                    continue;
                }
            } else if let Some((_, tokens)) = shared.iter().find(|(r, _)| r.start == anchor || (r.start.row <= cell.row && cell.row <= r.end.row && r.start.col <= cell.col && cell.col <= r.end.col)) {
                (formula::decompile(tokens, cell, true, names), None)
            } else {
                (None, None)
            }
        } else {
            (formula::decompile(&rgce, cell, false, names), None)
        };
        match text {
            Some(t) => {
                let mut f = Formula::new(&t);
                f.array = array;
                if let Some(c) = sheet.cell_mut(cell) {
                    c.formula = Some(Box::new(f));
                }
            }
            None => *kept += 1,
        }
    }
    sheet.invalidate_used();
}

#[cfg(test)]
mod tests;
