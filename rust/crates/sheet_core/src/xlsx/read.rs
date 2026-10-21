//! Reading an XLSX package into the model.

use std::collections::HashMap;
use std::sync::Arc;

use doc_opc::rel;
use doc_xml::{ns, Document, Event, Reader};

use super::styles::{read_styles, read_theme};
use super::{Package, SheetPart, SstEntry};
use crate::error::SheetError;
use crate::formula::adjust::offset_references;
use crate::model::{Cell, CellExtra, ColInfo, DefinedName, Format, Formula, RowInfo, Sheet, SheetState, Source, Workbook};
use crate::numfmt::{DateOrder, FormatOptions};
use crate::refs::{CellRef, Range, MAX_COLS, MAX_ROWS};
use crate::style::{StyleTable, Theme};
use crate::value::{CellError, Value};

/// Most cells a workbook may hold before reading stops. A 32-bit phone's
/// process cannot hold 20 million cells (each takes tens of bytes), so it
/// stops at 4 million, the plan's phone budget.
#[cfg(target_pointer_width = "64")]
const MAX_CELLS: usize = 20_000_000;
#[cfg(not(target_pointer_width = "64"))]
const MAX_CELLS: usize = 4_000_000;

fn corrupt(e: impl Into<SheetError>) -> SheetError {
    e.into()
}

fn xml_err(part: &str) -> impl Fn(doc_xml::XmlError) -> SheetError + '_ {
    move |e| if e.is_too_large() { SheetError::TooLarge(format!("{part}: {e}")) } else { SheetError::Corrupt(format!("{part}: {e}")) }
}

/// Read an XLSX (or XLSM/XLTX) file.
pub fn read(bytes: Vec<u8>, date_order: DateOrder) -> Result<Workbook, SheetError> {
    read_with_limits(bytes, date_order, crate::model::ReadLimits::default())
}

