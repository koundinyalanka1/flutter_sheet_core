//! Reading an ODS package into the model.

use std::collections::HashMap;
use std::sync::Arc;

use doc_xml::{Document, Element, Event, Reader};

use super::formula::of_to_excel;
use super::styles::{apply_props, attr, chars_from_pt, data_style_code, length_pt};
use super::{OdsCellExtra, Package, TablePart};
use crate::error::SheetError;
use crate::formula::adjust::offset_references;
use crate::model::{Cell, CellExtra, ColInfo, Format, Formula, RowInfo, Sheet, SheetState, Source, Workbook};
use crate::numfmt::DateOrder;
use crate::refs::{CellRef, Range, MAX_COLS, MAX_ROWS};
use crate::style::{CellStyle, StyleTable, Theme};
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

/// Parse a fragment of a document with the root's namespace declarations.
fn fragment(slice: &str, ns_decls: &str) -> Option<Element> {
    let text = format!("<w {ns_decls}>{slice}</w>");
    let doc = Document::parse_str(&text).ok()?;
    let first = doc.root.elements().next().cloned();
    first
}

/// Cell styles by name, resolved through their parents.
pub(super) struct Styles {
    fonts: HashMap<String, String>,
    named: HashMap<String, Element>,
    auto: HashMap<String, Element>,
    data: HashMap<String, Element>,
    default: CellStyle,
    resolved: HashMap<String, u32>,
}

impl Styles {
    fn index_styles(container: &Element, into: &mut HashMap<String, Element>, data: &mut HashMap<String, Element>) {
        for e in container.elements() {
            let Some(name) = attr(e, "name") else { continue };
            if e.local().ends_with("-style") && e.local() != "default-style" {
                data.insert(name.to_owned(), e.clone());
            } else if e.local() == "style" {
                into.insert(name.to_owned(), e.clone());
            }
        }
    }

    fn cell_style(&mut self, name: &str, table: &mut StyleTable) -> u32 {
        if let Some(&id) = self.resolved.get(name) {
            return id;
        }
        let style = self.resolve(name, 0);
        let id = table.intern(style);
        self.resolved.insert(name.to_owned(), id);
        id
    }

    fn resolve(&self, name: &str, depth: usize) -> CellStyle {
        let e = self.auto.get(name).or_else(|| self.named.get(name));
        let Some(e) = e else { return self.default.clone() };
        let mut base = match attr(e, "parent-style-name") {
            Some(p) if depth < 8 && p != name => self.resolve(p, depth + 1),
            _ => self.default.clone(),
        };
        apply_props(&mut base, e, &self.fonts);
        if let Some(ds) = attr(e, "data-style-name").and_then(|d| self.data.get(d)) {
            base.num_fmt = data_style_code(ds, &self.data, 0);
        }
        base
    }

    fn props(&self, name: &str) -> Option<&Element> {
        self.auto.get(name).or_else(|| self.named.get(name))
    }

    fn col_width(&self, name: &str) -> Option<f64> {
        let e = self.props(name)?;
        let p = e.elements().find(|c| c.local() == "table-column-properties")?;
        attr(p, "column-width").and_then(length_pt).map(chars_from_pt)
    }

    fn row_height(&self, name: &str) -> Option<(f64, bool)> {
        let e = self.props(name)?;
        let p = e.elements().find(|c| c.local() == "table-row-properties")?;
        let h = attr(p, "row-height").and_then(length_pt)?;
        let optimal = attr(p, "use-optimal-row-height") == Some("true");
        Some(((h * 100.0).round() / 100.0, !optimal))
    }

    fn table_hidden(&self, name: &str) -> bool {
        self.props(name)
            .and_then(|e| e.elements().find(|c| c.local() == "table-properties"))
            .is_some_and(|p| attr(p, "display") == Some("false"))
    }
}

/// Read an ODS file.
pub fn read(bytes: Vec<u8>, date_order: DateOrder) -> Result<Workbook, SheetError> {
    read_with_limits(bytes, date_order, crate::model::ReadLimits::default())
}

