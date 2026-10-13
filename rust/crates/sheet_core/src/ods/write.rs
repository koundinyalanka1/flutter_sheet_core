//! Writing a workbook as ODS, in place when it came from one.

use std::collections::BTreeMap;
use std::fmt::Write as _;

use doc_xml::{escape_attr, escape_text, Document, Element};

use super::styles::{cell_style_xml, font_face_xml, pt_from_chars, DataStyles};
use super::{template, Package};
use crate::error::SheetError;
use crate::model::{Cell, Sheet, SheetState, Source, Workbook};
use crate::numfmt::NumberFormat;
use crate::refs::CellRef;
use crate::value::Value;

fn corrupt(e: impl std::fmt::Display) -> SheetError {
    SheetError::Corrupt(e.to_string())
}

fn attr_text(v: &str) -> String {
    let mut s = String::new();
    escape_attr(v, &mut s);
    s
}

fn fresh_package() -> Result<Package, SheetError> {
    let opc = doc_opc::Package::open(template::bytes()).map_err(corrupt)?;
    let content = template::content();
    let span = |open: &str, close: &str| -> Option<(usize, usize)> {
        let a = content.find(open)?;
        let b = content[a..].find(close)? + a + close.len();
        Some((a, b))
    };
    let auto_styles = span("<office:automatic-styles>", "</office:automatic-styles>");
    let font_decls = span("<office:font-face-decls>", "</office:font-face-decls>");
    let at = content.find("<office:spreadsheet>").map(|i| i + "<office:spreadsheet>".len()).unwrap_or(0);
    Ok(Package {
        opc,
        content,
        auto_styles,
        font_decls,
        tables_region: (at, at),
        tables: Vec::new(),
        fonts: [("Calibri".to_owned(), "Calibri".to_owned())].into_iter().collect(),
    })
}

/// Write the workbook as ODS.
pub fn write(wb: &mut Workbook) -> Result<Vec<u8>, SheetError> {
    let own = matches!(wb.source, Source::Ods(_));
    let mut pkg = match std::mem::take(&mut wb.source) {
        Source::Ods(p) => *p,
        other => {
            wb.source = other;
            fresh_package()?
        }
    };
    let result = write_into(wb, &mut pkg);
    if own {
        wb.source = Source::Ods(Box::new(pkg));
    } else {
        for s in &mut wb.sheets {
            s.ods = None;
        }
    }
    result
}

/// Names for the styles this save adds, under a prefix the file does not
/// use yet.
struct Names {
    prefix: String,
    cells: BTreeMap<u32, String>,
    cols: Vec<(u64, String)>,
    rows: Vec<(u64, bool, String)>,
    data: DataStyles,
    xml: String,
    fonts: Vec<String>,
}

impl Names {
    fn cell(&mut self, wb: &Workbook, id: u32) -> String {
        if id == 0 {
            return "Default".into();
        }
        if let Some(n) = self.cells.get(&id) {
            return n.clone();
        }
        let style = wb.styles.get(id);
        let name = format!("{}ce{}", self.prefix, self.cells.len() + 1);
        let data = self.data.name_for(&style.num_fmt).map(|d| format!("{}{d}", self.prefix));
        let xml = cell_style_xml(&name, style, data.as_deref(), &wb.styles.theme);
        self.xml.push_str(&xml);
        if style.font.name != crate::style::Font::default().name && !self.fonts.contains(&style.font.name) {
            self.fonts.push(style.font.name.clone());
        }
        self.cells.insert(id, name.clone());
        name
    }

    fn col(&mut self, width_chars: f64) -> String {
        let pt = pt_from_chars(width_chars);
        let key = (pt * 100.0).round() as u64;
        if let Some((_, n)) = self.cols.iter().find(|(k, _)| *k == key) {
            return n.clone();
        }
        let name = format!("{}co{}", self.prefix, self.cols.len() + 1);
        let _ = write!(
            self.xml,
            "<style:style style:name=\"{name}\" style:family=\"table-column\"><style:table-column-properties fo:break-before=\"auto\" style:column-width=\"{:.4}in\"/></style:style>",
            pt / 72.0
        );
        self.cols.push((key, name.clone()));
        name
    }