pub fn read_with_limits(bytes: Vec<u8>, date_order: DateOrder, limits: crate::model::ReadLimits) -> Result<Workbook, SheetError> {
    if bytes.starts_with(&[0xD0, 0xCF, 0x11, 0xE0]) {
        // An OLE2 container: an encrypted OOXML file (or a legacy .xls).
        return Err(SheetError::Encrypted);
    }
    let opc = doc_opc::Package::open_with_limits(bytes, limits.package()).map_err(|e| match e {
        doc_opc::OpcError::Zip(doc_zip::ZipError::NotZip) => SheetError::Corrupt("This is not an Excel workbook (.xlsx).".into()),
        other => corrupt(other),
    })?;
    let workbook_part = opc.main_part().map_err(corrupt)?;
    let workbook = opc.read_xml(&workbook_part).map_err(corrupt)?;
    let workbook_rels = opc.rels(&workbook_part).map_err(corrupt)?;
    let main_content_type = opc.content_type(&workbook_part).unwrap_or_else(|| super::ct::WORKBOOK.into());

    let mut pkg = Package {
        opc,
        workbook_part,
        workbook,
        workbook_rels,
        styles_part: None,
        styles: None,
        xf_count: 0,
        fonts: Vec::new(),
        fills: Vec::new(),
        borders: Vec::new(),
        num_fmts: Vec::new(),
        sst_part: None,
        sst: Vec::new(),
        sst_root: None,
        main_content_type,
        original_active: 0,
    };

    let wb_root = pkg.workbook.root.clone();
    let date1904 = wb_root
        .elements()
        .find(|e| e.local() == "workbookPr")
        .and_then(|e| e.attr(ns::NONE, "date1904"))
        .is_some_and(|v| v == "1" || v == "true");
    let options = FormatOptions { date_order, date1904 };

    // Theme and styles.
    let theme = match pkg.workbook_rels.first_part(rel::THEME) {
        Some(p) if pkg.opc.exists(&p) => read_theme(&pkg.opc.read_xml(&p).map_err(corrupt)?),
        _ => Theme::default(),
    };
    if let Some(p) = pkg.workbook_rels.first_part(rel::STYLES) {
        if pkg.opc.exists(&p) {
            pkg.styles = Some(pkg.opc.read_xml(&p).map_err(corrupt)?);
            pkg.styles_part = Some(p);
        }
    }
    let styles = read_styles(&mut pkg, &theme, &options);

    // Shared strings.
    if let Some(p) = pkg.workbook_rels.first_part(rel::SHARED_STRINGS) {
        if pkg.opc.exists(&p) {
            let text = pkg.opc.read_text(&p).map_err(corrupt)?;
            let (entries, root) = read_shared_strings(&text, limits).map_err(xml_err(&p))?;
            pkg.sst = entries;
            pkg.sst_root = Some(root);
            pkg.sst_part = Some(p);
        }
    }

    let mut wb = Workbook {
        limits,
        styles: StyleTable::new(styles, theme),
        date1904,
        date_order,
        format: Some(Format::Xlsx),
        ..Workbook::default()
    };
    if !pkg.opc.duplicate_parts().is_empty() {
        wb.warnings.push("This workbook has duplicate parts. The first copy of each part is used and kept when saving.".into());
    }
    if pkg.main_content_type.contains("macroEnabled") {
        wb.format = Some(Format::Xlsm);
    }

    // Sheets.
    let sheets_el = wb_root.elements().find(|e| e.local() == "sheets");
    let mut cell_total = 0usize;
    for (index, s) in sheets_el.into_iter().flat_map(|e| e.elements().filter(|e| e.local() == "sheet")).enumerate() {
        let name = s.attr(ns::NONE, "name").unwrap_or("Sheet").to_owned();
        let rel_id = s.attr(ns::R, "id").unwrap_or_default().to_owned();
        let sheet_id = s.attr(ns::NONE, "sheetId").and_then(|v| v.parse().ok()).unwrap_or(index as u32 + 1);
        let state = match s.attr(ns::NONE, "state") {
            Some("hidden") => SheetState::Hidden,
            Some("veryHidden") => SheetState::VeryHidden,
            _ => SheetState::Visible,
        };
        let sheet_attrs: Vec<(String, String)> = s
            .attrs
            .iter()
            .filter(|a| !(a.ns == ns::NONE && matches!(a.local(), "name" | "sheetId" | "state")) && !(a.ns == ns::R && a.local() == "id"))
            .map(|a| (a.name.clone(), a.value.clone()))
            .collect();
        let rel = pkg.workbook_rels.by_id(&rel_id);
        let path = rel.as_ref().and_then(|r| pkg.workbook_rels.part_of(r)).unwrap_or_default();
        let chart = rel.as_ref().is_some_and(|r| !rel::is(&r.rel_type, rel::WORKSHEET));
        let mut sheet = Sheet::new(name);
        sheet.state = state;
        let mut part = SheetPart {
            path: path.clone(),
            rel_id,
            sheet_id,
            sheet_attrs,
            doc: None,
            chart,
            original_freeze: None,
            tables: Vec::new(),
            shifts: Vec::new(),
            detached: false,
        };
        if !chart && pkg.opc.exists(&path) {
            let text = pkg.opc.read_text(&path).map_err(corrupt)?;
            read_worksheet(&text, &path, &mut sheet, &mut part, &pkg, &mut wb, index, &mut cell_total)?;
            // Tables on the sheet.
            if let Ok(rels) = pkg.opc.rels(&path) {
                for r in rels.of_kind(rel::TABLE) {
                    if let Some(tp) = rels.part_of(&r) {
                        if let Ok(doc) = pkg.opc.read_xml(&tp) {
                            if let Some(range) = doc.root.attr(ns::NONE, "ref").and_then(Range::parse) {
                                part.tables.push((tp, range));
                            }
                        }
                    }
                }
            }
        } else if chart {
            wb.warnings.push(format!("'{}' is a chart sheet; it is kept but not shown.", sheet.name));
        }
        sheet.xlsx = Some(Box::new(part));
        wb.sheets.push(sheet);
    }
    if wb.sheets.is_empty() {
        return Err(SheetError::Corrupt("The workbook has no sheets.".into()));
    }

    // Defined names.
    if let Some(names) = wb_root.elements().find(|e| e.local() == "definedNames") {
        for n in names.elements().filter(|e| e.local() == "definedName") {
            let Some(name) = n.attr(ns::NONE, "name") else { continue };
            let sheet = n.attr(ns::NONE, "localSheetId").and_then(|v| v.parse::<usize>().ok());
            let text = n.text();
            let mut d = DefinedName::new(name, sheet, text.trim_start_matches('='));
            d.hidden = n.attr(ns::NONE, "hidden").is_some_and(|v| v == "1" || v == "true");
            d.extra = n
                .attrs
                .iter()
                .filter(|a| !matches!(a.local(), "name" | "localSheetId" | "hidden"))
                .map(|a| (a.name.clone(), a.value.clone()))
                .collect();
            wb.names.push(d);
        }
    }

    // The sheet shown first.
    let active = wb_root
        .elements()
        .find(|e| e.local() == "bookViews")
        .and_then(|b| b.elements().next())
        .and_then(|v| v.attr(ns::NONE, "activeTab"))
        .and_then(|v| v.parse::<usize>().ok())
        .unwrap_or(0);
    wb.active = active.min(wb.sheets.len() - 1);
    pkg.original_active = wb.active;
    if wb.sheets[wb.active].state != SheetState::Visible {
        wb.active = wb.sheets.iter().position(|s| s.state == SheetState::Visible).unwrap_or(0);
    }
    wb.source = Source::Xlsx(Box::new(pkg));
    wb.calc_missing();
    for s in &mut wb.sheets {
        s.modified = false;
    }
    wb.recalculated = false;
    Ok(wb)
}