pub fn read_with_limits(bytes: Vec<u8>, date_order: DateOrder, limits: crate::model::ReadLimits) -> Result<Workbook, SheetError> {
    let opc = doc_opc::Package::open_with_limits(bytes, limits.package()).map_err(|e| match e {
        doc_opc::OpcError::Zip(doc_zip::ZipError::NotZip) => SheetError::Corrupt("This is not an OpenDocument spreadsheet.".into()),
        other => corrupt(other),
    })?;
    if opc.exists("META-INF/manifest.xml") {
        let manifest = opc.read_text("META-INF/manifest.xml").map_err(corrupt)?;
        if manifest.contains("encryption-data") {
            return Err(SheetError::Encrypted);
        }
    }
    let content = opc.read_text("content.xml").map_err(corrupt)?;

    let mut styles = Styles {
        fonts: HashMap::new(),
        named: HashMap::new(),
        auto: HashMap::new(),
        data: HashMap::new(),
        default: CellStyle::base(),
        resolved: HashMap::new(),
    };
    // styles.xml: the default and named styles, and their data styles.
    if opc.exists("styles.xml") {
        let doc = opc.read_xml("styles.xml").map_err(corrupt)?;
        for c in doc.root.elements() {
            match c.local() {
                "font-face-decls" => {
                    for f in c.elements() {
                        if let (Some(n), Some(fam)) = (attr(f, "name"), attr(f, "font-family")) {
                            styles.fonts.insert(n.to_owned(), fam.trim_matches('\'').trim_matches('"').to_owned());
                        }
                    }
                }
                "styles" => {
                    let (mut named, mut data) = (HashMap::new(), HashMap::new());
                    Styles::index_styles(c, &mut named, &mut data);
                    styles.named.extend(named);
                    styles.data.extend(data);
                    for d in c.elements().filter(|e| e.local() == "default-style" && attr(e, "family") == Some("table-cell")) {
                        let mut base = CellStyle::base();
                        apply_props(&mut base, d, &styles.fonts);
                        styles.default = base;
                    }
                }
                "automatic-styles" => {
                    let (mut named, mut data) = (HashMap::new(), HashMap::new());
                    Styles::index_styles(c, &mut named, &mut data);
                    styles.data.extend(data);
                }
                _ => {}
            }
        }
        if let Some(d) = styles.named.get("Default").cloned() {
            let mut base = styles.default.clone();
            apply_props(&mut base, &d, &styles.fonts);
            styles.default = base;
        }
    }

    let mut wb = Workbook {
        limits,
        date_order,
        format: Some(Format::Ods),
        styles: StyleTable::new(vec![styles.default.clone()], Theme::default()),
        ..Workbook::default()
    };
    if !opc.duplicate_parts().is_empty() {
        wb.warnings.push("This spreadsheet has duplicate parts. The first copy of each part is used and kept when saving.".into());
    }
    let mut pkg = Package {
        opc,
        content: String::new(),
        auto_styles: None,
        font_decls: None,
        tables_region: (0, 0),
        tables: Vec::new(),
        fonts: HashMap::new(),
    };

    let mut r = Reader::new(&content);
    let mut ns_decls = String::new();
    let mut spreadsheet_open: Option<usize> = None;
    let mut cell_total = 0usize;
    loop {
        let before = r.offset();
        let event = r.next().map_err(corrupt)?;
        match event {
            Event::Eof => break,
            Event::Start(tag) if tag.local() == "document-content" => {
                for a in tag.attrs().filter_map(Result::ok) {
                    if a.0.starts_with("xmlns") {
                        let mut v = String::new();
                        doc_xml::escape_attr(&a.1, &mut v);
                        ns_decls.push_str(&format!(" {}=\"{}\"", a.0, v));
                    }
                }
            }
            Event::Start(tag) if tag.local() == "font-face-decls" => {
                r.skip_element().map_err(corrupt)?;
                let slice = &content[before..r.offset()];
                pkg.font_decls = Some((before, r.offset()));
                if let Some(e) = fragment(slice, &ns_decls) {
                    for f in e.elements() {
                        if let (Some(n), Some(fam)) = (attr(f, "name"), attr(f, "font-family")) {
                            let fam = fam.trim_matches('\'').trim_matches('"').to_owned();
                            styles.fonts.insert(n.to_owned(), fam.clone());
                            pkg.fonts.insert(n.to_owned(), fam);
                        }
                    }
                }
            }
            Event::Start(tag) if tag.local() == "automatic-styles" => {
                r.skip_element().map_err(corrupt)?;
                let slice = &content[before..r.offset()];
                pkg.auto_styles = Some((before, r.offset()));
                if let Some(e) = fragment(slice, &ns_decls) {
                    let (mut named, mut data) = (HashMap::new(), HashMap::new());
                    Styles::index_styles(&e, &mut named, &mut data);
                    styles.auto = named;
                    styles.data.extend(data);
                }
            }
            Event::Start(tag) if tag.local() == "spreadsheet" => {
                spreadsheet_open = Some(r.offset());
                if tag.empty {
                    r.next().map_err(corrupt)?;
                }
            }
            Event::Start(tag) if tag.local() == "table" && spreadsheet_open.is_some() => {
                let attrs: Vec<(String, String)> = tag.attrs().filter_map(Result::ok).map(|(k, v)| (k.to_owned(), v.into_owned())).collect();
                let index = wb.sheets.len();
                let (sheet, part) = read_table(&mut r, &content, &attrs, tag.empty, &mut styles, &mut wb, index, &mut cell_total)?;
                pkg.tables.push((before, r.offset()));
                let mut sheet = sheet;
                let mut part = part;
                part.original = Some(index);
                part.name = sheet.name.clone();
                part.hidden = sheet.state != SheetState::Visible;
                sheet.ods = Some(Box::new(part));
                wb.sheets.push(sheet);
            }
            Event::End(name) if name.ends_with("spreadsheet") => {
                if pkg.tables.is_empty() {
                    let at = spreadsheet_open.unwrap_or(before);
                    pkg.tables_region = (at, at);
                }
            }
            _ => {}
        }
    }
    if let (Some(first), Some(last)) = (pkg.tables.first(), pkg.tables.last()) {
        pkg.tables_region = (first.0, last.1);
    }
    if wb.sheets.is_empty() {
        return Err(SheetError::Corrupt("The spreadsheet has no sheets.".into()));
    }

    // Frozen panes and the active sheet live in settings.xml.
    if pkg.opc.exists("settings.xml") {
        let settings = pkg.opc.read_xml("settings.xml").map_err(corrupt)?;
        read_settings(&settings, &mut wb);
    }
    if wb.sheets[wb.active].state != SheetState::Visible {
        wb.active = wb.sheets.iter().position(|s| s.state == SheetState::Visible).unwrap_or(0);
    }
    wb.styles.original = wb.styles.len();
    pkg.content = content;
    wb.source = Source::Ods(Box::new(pkg));
    wb.calc_missing();
    for s in &mut wb.sheets {
        s.modified = false;
    }
    wb.recalculated = false;
    Ok(wb)
}