    fn row(&mut self, height_pt: f64, custom: bool) -> String {
        let key = (height_pt * 100.0).round() as u64;
        if let Some((_, _, n)) = self.rows.iter().find(|(k, c, _)| *k == key && *c == custom) {
            return n.clone();
        }
        let name = format!("{}ro{}", self.prefix, self.rows.len() + 1);
        let _ = write!(
            self.xml,
            "<style:style style:name=\"{name}\" style:family=\"table-row\"><style:table-row-properties style:row-height=\"{:.4}in\" fo:break-before=\"auto\" style:use-optimal-row-height=\"{}\"/></style:style>",
            height_pt / 72.0,
            !custom
        );
        self.rows.push((key, custom, name.clone()));
        name
    }
}

fn write_into(wb: &mut Workbook, pkg: &mut Package) -> Result<Vec<u8>, SheetError> {
    let content = &pkg.content;
    let prefix = (1..)
        .map(|i| format!("dh{i}"))
        .find(|p| !content.contains(&format!("\"{p}")))
        .unwrap_or_else(|| "dhx".into());
    let mut names = Names {
        prefix: prefix.clone(),
        cells: BTreeMap::new(),
        cols: Vec::new(),
        rows: Vec::new(),
        data: DataStyles::default(),
        xml: String::new(),
        fonts: Vec::new(),
    };
    let _ = write!(
        names.xml,
        "<style:style style:name=\"{prefix}ta1\" style:family=\"table\"><style:table-properties table:display=\"true\" style:writing-mode=\"lr-tb\"/></style:style><style:style style:name=\"{prefix}ta2\" style:family=\"table\"><style:table-properties table:display=\"false\" style:writing-mode=\"lr-tb\"/></style:style>"
    );

    // The tables, unchanged ones copied as written.
    let mut tables = String::new();
    for (i, sheet) in wb.sheets.iter().enumerate() {
        let reuse = sheet.ods.as_ref().and_then(|p| {
            let o = p.original?;
            let same = !sheet.modified && p.name == sheet.name && p.hidden == (sheet.state != SheetState::Visible);
            if same {
                pkg.tables.get(o).map(|(a, b)| &content[*a..*b])
            } else {
                None
            }
        });
        match reuse {
            Some(raw) => tables.push_str(raw),
            None => tables.push_str(&table_xml(wb, i, &mut names)),
        }
    }

    // Automatic styles: the original ones, then this save's.
    let data_xml = names
        .data
        .xml
        .replace("style:name=\"dhN", &format!("style:name=\"{prefix}dhN"))
        .replace("style:apply-style-name=\"dhN", &format!("style:apply-style-name=\"{prefix}dhN"));
    let mut auto = String::from("<office:automatic-styles>");
    if let Some((a, b)) = pkg.auto_styles {
        let original = &content[a..b];
        if let (Some(open_end), Some(close)) = (original.find('>'), original.rfind("</")) {
            if !original[..open_end].ends_with('/') && close > open_end {
                auto.push_str(&original[open_end + 1..close]);
            }
        }
    }
    auto.push_str(&data_xml);
    auto.push_str(&names.xml);
    auto.push_str("</office:automatic-styles>");

    // Fonts the new styles use.
    let mut fonts = None;
    let missing: Vec<&String> = names.fonts.iter().filter(|f| !pkg.fonts.contains_key(*f)).collect();
    if !missing.is_empty() {
        let mut decls = String::from("<office:font-face-decls>");
        if let Some((a, b)) = pkg.font_decls {
            let original = &content[a..b];
            if let (Some(open_end), Some(close)) = (original.find('>'), original.rfind("</")) {
                if !original[..open_end].ends_with('/') && close > open_end {
                    decls.push_str(&original[open_end + 1..close]);
                }
            }
        }
        for f in missing {
            decls.push_str(&font_face_xml(f));
        }
        decls.push_str("</office:font-face-decls>");
        fonts = Some(decls);
    }

    // Splice: fonts, automatic styles, tables, in document order.
    let mut edits: Vec<(usize, usize, String)> = Vec::new();
    match pkg.font_decls {
        Some((a, b)) => {
            if let Some(f) = fonts {
                edits.push((a, b, f));
            }
        }
        None => {
            if let Some(f) = fonts {
                let at = pkg.auto_styles.map(|s| s.0).unwrap_or_else(|| content.find("<office:body").unwrap_or(0));
                edits.push((at, at, f));
            }
        }
    }
    match pkg.auto_styles {
        Some((a, b)) => edits.push((a, b, auto)),
        None => {
            let at = content.find("<office:body").unwrap_or(0);
            edits.push((at, at, auto));
        }
    }
    edits.push((pkg.tables_region.0, pkg.tables_region.1, tables));
    edits.sort_by_key(|e| (e.0, e.1));
    let mut out = String::with_capacity(content.len() + 1024);
    let mut pos = 0;
    for (a, b, text) in edits {
        if a < pos {
            continue;
        }
        out.push_str(&content[pos..a]);
        out.push_str(&text);
        pos = b;
    }
    out.push_str(&content[pos..]);
    pkg.opc.set("content.xml", out.into_bytes());

    // Frozen panes and the active sheet.
    let settings = if pkg.opc.exists("settings.xml") {
        pkg.opc.read_xml("settings.xml").ok()
    } else {
        None
    };
    let mut settings = match settings {
        Some(s) => s,
        None => Document::parse_str(template::SETTINGS).map_err(corrupt)?,
    };
    update_settings(&mut settings, wb);
    pkg.opc.set_xml("settings.xml", &settings);
    if !pkg.opc.exists("META-INF/manifest.xml") || !pkg.opc.exists("mimetype") {
        return Err(SheetError::Corrupt("The OpenDocument package is missing its manifest.".into()));
    }
    pkg.opc.save().map_err(corrupt)
}