/// The shared strings, and the `<sst>` start tag.
fn read_shared_strings(text: &str, limits: crate::model::ReadLimits) -> Result<(Vec<SstEntry>, (String, Vec<(String, String)>)), doc_xml::XmlError> {
    if text.len() > limits.max_string_bytes {
        return Err(doc_xml::XmlError { offset: 0, message: "too large: shared string byte budget exceeded".into() });
    }
    let mut r = Reader::new(text);
    let mut out = Vec::new();
    let mut root = (String::from("sst"), Vec::new());
    loop {
        let before = r.offset();
        match r.next()? {
            Event::Eof => break,
            Event::Start(tag) if tag.local() == "sst" => {
                root = (
                    tag.name.to_owned(),
                    tag.attrs().filter_map(Result::ok).map(|(k, v)| (k.to_owned(), v.into_owned())).collect(),
                );
            }
            Event::Start(tag) if tag.local() == "si" => {
                if out.len() >= limits.max_shared_strings {
                    return Err(doc_xml::XmlError { offset: before, message: "too large: shared string count budget exceeded".into() });
                }
                let _ = before;
                let inner_start = r.offset();
                if tag.empty {
                    r.next()?; // its end
                    out.push(SstEntry::Plain(Arc::from("")));
                    continue;
                }
                let mut text = String::new();
                let mut rich = false;
                let depth = r.depth();
                loop {
                    let ev_start = r.offset();
                    match r.next()? {
                        Event::Start(t) if t.local() == "t" => {
                            let s = r.read_text()?;
                            text.push_str(&s);
                        }
                        Event::Start(t) if t.local() == "r" => {
                            rich = true;
                            let _ = t;
                        }
                        Event::Start(t) if matches!(t.local(), "rPh" | "phoneticPr" | "rPr") => {
                            if t.local() == "rPh" || t.local() == "phoneticPr" {
                                rich = true;
                            }
                            r.skip_element()?;
                        }
                        Event::End(_) if r.depth() < depth => {
                            let inner = &r.source()[inner_start..ev_start];
                            let decoded = doc_xml::ooxml::decode_xstring(&text);
                            out.push(if rich {
                                SstEntry::Rich {
                                    xml: Arc::from(inner),
                                    text: Arc::from(decoded),
                                }
                            } else {
                                SstEntry::Plain(Arc::from(decoded))
                            });
                            break;
                        }
                        Event::Eof => return Err(doc_xml::XmlError { offset: r.offset(), message: "unterminated <si>".into() }),
                        _ => {}
                    }
                }
            }
            _ => {}
        }
    }
    Ok((out, root))
}