fn read_settings(doc: &Document, wb: &mut Workbook) {
    let item = |e: &Element, name: &str| -> Option<String> {
        e.elements()
            .find(|c| c.local() == "config-item" && attr(c, "name") == Some(name))
            .map(|c| c.text())
    };
    for set in doc.root.descendants().filter(|e| e.local() == "config-item-map-entry") {
        if let Some(active) = item(set, "ActiveTable") {
            if let Some(i) = wb.sheet_index(&active) {
                wb.active = i;
            }
        }
        let Some(tables) = set
            .elements()
            .find(|c| c.local() == "config-item-map-named" && attr(c, "name") == Some("Tables"))
        else {
            continue;
        };
        for t in tables.elements() {
            let Some(name) = attr(t, "name") else { continue };
            let Some(i) = wb.sheet_index(name) else { continue };
            let num = |n: &str| item(t, n).and_then(|v| v.parse::<u32>().ok()).unwrap_or(0);
            let (hmode, vmode) = (num("HorizontalSplitMode"), num("VerticalSplitMode"));
            let cols = if hmode == 2 { num("HorizontalSplitPosition") } else { 0 };
            let rows = if vmode == 2 { num("VerticalSplitPosition") } else { 0 };
            if cols > 0 || rows > 0 {
                wb.sheets[i].freeze = Some(CellRef::new(rows.min(MAX_ROWS - 1), cols.min(MAX_COLS - 1)));
            }
            if item(t, "ShowGrid").as_deref() == Some("false") {
                wb.sheets[i].show_grid = false;
            }
        }
    }
}