fn config_item(name: &str, kind: &str, value: &str) -> Element {
    let mut e = Element::new("config:config-item", doc_xml::ns::Ns::from_uri("urn:oasis:names:tc:opendocument:xmlns:config:1.0"));
    e.set_attr("config:name", doc_xml::ns::NONE, name);
    e.set_attr("config:type", doc_xml::ns::NONE, kind);
    e.set_text(value);
    e
}

fn set_item(parent: &mut Element, name: &str, kind: &str, value: &str) {
    for c in parent.elements_mut() {
        if c.local() == "config-item" && super::styles::attr(c, "name") == Some(name) {
            c.set_text(value);
            return;
        }
    }
    parent.push(config_item(name, kind, value));
}

fn update_settings(doc: &mut Document, wb: &Workbook) {
    let config_ns = doc_xml::ns::Ns::from_uri("urn:oasis:names:tc:opendocument:xmlns:config:1.0");
    // The first view entry.
    fn find_view(e: &mut Element) -> Option<&mut Element> {
        if e.local() == "config-item-map-indexed" && super::styles::attr(e, "name") == Some("Views") {
            return e.elements_mut().find(|c| c.local() == "config-item-map-entry");
        }
        for c in e.elements_mut() {
            if let Some(found) = find_view(c) {
                return Some(found);
            }
        }
        None
    }
    if find_view(&mut doc.root).is_none() {
        // No view settings yet: add them.
        let office_ns = doc_xml::ns::OFFICE;
        let settings_idx = doc.root.children.iter().position(|n| n.as_element().is_some_and(|e| e.local() == "settings"));
        let settings_idx = match settings_idx {
            Some(i) => i,
            None => {
                doc.root.push(Element::new("office:settings", office_ns));
                doc.root.children.len() - 1
            }
        };
        let Some(settings) = doc.root.children[settings_idx].as_element_mut() else { return };
        let mut set = Element::new("config:config-item-set", config_ns);
        set.set_attr("config:name", doc_xml::ns::NONE, "ooo:view-settings");
        let mut views = Element::new("config:config-item-map-indexed", config_ns);
        views.set_attr("config:name", doc_xml::ns::NONE, "Views");
        let mut entry = Element::new("config:config-item-map-entry", config_ns);
        entry.push(config_item("ViewId", "string", "view1"));
        views.push(entry);
        set.push(views);
        settings.children.insert(0, doc_xml::Node::Element(set));
        if doc.root.attr(doc_xml::ns::XMLNS, "config").is_none() && !doc.root.attrs.iter().any(|a| a.name == "xmlns:config") {
            doc.root.set_attr("xmlns:config", doc_xml::ns::XMLNS, "urn:oasis:names:tc:opendocument:xmlns:config:1.0");
        }
    }
    let Some(view) = find_view(&mut doc.root) else { return };
    if let Some(active) = wb.sheets.get(wb.active) {
        set_item(view, "ActiveTable", "string", &active.name);
    }
    let has_tables = view
        .elements()
        .any(|c| c.local() == "config-item-map-named" && super::styles::attr(c, "name") == Some("Tables"));
    if !has_tables {
        let mut t = Element::new("config:config-item-map-named", config_ns);
        t.set_attr("config:name", doc_xml::ns::NONE, "Tables");
        view.push(t);
    }
    let Some(tables) = view
        .elements_mut()
        .find(|c| c.local() == "config-item-map-named" && super::styles::attr(c, "name") == Some("Tables"))
    else {
        return;
    };
    // Entries for sheets that no longer exist are dropped.
    tables.children.retain(|n| {
        n.as_element()
            .and_then(|e| super::styles::attr(e, "name"))
            .is_none_or(|name| wb.sheet_index(name).is_some())
    });
    for sheet in &wb.sheets {
        let exists = tables.elements().any(|e| super::styles::attr(e, "name") == Some(sheet.name.as_str()));
        if !exists {
            let mut entry = Element::new("config:config-item-map-entry", config_ns);
            entry.set_attr("config:name", doc_xml::ns::NONE, sheet.name.clone());
            tables.push(entry);
        }
        let Some(entry) = tables.elements_mut().find(|e| super::styles::attr(e, "name") == Some(sheet.name.as_str())) else {
            continue;
        };
        let (rows, cols) = sheet.freeze.map_or((0, 0), |f| (f.row, f.col));
        set_item(entry, "HorizontalSplitMode", "short", if cols > 0 { "2" } else { "0" });
        set_item(entry, "VerticalSplitMode", "short", if rows > 0 { "2" } else { "0" });
        set_item(entry, "HorizontalSplitPosition", "int", &cols.to_string());
        set_item(entry, "VerticalSplitPosition", "int", &rows.to_string());
        set_item(entry, "PositionRight", "int", &cols.to_string());
        set_item(entry, "PositionBottom", "int", &rows.to_string());
        set_item(entry, "ShowGrid", "boolean", if sheet.show_grid { "true" } else { "false" });
    }
}