/// A cell reference like `B7`; None for a malformed one.
fn parse_ref(r: &str) -> Option<CellRef> {
    CellRef::parse_a1(r).map(|(c, _, _)| c)
}

#[allow(clippy::too_many_arguments)]
fn read_worksheet(
    text: &str,
    path: &str,
    sheet: &mut Sheet,
    part: &mut SheetPart,
    pkg: &Package,
    wb: &mut Workbook,
    sheet_index: usize,
    cell_total: &mut usize,
) -> Result<(), SheetError> {
    // Find <sheetData> among the root's children.
    let mut r = Reader::new(text);
    let mut span: Option<(usize, usize, String)> = None;
    loop {
        let before = r.offset();
        match r.next().map_err(xml_err(path))? {
            Event::Eof => break,
            Event::Start(tag) if tag.local() == "sheetData" && (r.depth() == 2 || (tag.empty && r.depth() == 1)) => {
                let name = tag.name.to_owned();
                if !tag.empty {
                    r.skip_element().map_err(xml_err(path))?;
                } else {
                    r.next().map_err(xml_err(path))?;
                }
                span = Some((before, r.offset(), name));
                break;
            }
            _ => {}
        }
    }
    let src = r.source();
    let (doc_text, data) = match &span {
        Some((a, b, name)) => (format!("{}<{}/>{}", &src[..*a], name, &src[*b..]), &src[*a..*b]),
        None => (src.to_owned(), ""),
    };
    let doc = Document::parse_str(&doc_text).map_err(xml_err(path))?;

    // Layout from the tree.
    let root = &doc.root;
    if let Some(fmt) = root.elements().find(|e| e.local() == "sheetFormatPr") {
        if let Some(h) = fmt.attr(ns::NONE, "defaultRowHeight").and_then(|v| v.parse().ok()) {
            sheet.default_row_height = h;
        }
        match fmt.attr(ns::NONE, "defaultColWidth").and_then(|v| v.parse::<f64>().ok()) {
            Some(w) => sheet.default_col_width = w,
            None => {
                if let Some(b) = fmt.attr(ns::NONE, "baseColWidth").and_then(|v| v.parse::<f64>().ok()) {
                    sheet.default_col_width = b + 0.43;
                }
            }
        }
    }
    if let Some(view) = root
        .elements()
        .find(|e| e.local() == "sheetViews")
        .and_then(|v| v.elements().find(|e| e.local() == "sheetView"))
    {
        if view.attr(ns::NONE, "showGridLines").is_some_and(|v| v == "0" || v == "false") {
            sheet.show_grid = false;
        }
        if let Some(pane) = view.elements().find(|e| e.local() == "pane") {
            if matches!(pane.attr(ns::NONE, "state"), Some("frozen") | Some("frozenSplit")) {
                let x = pane.attr(ns::NONE, "xSplit").and_then(|v| v.parse::<f64>().ok()).unwrap_or(0.0) as u32;
                let y = pane.attr(ns::NONE, "ySplit").and_then(|v| v.parse::<f64>().ok()).unwrap_or(0.0) as u32;
                if x > 0 || y > 0 {
                    sheet.freeze = Some(CellRef::new(y.min(MAX_ROWS - 1), x.min(MAX_COLS - 1)));
                }
            }
        }
    }
    part.original_freeze = sheet.freeze;
    if let Some(cols) = root.elements().find(|e| e.local() == "cols") {
        let mut spans: std::collections::BTreeMap<u32, (u32, ColInfo)> = std::collections::BTreeMap::new();
        for c in cols.elements().filter(|e| e.local() == "col") {
            let min = c.attr(ns::NONE, "min").and_then(|v| v.parse::<u32>().ok()).unwrap_or(1).max(1);
            let max = c.attr(ns::NONE, "max").and_then(|v| v.parse::<u32>().ok()).unwrap_or(min).min(MAX_COLS);
            let info = ColInfo {
                width: c.attr(ns::NONE, "width").and_then(|v| v.parse().ok()),
                hidden: c.attr(ns::NONE, "hidden").is_some_and(|v| v == "1" || v == "true"),
                style: c.attr(ns::NONE, "style").and_then(|v| v.parse().ok()),
                custom_width: c.attr(ns::NONE, "customWidth").is_some_and(|v| v == "1" || v == "true"),
                extra: c
                    .attrs
                    .iter()
                    .filter(|a| !matches!(a.local(), "min" | "max" | "width" | "hidden" | "style" | "customWidth"))
                    .map(|a| (a.name.clone(), a.value.clone()))
                    .collect(),
            };
            if min > max {
                continue;
            }
            let first = spans.range(..=min).next_back().filter(|(_, (end, _))| *end >= min).map_or(min, |(&start, _)| start);
            let keys: Vec<u32> = spans.range(first..=max).map(|(&start, _)| start).collect();
            for start in keys {
                let (end, previous) = spans.remove(&start).expect("known span");
                if start < min {
                    spans.insert(start, (min - 1, previous.clone()));
                }
                if end > max {
                    spans.insert(max + 1, (end, previous));
                }
            }
            spans.insert(min, (max, info));
        }
        let mut bytes = 0usize;
        for (min, (max, info)) in spans {
            let size = std::mem::size_of::<ColInfo>() + info.extra.iter().map(|(k, v)| k.len() + v.len() + 48).sum::<usize>();
            bytes = bytes.saturating_add(size.saturating_mul((max - min + 1) as usize));
            if bytes > wb.limits.max_string_bytes {
                return Err(SheetError::TooLarge("The workbook has too much column metadata.".into()));
            }
            for col in min..=max {
                sheet.cols.insert(col - 1, info.clone());
            }
        }
    }
    if let Some(m) = root.elements().find(|e| e.local() == "mergeCells") {
        for mc in m.elements().filter(|e| e.local() == "mergeCell") {
            if let Some(range) = mc.attr(ns::NONE, "ref").and_then(Range::parse) {
                if range.cell_count() > 1 {
                    sheet.merges.push(range);
                }
            }
        }
    }
    part.doc = Some(doc);

    // The cells.
    if !data.is_empty() {
        read_sheet_data(data, path, sheet, pkg, wb, sheet_index, cell_total)?;
    }
    Ok(())
}