/// A cell as read, before it is placed (it may repeat).
struct RawCell {
    value: Value,
    formula: Option<String>,
    style: Option<String>,
    span: (u32, u32),
    covered: bool,
    extra: OdsCellExtra,
    has_text: bool,
}

#[allow(clippy::too_many_arguments)]
fn read_table(
    r: &mut Reader,
    src: &str,
    attrs: &[(String, String)],
    empty: bool,
    styles: &mut Styles,
    wb: &mut Workbook,
    sheet_index: usize,
    cell_total: &mut usize,
) -> Result<(Sheet, TablePart), SheetError> {
    let get = |name: &str| attrs.iter().find(|(k, _)| k.rsplit(':').next() == Some(name)).map(|(_, v)| v.as_str());
    let mut sheet = Sheet::new(get("name").unwrap_or("Sheet"));
    if let Some(s) = get("style-name") {
        if styles.table_hidden(s) {
            sheet.state = SheetState::Hidden;
        }
    }
    let mut part = TablePart {
        attrs: attrs
            .iter()
            .filter(|(k, _)| !matches!(k.rsplit(':').next(), Some("name") | Some("style-name")))
            .cloned()
            .collect(),
        ..TablePart::default()
    };
    if empty {
        r.next().map_err(corrupt)?;
        return Ok((sheet, part));
    }
    let depth = r.depth();
    let mut col: u32 = 0;
    let mut row: u32 = 0;
    let mut seen_body = false;
    // Default cell style of each column: (first, last, style).
    let mut col_styles: Vec<(u32, u32, u32)> = Vec::new();
    let mut arrays: Vec<(CellRef, Range)> = Vec::new();
    loop {
        let before = r.offset();
        match r.next().map_err(corrupt)? {
            Event::End(_) if r.depth() < depth => break,
            Event::Eof => return Err(SheetError::Corrupt("a table is never closed".into())),
            Event::Start(tag) => {
                let local = tag.local();
                match local {
                    "table-column-group" | "table-header-columns" | "table-columns" | "table-row-group" | "table-header-rows" | "table-rows" => {
                        seen_body = true;
                        if tag.empty {
                            r.next().map_err(corrupt)?;
                        }
                    }
                    "table-column" => {
                        seen_body = true;
                        let n = tag.attr("table:number-columns-repeated").and_then(|v| v.parse::<u32>().ok()).unwrap_or(1).max(1);
                        let last = (col as u64 + n as u64 - 1).min(MAX_COLS as u64 - 1) as u32;
                        let width = tag.attr("table:style-name").and_then(|s| styles.col_width(&s));
                        let hidden = tag.attr("table:visibility").is_some_and(|v| v != "visible");
                        let default_style = tag
                            .attr("table:default-cell-style-name")
                            .filter(|s| s != "Default")
                            .map(|s| styles.cell_style(&s, &mut wb.styles));
                        if let Some(st) = default_style.filter(|s| *s != 0) {
                            col_styles.push((col, last, st));
                        }
                        let info = ColInfo {
                            width,
                            hidden,
                            style: None,
                            custom_width: width.is_some(),
                            extra: Vec::new(),
                        };
                        // Very long runs are the rest of the sheet: only
                        // keep them when they hide or style something.
                        if (width.is_some_and(|w| (w - sheet.default_col_width).abs() > 0.01) || hidden) && n <= 1024 {
                            for c in col..=last {
                                sheet.cols.insert(c, info.clone());
                            }
                        }
                        col = last.saturating_add(1);
                        if !tag.empty {
                            r.skip_element().map_err(corrupt)?;
                        } else {
                            r.next().map_err(corrupt)?;
                        }
                    }
                    "table-row" => {
                        seen_body = true;
                        let n = tag.attr("table:number-rows-repeated").and_then(|v| v.parse::<u32>().ok()).unwrap_or(1).max(1);
                        let height = tag.attr("table:style-name").and_then(|s| styles.row_height(&s));
                        let hidden = tag.attr("table:visibility").is_some_and(|v| v != "visible");
                        let row_default = tag
                            .attr("table:default-cell-style-name")
                            .filter(|s| s != "Default")
                            .map(|s| s.into_owned());
                        let cells = if tag.empty {
                            r.next().map_err(corrupt)?;
                            Vec::new()
                        } else {
                            read_row(r, src)?
                        };
                        let has_content = cells.iter().any(|(c, _)| !c.value.is_empty() || c.formula.is_some() || c.extra != OdsCellExtra::default() || c.span != (1, 1) || c.style.as_deref().is_some_and(|s| s != "Default"));
                        let repeat = if has_content || hidden || height.is_some_and(|(h, custom)| custom && (h - sheet.default_row_height).abs() > 0.05) { n.min(MAX_ROWS.saturating_sub(row)) } else { 1 };
                        let mut column = 0u64;
                        let mut row_cells = 0u64;
                        for (cell, count) in &cells {
                            let count = (*count as u64).min((MAX_COLS as u64).saturating_sub(column));
                            if !cell.value.is_empty() || cell.formula.is_some() || cell.extra != OdsCellExtra::default() || cell.span != (1, 1) || cell.style.as_deref().is_some_and(|s| s != "Default") {
                                row_cells = row_cells.saturating_add(count);
                            }
                            column = column.saturating_add(count);
                        }
                        if row_cells.saturating_mul(repeat as u64).saturating_add(*cell_total as u64) > wb.limits.max_cells.min(MAX_CELLS) as u64 {
                            return Err(SheetError::TooLarge("The spreadsheet repeats more cells than this app can hold.".into()));
                        }
                        for k in 0..repeat {
                            let rr = row as u64 + k as u64;
                            if rr >= MAX_ROWS as u64 {
                                break;
                            }
                            let rr = rr as u32;
                            if let Some((h, custom)) = height {
                                if (custom && (h - sheet.default_row_height).abs() > 0.05) || hidden {
                                    sheet.rows.insert(rr, RowInfo { height: Some(h), custom_height: custom, hidden, ..RowInfo::default() });
                                }
                            } else if hidden {
                                sheet.rows.insert(rr, RowInfo { hidden, ..RowInfo::default() });
                            }
                            place_row(&cells, rr, &row_default, &col_styles, styles, wb, &mut sheet, sheet_index, k, &mut arrays, cell_total)?;
                        }
                        row = (row as u64 + n as u64).min(MAX_ROWS as u64) as u32;
                    }
                    _ => {
                        // Something we keep as written.
                        if !tag.empty {
                            r.skip_element().map_err(corrupt)?;
                        } else {
                            r.next().map_err(corrupt)?;
                        }
                        let raw = &src[before..r.offset()];
                        if seen_body {
                            part.tail.push_str(raw);
                        } else {
                            part.head.push_str(raw);
                        }
                    }
                }
            }
            _ => {}
        }
    }
    let mut members = 0u64;
    for (_, range) in &arrays {
        members = members.saturating_add(range.cell_count().saturating_sub(sheet.cells_in(*range).count() as u64));
        if members.saturating_add(*cell_total as u64) > wb.limits.max_cells.min(MAX_CELLS) as u64 {
            return Err(SheetError::TooLarge("The spreadsheet has more array cells than this app can hold.".into()));
        }
    }
    // Each cell an array formula marks is held in memory, so it counts toward
    // the spreadsheet's cells.
    for (anchor, range) in arrays {
        for at in range.iter() {
            if at == anchor {
                continue;
            }
            let row = sheet.cells.entry(at.row).or_default();
            if !row.contains_key(&at.col) {
                *cell_total += 1;
                if *cell_total > wb.limits.max_cells.min(MAX_CELLS) {
                    return Err(SheetError::TooLarge("The spreadsheet has more cells than this app can hold.".into()));
                }
            }
            let cell = row.entry(at.col).or_default();
            cell.extra.get_or_insert_with(Default::default).array_member = Some(anchor);
        }
    }
    let mut seen = std::collections::HashSet::with_capacity(sheet.merges.len());
    sheet.merges.retain(|m| seen.insert(*m));
    sheet.invalidate_used();
    Ok((sheet, part))
}