/// Text as ODF paragraphs: lines become `text:p`, runs of spaces
/// `text:s`, tabs `text:tab`.
fn paragraphs(text: &str, out: &mut String) {
    for line in text.split('\n') {
        out.push_str("<text:p>");
        let mut spaces = 0usize;
        let mut first = true;
        let flush = |spaces: &mut usize, out: &mut String, at_start: bool| {
            if *spaces == 0 {
                return;
            }
            if at_start {
                if *spaces == 1 {
                    out.push_str("<text:s/>");
                } else {
                    let _ = write!(out, "<text:s text:c=\"{spaces}\"/>");
                }
            } else {
                out.push(' ');
                if *spaces > 1 {
                    let _ = write!(out, "<text:s text:c=\"{}\"/>", *spaces - 1);
                }
            }
            *spaces = 0;
        };
        for c in line.chars() {
            match c {
                ' ' => spaces += 1,
                '\t' => {
                    flush(&mut spaces, out, first);
                    out.push_str("<text:tab/>");
                    first = false;
                }
                '\r' => {}
                c => {
                    flush(&mut spaces, out, first);
                    first = false;
                    let mut buf = [0u8; 4];
                    escape_text(c.encode_utf8(&mut buf), out);
                }
            }
        }
        flush(&mut spaces, out, first);
        out.push_str("</text:p>");
    }
}

fn duration(serial: f64) -> String {
    let total_ms = (serial * 86_400_000.0).round() as i64;
    let neg = total_ms < 0;
    let ms = total_ms.unsigned_abs();
    let h = ms / 3_600_000;
    let m = (ms / 60_000) % 60;
    let s = (ms / 1000) % 60;
    let frac = ms % 1000;
    let sign = if neg { "-" } else { "" };
    if frac > 0 {
        format!("{sign}PT{h:02}H{m:02}M{s:02}.{frac:03}S")
    } else {
        format!("{sign}PT{h:02}H{m:02}M{s:02}S")
    }
}