struct SharedFormula {
    anchor: CellRef,
    expr: Option<crate::formula::Expr>,
}

#[allow(clippy::too_many_arguments)]
fn read_sheet_data(
    data: &str,
    path: &str,
    sheet: &mut Sheet,
    pkg: &Package,
    wb: &mut Workbook,
    sheet_index: usize,
    cell_total: &mut usize,
) -> Result<(), SheetError> {
    let date1904 = wb.date1904;
    let mut r = Reader::new(data);
    let mut row_index: i64 = -1;
    let mut col_index: i64 = -1;
    let mut shared: HashMap<String, SharedFormula> = HashMap::new();
    let mut followers = Vec::new();
    let mut arrays: Vec<(CellRef, Range)> = Vec::new();
    loop {
        match r.next().map_err(xml_err(path))? {
            Event::Eof => break,
            Event::Start(tag) if tag.local() == "row" => {
                row_index = tag
                    .attr("r")
                    .and_then(|v| v.parse::<i64>().ok())
                    .map(|v| v - 1)
                    .unwrap_or(row_index + 1);
                col_index = -1;
                if row_index < 0 || row_index >= MAX_ROWS as i64 {
                    return Err(SheetError::Corrupt(format!("{path}: row {} is outside the sheet", row_index + 1)));
                }
                let mut info = RowInfo::default();
                let mut any = false;
                for attr in tag.attrs() {
                    let (k, v) = attr.map_err(xml_err(path))?;
                    let local = k.rsplit(':').next().unwrap_or(k);
                    match (k, local) {
                        (_, "r") | (_, "spans") => {}
                        ("ht", _) => {
                            info.height = v.parse().ok();
                            any = true;
                        }
                        ("customHeight", _) => {
                            info.custom_height = v == "1" || v == "true";
                            any = true;
                        }
                        ("hidden", _) => {
                            info.hidden = v == "1" || v == "true";
                            any = true;
                        }
                        ("s", _) => {
                            info.style = v.parse().ok();
                            any = true;
                        }
                        ("customFormat", _) => {}
                        _ => {
                            info.extra.push((k.to_owned(), v.into_owned()));
                            any = true;
                        }
                    }
                }
                if tag.attr("customFormat").is_none_or(|v| v != "1" && v != "true") {
                    info.style = None;
                }
                if any && info != RowInfo::default() {
                    sheet.rows.insert(row_index as u32, info);
                }
            }
            Event::Start(tag) if tag.local() == "c" => {
                if row_index < 0 {
                    row_index = 0;
                }
                let at = match tag.attr("r").and_then(|v| parse_ref(&v)) {
                    Some(c) => {
                        // A cell can name its own row; trust it.
                        row_index = c.row as i64;
                        c
                    }
                    None => CellRef::new(row_index as u32, (col_index + 1).clamp(0, MAX_COLS as i64 - 1) as u32),
                };
                col_index = at.col as i64;
                let style: u32 = tag.attr("s").and_then(|v| v.parse().ok()).unwrap_or(0);
                let t = tag.attr("t").map(|v| v.into_owned()).unwrap_or_else(|| "n".into());
                let attrs: Vec<(String, String)> = tag.attrs().filter_map(Result::ok)
                    .filter(|(k, _)| !matches!(*k, "r" | "s" | "t"))
                    .map(|(k, v)| (k.to_owned(), v.into_owned())).collect();
                let mut children = String::new();
                let mut v_text: Option<String> = None;
                let mut inline: Option<String> = None;
                let mut f: Option<(String, Vec<(String, String)>, Option<String>)> = None;
                if !tag.empty {
                    let depth = r.depth();
                    loop {
                        let start = r.offset();
                        match r.next().map_err(xml_err(path))? {
                            Event::Start(c) if c.local() == "v" => {
                                v_text = Some(if c.empty { String::new() } else { r.read_text().map_err(xml_err(path))? });
                            }
                            Event::Start(c) if c.local() == "f" => {
                                let attrs: Vec<(String, String)> =
                                    c.attrs().filter_map(Result::ok).map(|(k, v)| (k.to_owned(), v.into_owned())).collect();
                                let empty = c.empty;
                                let body = if empty { String::new() } else { r.read_text().map_err(xml_err(path))? };
                                let raw = Some(r.source()[start..r.offset()].to_owned());
                                f = Some((body, attrs, raw));
                            }
                            Event::Start(c) if c.local() == "is" => {
                                inline = Some(if c.empty { String::new() } else { read_inline(&mut r).map_err(xml_err(path))? });
                            }
                            Event::Start(c) => {
                                if !c.empty {
                                    r.skip_element().map_err(xml_err(path))?;
                                } else {
                                    r.next().map_err(xml_err(path))?;
                                }
                                children.push_str(&r.source()[start..r.offset()]);
                            }
                            Event::End(_) if r.depth() < depth => break,
                            Event::Eof => return Err(SheetError::Corrupt(format!("{path}: a cell is never closed"))),
                            _ => {}
                        }
                    }
                } else {
                    r.next().map_err(xml_err(path))?;
                }
                let mut cell = Cell {
                    style: if (style as usize) < wb.styles.len() { style } else { 0 },
                    ..Cell::default()
                };
                cell.value = match t.as_str() {
                    "s" => match v_text.as_deref().and_then(|v| v.trim().parse::<usize>().ok()).and_then(|i| pkg.sst.get(i).map(|e| (i, e))) {
                        Some((i, SstEntry::Plain(text))) => {
                            let _ = i;
                            Value::Text(text.clone())
                        }
                        Some((i, SstEntry::Rich { xml, text })) => {
                            cell.extra = Some(Box::new(CellExtra {
                                rich: Some((xml.clone(), i as u32)),
                                ..CellExtra::default()
                            }));
                            Value::Text(text.clone())
                        }
                        None => Value::Empty,
                    },
                    "inlineStr" => Value::text(doc_xml::ooxml::decode_xstring(&inline.unwrap_or_default())),
                    "str" => Value::text(doc_xml::ooxml::decode_xstring(v_text.as_deref().unwrap_or(""))),
                    "b" => match v_text.as_deref().map(str::trim) {
                        Some("1") | Some("true") => Value::Bool(true),
                        Some(_) => Value::Bool(false),
                        None => Value::Empty,
                    },
                    "e" => match v_text.as_deref() {
                        Some(e) => Value::Error(CellError::parse(e.trim()).unwrap_or(CellError::Value)),
                        None => Value::Empty,
                    },
                    "d" => match v_text.as_deref().and_then(|v| crate::datetime::iso_to_serial(v.trim(), date1904)) {
                        Some(n) => Value::Number(n),
                        None => Value::Empty,
                    },
                    _ => match v_text.as_deref().map(str::trim).filter(|v| !v.is_empty()) {
                        Some(v) => match v.parse::<f64>() {
                            Ok(n) if n.is_finite() => Value::Number(n),
                            _ => Value::Error(CellError::Value),
                        },
                        None => Value::Empty,
                    },
                };
                if !attrs.is_empty() || !children.is_empty() {
                    let extra = cell.extra.get_or_insert_with(Default::default);
                    extra.xlsx_attrs = attrs;
                    extra.xlsx_children = (!children.is_empty()).then(|| Arc::from(children));
                }
                if let Some((body, attrs, raw)) = f {
                    let attr = |name: &str| attrs.iter().find(|(k, _)| k == name).map(|(_, v)| v.as_str());
                    let kind = attr("t").unwrap_or("normal");
                    let formula = match kind {
                        "shared" => {
                            let si = attr("si").unwrap_or("").to_owned();
                            if !body.trim().is_empty() {
                                let parsed = crate::formula::parse(&body).ok().filter(|e| e.functions().iter().all(|n| crate::formula::functions::is_known(n)));
                                shared.insert(si, SharedFormula { anchor: at, expr: parsed.clone() });
                                let mut formula = Formula::new(&body);
                                formula.expr = parsed.map(Arc::new);
                                Some(formula)
                            } else {
                                followers.push((at, si, v_text.is_none()));
                                Some(Formula::new(&body))
                            }
                        }
                        "array" => {
                            let mut formula = Formula::new(&body);
                            let range = attr("ref").and_then(Range::parse).unwrap_or(Range::cell(at));
                            formula.array = Some(range);
                            if range.cell_count() > 1 {
                                arrays.push((at, range));
                            }
                            Some(formula)
                        }
                        "dataTable" => Some(Formula {
                            text: body.clone(),
                            expr: None,
                            array: None,
                            raw_xml: None,
                            ods_text: None,
                        }),
                        _ => Some(Formula::new(&body)),
                    };
                    if let Some(mut formula) = formula {
                        if kind != "shared" || formula.expr.is_none() {
                            formula.raw_xml = raw.map(Arc::from);
                        }
                        if v_text.is_none() && formula.expr.is_some() {
                            wb.calc.needs_calc.push((sheet_index, at));
                        }
                        cell.formula = Some(Box::new(formula));
                    }
                }
                if !cell.is_blank() {
                    *cell_total += 1;
                    if *cell_total > wb.limits.max_cells.min(MAX_CELLS) {
                        return Err(SheetError::TooLarge("The workbook has more cells than this app can hold.".into()));
                    }
                    sheet.cells.entry(at.row).or_default().insert(at.col, cell);
                }
            }
            _ => {}
        }
    }
    for (at, si, missing) in followers {
        match shared.get(&si) {
            Some(SharedFormula { anchor, expr: Some(e), .. }) => {
                let mut e = e.clone();
                offset_references(&mut e, at.row as i64 - anchor.row as i64, at.col as i64 - anchor.col as i64);
                if let Some(cell) = sheet.cell_mut(at) {
                    cell.formula = Some(Box::new(Formula::from_expr(e)));
                    if missing {
                        wb.calc.needs_calc.push((sheet_index, at));
                    }
                }
            }
            _ => {
                if !wb.warnings.iter().any(|w| w.contains("shared formula")) {
                    wb.warnings.push("A shared formula could not be read; its original formula and saved values are kept.".into());
                }
            }
        }
    }
    let mut members = 0u64;
    for (_, range) in &arrays {
        members = members.saturating_add(range.cell_count().saturating_sub(sheet.cells_in(*range).count() as u64));
        if members.saturating_add(*cell_total as u64) > wb.limits.max_cells.min(MAX_CELLS) as u64 {
            return Err(SheetError::TooLarge("The workbook has more array cells than this app can hold.".into()));
        }
    }
    // Mark the cells an array formula fills. Each one marked is a cell held
    // in memory, so it counts toward the workbook's cells: a 1.8 KB sheet of
    // array formulas made 20 million of them.
    for (anchor, range) in arrays {
        for at in range.iter() {
            if at == anchor {
                continue;
            }
            let row = sheet.cells.entry(at.row).or_default();
            if !row.contains_key(&at.col) {
                *cell_total += 1;
                if *cell_total > wb.limits.max_cells.min(MAX_CELLS) {
                    return Err(SheetError::TooLarge("The workbook has more cells than this app can hold.".into()));
                }
            }
            let cell = row.entry(at.col).or_default();
            let extra = cell.extra.get_or_insert_with(Default::default);
            extra.array_member = Some(anchor);
        }
    }
    sheet.invalidate_used();
    Ok(())
}