/// The cells of a row: each with its repeat count.
fn read_row(r: &mut Reader, src: &str) -> Result<Vec<(RawCell, u32)>, SheetError> {
    let depth = r.depth();
    let mut out = Vec::new();
    loop {
        match r.next().map_err(corrupt)? {
            Event::End(_) if r.depth() < depth => return Ok(out),
            Event::Eof => return Err(SheetError::Corrupt("a row is never closed".into())),
            Event::Start(tag) if tag.local() == "table-cell" || tag.local() == "covered-table-cell" => {
                let covered = tag.local() == "covered-table-cell";
                let mut repeat = 1u32;
                let mut cell = RawCell {
                    value: Value::Empty,
                    formula: None,
                    style: None,
                    span: (1, 1),
                    covered,
                    extra: OdsCellExtra::default(),
                    has_text: false,
                };
                let mut value_type = String::new();
                let mut calc_type = String::new();
                let mut vals: HashMap<&'static str, String> = HashMap::new();
                for a in tag.attrs() {
                    let (k, v) = a.map_err(corrupt)?;
                    let local = k.rsplit(':').next().unwrap_or(k);
                    match local {
                        "number-columns-repeated" => repeat = v.parse().unwrap_or(1),
                        "number-columns-spanned" => cell.span.1 = v.parse().unwrap_or(1),
                        "number-rows-spanned" => cell.span.0 = v.parse().unwrap_or(1),
                        "formula" => cell.formula = Some(v.into_owned()),
                        "style-name" => cell.style = Some(v.into_owned()),
                        "value-type" if k.starts_with("office") => value_type = v.into_owned(),
                        "value-type" => calc_type = v.into_owned(),
                        "value" => {
                            vals.insert("value", v.into_owned());
                        }
                        "date-value" => {
                            vals.insert("date", v.into_owned());
                        }
                        "time-value" => {
                            vals.insert("time", v.into_owned());
                        }
                        "boolean-value" => {
                            vals.insert("bool", v.into_owned());
                        }
                        "string-value" => {
                            vals.insert("string", v.into_owned());
                        }
                        "currency" => {}
                        _ => cell.extra.attrs.push((k.to_owned(), v.into_owned())),
                    }
                }
                let mut text = String::new();
                if !tag.empty {
                    let d = r.depth();
                    let mut paragraphs = 0;
                    loop {
                        let before = r.offset();
                        match r.next().map_err(corrupt)? {
                            Event::End(_) if r.depth() < d => break,
                            Event::Start(c) if c.local() == "p" => {
                                if paragraphs > 0 {
                                    text.push('\n');
                                }
                                paragraphs += 1;
                                cell.has_text = true;
                                if !c.empty {
                                    read_paragraph(r, &mut text)?;
                                } else {
                                    r.next().map_err(corrupt)?;
                                }
                            }
                            Event::Start(c) if c.local() == "annotation" => {
                                if !c.empty {
                                    r.skip_element().map_err(corrupt)?;
                                } else {
                                    r.next().map_err(corrupt)?;
                                }
                                cell.extra.annotation = Some(Arc::from(&src[before..r.offset()]));
                            }
                            Event::Start(c) => {
                                if !c.empty {
                                    r.skip_element().map_err(corrupt)?;
                                } else {
                                    r.next().map_err(corrupt)?;
                                }
                            }
                            Event::Eof => return Err(SheetError::Corrupt("a cell is never closed".into())),
                            _ => {}
                        }
                    }
                } else {
                    r.next().map_err(corrupt)?;
                }
                cell.value = match value_type.as_str() {
                    "float" | "percentage" | "currency" => vals
                        .get("value")
                        .and_then(|v| v.trim().parse::<f64>().ok())
                        .map(Value::Number)
                        .unwrap_or(Value::Empty),
                    "date" => vals
                        .get("date")
                        .and_then(|v| crate::datetime::iso_to_serial(v.trim(), false))
                        .map(Value::Number)
                        .unwrap_or(Value::Empty),
                    "time" => vals
                        .get("time")
                        .and_then(|v| crate::datetime::iso_duration_days(v))
                        .map(Value::Number)
                        .unwrap_or(Value::Empty),
                    "boolean" => Value::Bool(vals.get("bool").is_some_and(|v| v == "true" || v == "1")),
                    "string" => Value::text(vals.get("string").cloned().unwrap_or_else(|| text.clone())),
                    _ if calc_type == "error" => Value::Error(CellError::parse(text.trim()).unwrap_or(CellError::Value)),
                    _ => Value::Empty,
                };
                if calc_type == "error" {
                    cell.value = Value::Error(CellError::parse(text.trim()).unwrap_or(CellError::Value));
                }
                out.push((cell, repeat.max(1)));
            }
            Event::Start(tag) => {
                if !tag.empty {
                    r.skip_element().map_err(corrupt)?;
                } else {
                    r.next().map_err(corrupt)?;
                }
            }
            _ => {}
        }
    }
}