fn cell_xml(wb: &Workbook, sheet: &Sheet, at: CellRef, cell: &Cell, names: &mut Names, out: &mut String) {
    out.push_str("<table:table-cell");
    if cell.style != 0 {
        let name = names.cell(wb, cell.style);
        let _ = write!(out, " table:style-name=\"{}\"", attr_text(&name));
    }
    if let Some(m) = sheet.merges.iter().find(|m| m.start == at) {
        let _ = write!(out, " table:number-columns-spanned=\"{}\" table:number-rows-spanned=\"{}\"", m.cols(), m.rows());
    }
    let formula = cell.formula.as_deref().filter(|f| f.raw_xml.is_none());
    if let Some(f) = formula {
        let text = match &f.expr {
            Some(e) => crate::formula::unparse_odf(e),
            None => format!("of:={}", f.text),
        };
        let _ = write!(out, " table:formula=\"{}\"", attr_text(&text));
        if let Some(a) = f.array {
            let _ = write!(
                out,
                " table:number-matrix-columns-spanned=\"{}\" table:number-matrix-rows-spanned=\"{}\"",
                a.cols(),
                a.rows()
            );
        }
    }
    let style = wb.styles.get(cell.style);
    let fmt = NumberFormat::cached(&style.num_fmt);
    let mut text = String::new();
    match &cell.value {
        Value::Empty => {}
        Value::Number(n) => {
            let shown = crate::display::shown_text(wb, cell);
            if fmt.is_date() && has_date_part(&style.num_fmt) {
                match crate::datetime::serial_to_iso(*n, wb.date1904) {
                    Some(iso) => {
                        let _ = write!(out, " office:value-type=\"date\" office:date-value=\"{iso}\" calcext:value-type=\"date\"");
                    }
                    None => {
                        let _ = write!(out, " office:value-type=\"float\" office:value=\"{n}\" calcext:value-type=\"float\"");
                    }
                }
            } else if fmt.is_date() {
                let _ = write!(out, " office:value-type=\"time\" office:time-value=\"{}\" calcext:value-type=\"time\"", duration(*n));
            } else if fmt.is_percent() {
                let _ = write!(out, " office:value-type=\"percentage\" office:value=\"{n}\" calcext:value-type=\"percentage\"");
            } else {
                let _ = write!(out, " office:value-type=\"float\" office:value=\"{n}\" calcext:value-type=\"float\"");
            }
            paragraphs(&shown, &mut text);
        }
        Value::Text(t) => {
            out.push_str(" office:value-type=\"string\"");
            if formula.is_some() {
                let _ = write!(out, " office:string-value=\"{}\"", attr_text(t));
            }
            out.push_str(" calcext:value-type=\"string\"");
            paragraphs(t, &mut text);
        }
        Value::Bool(b) => {
            let _ = write!(out, " office:value-type=\"boolean\" office:boolean-value=\"{b}\" calcext:value-type=\"boolean\"");
            paragraphs(if *b { "TRUE" } else { "FALSE" }, &mut text);
        }
        Value::Error(e) => {
            if formula.is_some() {
                out.push_str(" office:value-type=\"float\" office:value=\"0\" calcext:value-type=\"error\"");
            } else {
                out.push_str(" office:value-type=\"string\" calcext:value-type=\"string\"");
            }
            paragraphs(e.as_str(), &mut text);
        }
    }
    let extra = cell.extra.as_ref().and_then(|e| e.ods.as_ref());
    if let Some(x) = extra {
        for (k, v) in &x.attrs {
            let _ = write!(out, " {k}=\"{}\"", attr_text(v));
        }
    }
    let annotation = extra.and_then(|x| x.annotation.as_deref());
    if text.is_empty() && annotation.is_none() {
        out.push_str("/>");
        return;
    }
    out.push('>');
    if let Some(a) = annotation {
        out.push_str(a);
    }
    out.push_str(&text);
    out.push_str("</table:table-cell>");
}