/// The text of an inline string (`<is>`), without phonetic runs.
fn read_inline(r: &mut Reader) -> Result<String, doc_xml::XmlError> {
    let depth = r.depth();
    let mut out = String::new();
    loop {
        match r.next()? {
            Event::Start(t) if t.local() == "t" => {
                if !t.empty {
                    out.push_str(&r.read_text()?);
                } else {
                    r.next()?;
                }
            }
            Event::Start(t) if matches!(t.local(), "rPh" | "phoneticPr" | "rPr") => {
                if !t.empty {
                    r.skip_element()?;
                } else {
                    r.next()?;
                }
            }
            Event::End(_) if r.depth() < depth => return Ok(out),
            Event::Eof => return Ok(out),
            _ => {}
        }
    }
}

/// A package for a workbook that has never been an XLSX file: the parts of
/// an empty workbook, ready for the writer to add sheets.
pub(super) fn fresh_package(options: &FormatOptions) -> Result<Package, SheetError> {
    let opc = doc_opc::Package::open(super::template::bytes()).map_err(corrupt)?;
    let workbook_part = "xl/workbook.xml".to_owned();
    let workbook = opc.read_xml(&workbook_part).map_err(corrupt)?;
    let workbook_rels = opc.rels(&workbook_part).map_err(corrupt)?;
    let styles = Some(opc.read_xml("xl/styles.xml").map_err(corrupt)?);
    let mut pkg = Package {
        opc,
        workbook_part,
        workbook,
        workbook_rels,
        styles_part: Some("xl/styles.xml".into()),
        styles,
        xf_count: 0,
        fonts: Vec::new(),
        fills: Vec::new(),
        borders: Vec::new(),
        num_fmts: Vec::new(),
        sst_part: None,
        sst: Vec::new(),
        sst_root: None,
        main_content_type: super::ct::WORKBOOK.into(),
        original_active: 0,
    };
    read_styles(&mut pkg, &Theme::default(), options);
    Ok(pkg)
}