/// The text of a `text:p`, with spaces, tabs and line breaks.
fn read_paragraph(r: &mut Reader, out: &mut String) -> Result<(), SheetError> {
    let d = r.depth();
    loop {
        match r.next().map_err(corrupt)? {
            Event::End(_) if r.depth() < d => return Ok(()),
            Event::Text(t) => out.push_str(&t),
            Event::CData(t) => out.push_str(t),
            Event::Start(c) => match c.local() {
                "s" => {
                    let n = c.attr("text:c").and_then(|v| v.parse::<usize>().ok()).unwrap_or(1).min(10_000);
                    out.push_str(&" ".repeat(n));
                    if !c.empty {
                        r.skip_element().map_err(corrupt)?;
                    } else {
                        r.next().map_err(corrupt)?;
                    }
                }
                "tab" => {
                    out.push('\t');
                    if !c.empty {
                        r.skip_element().map_err(corrupt)?;
                    } else {
                        r.next().map_err(corrupt)?;
                    }
                }
                "line-break" => {
                    out.push('\n');
                    if !c.empty {
                        r.skip_element().map_err(corrupt)?;
                    } else {
                        r.next().map_err(corrupt)?;
                    }
                }
                "annotation" | "note" | "bookmark" | "bookmark-start" | "bookmark-end" | "soft-page-break" => {
                    if !c.empty {
                        r.skip_element().map_err(corrupt)?;
                    } else {
                        r.next().map_err(corrupt)?;
                    }
                }
                // Spans and links: their text is read as the loop goes on.
                _ => {
                    if c.empty {
                        r.next().map_err(corrupt)?;
                    }
                }
            },
            Event::Eof => return Err(SheetError::Corrupt("a paragraph is never closed".into())),
            _ => {}
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn place_row(
    cells: &[(RawCell, u32)],
    row: u32,
    row_default: &Option<String>,
    col_styles: &[(u32, u32, u32)],
    styles: &mut Styles,
    wb: &mut Workbook,
    sheet: &mut Sheet,
    sheet_index: usize,
    row_repeat: u32,
    arrays: &mut Vec<(CellRef, Range)>,
    cell_total: &mut usize,
) -> Result<(), SheetError> {
    let mut col: u32 = 0;
    for (raw, repeat) in cells {
        let content = !raw.value.is_empty() || raw.formula.is_some() || raw.extra != OdsCellExtra::default() || raw.span != (1, 1);
        let styled = raw.style.as_deref().is_some_and(|s| s != "Default");
        let n = if content || styled { (*repeat).min(MAX_COLS.saturating_sub(col)) } else { 0 };
        for k in 0..n {
            let c = col as u64 + k as u64;
            if c >= MAX_COLS as u64 {
                break;
            }
            let c = c as u32;
            if raw.covered && !content {
                continue;
            }
            let at = CellRef::new(row, c);
            let style_name = raw
                .style
                .clone()
                .or_else(|| row_default.clone())
                .or_else(|| None);
            let style = match style_name {
                Some(s) if s != "Default" => styles.cell_style(&s, &mut wb.styles),
                Some(_) => 0,
                None => col_styles.iter().find(|(a, b, _)| (*a..=*b).contains(&c)).map_or(0, |x| x.2),
            };
            let mut cell = Cell {
                value: raw.value.clone(),
                style,
                ..Cell::default()
            };
            if let Some(f) = &raw.formula {
                let text = of_to_excel(f);
                let mut formula = Formula::new(&text);
                formula.ods_text = Some(Arc::from(f.as_str()));
                // Repeated formula cells move their relative references.
                if (k > 0 || row_repeat > 0) && formula.expr.is_some() {
                    let mut e = (**formula.expr.as_ref().expect("checked")).clone();
                    offset_references(&mut e, row_repeat as i64, k as i64);
                    formula = Formula::from_expr(e);
                }
                if raw.extra.attrs.iter().any(|(k, _)| k.ends_with("number-matrix-columns-spanned")) {
                    let cols = raw.extra.attrs.iter().find(|(k, _)| k.ends_with("number-matrix-columns-spanned")).and_then(|(_, v)| v.parse::<u32>().ok()).unwrap_or(1);
                    let rows = raw.extra.attrs.iter().find(|(k, _)| k.ends_with("number-matrix-rows-spanned")).and_then(|(_, v)| v.parse::<u32>().ok()).unwrap_or(1);
                    let end = CellRef::new(row.saturating_add(rows.max(1) - 1).min(MAX_ROWS - 1), c.saturating_add(cols.max(1) - 1).min(MAX_COLS - 1));
                    let range = Range::new(at, end);
                    formula.array = Some(range);
                    if range.cell_count() > 1 {
                        arrays.push((at, range));
                    }
                }
                if raw.value.is_empty() && formula.expr.is_some() {
                    wb.calc.needs_calc.push((sheet_index, at));
                }
                cell.formula = Some(Box::new(formula));
            }
            let extra = OdsCellExtra {
                attrs: raw
                    .extra
                    .attrs
                    .iter()
                    .filter(|(k, _)| !k.ends_with("number-matrix-columns-spanned") && !k.ends_with("number-matrix-rows-spanned"))
                    .cloned()
                    .collect(),
                annotation: raw.extra.annotation.clone(),
            };
            if extra != OdsCellExtra::default() {
                cell.extra = Some(Box::new(CellExtra {
                    ods: Some(Box::new(extra)),
                    ..CellExtra::default()
                }));
            }
            if raw.span != (1, 1) && !raw.covered {
                let end = CellRef::new(row.saturating_add(raw.span.0.max(1) - 1).min(MAX_ROWS - 1), c.saturating_add(raw.span.1.max(1) - 1).min(MAX_COLS - 1));
                let m = Range::new(at, end);
                if m.cell_count() > 1 {
                    // Duplicates are dropped once the sheet is read.
                    if sheet.merges.len() >= wb.limits.max_cells.min(MAX_CELLS) {
                        return Err(SheetError::TooLarge("The spreadsheet has more merged cells than this app can hold.".into()));
                    }
                    sheet.merges.push(m);
                }
            }
            if !cell.is_blank() {
                *cell_total += 1;
                if *cell_total > wb.limits.max_cells.min(MAX_CELLS) {
                    return Err(SheetError::TooLarge("The spreadsheet has more cells than this app can hold.".into()));
                }
                sheet.cells.entry(row).or_default().insert(c, cell);
            }
        }
        col = (col as u64 + *repeat as u64).min(MAX_COLS as u64) as u32;
    }
    Ok(())
}