fn table_xml(wb: &Workbook, index: usize, names: &mut Names) -> String {
    let sheet = &wb.sheets[index];
    let part = sheet.ods.as_deref();
    let mut out = String::new();
    let table_style = format!("{}ta{}", names.prefix, if sheet.state == SheetState::Visible { 1 } else { 2 });
    let _ = write!(out, "<table:table table:name=\"{}\" table:style-name=\"{table_style}\"", attr_text(&sheet.name));
    if let Some(p) = part {
        for (k, v) in &p.attrs {
            let _ = write!(out, " {k}=\"{}\"", attr_text(v));
        }
    }
    out.push('>');
    if let Some(p) = part {
        out.push_str(&p.head);
    }
    let used = sheet.used_range();
    let mut last_col = used.map_or(0, |u| u.end.col);
    if let Some((&c, _)) = sheet.cols.iter().next_back() {
        last_col = last_col.max(c);
    }
    for m in &sheet.merges {
        last_col = last_col.max(m.end.col);
    }
    // Columns, grouping equal neighbours.
    let mut c = 0u32;
    while c <= last_col {
        let info = sheet.cols.get(&c);
        let key = (info.and_then(|i| i.width), info.is_some_and(|i| i.hidden), info.and_then(|i| i.style));
        let mut n = 1;
        while c + n <= last_col {
            let next = sheet.cols.get(&(c + n));
            if (next.and_then(|i| i.width), next.is_some_and(|i| i.hidden), next.and_then(|i| i.style)) != key {
                break;
            }
            n += 1;
        }
        let style = names.col(key.0.unwrap_or(sheet.default_col_width));
        let default_cell = match key.2 {
            Some(s) if s != 0 => names.cell(wb, s),
            _ => "Default".into(),
        };
        let _ = write!(out, "<table:table-column table:style-name=\"{style}\"");
        if n > 1 {
            let _ = write!(out, " table:number-columns-repeated=\"{n}\"");
        }
        if key.1 {
            out.push_str(" table:visibility=\"collapse\"");
        }
        let _ = write!(out, " table:default-cell-style-name=\"{}\"/>", attr_text(&default_cell));
        c += n;
    }
    // Rows.
    let mut last_row = used.map_or(0, |u| u.end.row);
    if let Some((&r, _)) = sheet.rows.iter().next_back() {
        last_row = last_row.max(r);
    }
    for m in &sheet.merges {
        last_row = last_row.max(m.end.row);
    }
    let covered = |r: u32, c: u32| sheet.merges.iter().any(|m| m.contains(CellRef::new(r, c)) && m.start != CellRef::new(r, c));
    let empty: BTreeMap<u32, Cell> = BTreeMap::new();
    let mut r = 0u32;
    while r <= last_row {
        let info = sheet.rows.get(&r);
        let height = info.and_then(|i| i.height).unwrap_or(sheet.default_row_height);
        let custom = info.is_some_and(|i| i.custom_height);
        let hidden = info.is_some_and(|i| i.hidden);
        let row_style = names.row(height, custom);
        let cells = sheet.cells.get(&r).unwrap_or(&empty);
        let has_merge = sheet.merges.iter().any(|m| (m.start.row..=m.end.row).contains(&r));
        if cells.is_empty() && !has_merge {
            // Group identical empty rows.
            let mut n = 1;
            while r + n <= last_row
                && sheet.cells.get(&(r + n)).is_none_or(|c| c.is_empty())
                && !sheet.merges.iter().any(|m| (m.start.row..=m.end.row).contains(&(r + n)))
                && sheet.rows.get(&(r + n)) == info
            {
                n += 1;
            }
            let _ = write!(out, "<table:table-row table:style-name=\"{row_style}\"");
            if n > 1 {
                let _ = write!(out, " table:number-rows-repeated=\"{n}\"");
            }
            if hidden {
                out.push_str(" table:visibility=\"collapse\"");
            }
            let _ = write!(out, "><table:table-cell table:number-columns-repeated=\"{}\"/></table:table-row>", last_col + 1);
            r += n;
            continue;
        }
        let _ = write!(out, "<table:table-row table:style-name=\"{row_style}\"");
        if hidden {
            out.push_str(" table:visibility=\"collapse\"");
        }
        out.push('>');
        let mut c = 0u32;
        while c <= last_col {
            if covered(r, c) {
                out.push_str("<table:covered-table-cell/>");
                c += 1;
                continue;
            }
            match cells.get(&c) {
                Some(cell) => {
                    cell_xml(wb, sheet, CellRef::new(r, c), cell, names, &mut out);
                    c += 1;
                }
                None => {
                    let mut n = 1;
                    while c + n <= last_col && !cells.contains_key(&(c + n)) && !covered(r, c + n) && !sheet.merges.iter().any(|m| m.start == CellRef::new(r, c + n)) {
                        n += 1;
                    }
                    if sheet.merges.iter().any(|m| m.start == CellRef::new(r, c)) {
                        cell_xml(wb, sheet, CellRef::new(r, c), &Cell::default(), names, &mut out);
                        c += 1;
                        continue;
                    }
                    if n > 1 {
                        let _ = write!(out, "<table:table-cell table:number-columns-repeated=\"{n}\"/>");
                    } else {
                        out.push_str("<table:table-cell/>");
                    }
                    c += n;
                }
            }
        }
        out.push_str("</table:table-row>");
        r += 1;
    }
    if let Some(p) = part {
        out.push_str(&p.tail);
    }
    out.push_str("</table:table>");
    out
}

/// Whether a date format shows a date (not only a time of day).
fn has_date_part(code: &str) -> bool {
    crate::numfmt::parse_sections_public(code).first().is_some_and(|s| {
        s.tokens
            .iter()
            .any(|t| matches!(t, crate::numfmt::Token::Year(_) | crate::numfmt::Token::Month(_) | crate::numfmt::Token::Day(_)))
    })
}
