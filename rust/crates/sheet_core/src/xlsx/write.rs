//! Writing a workbook as XLSX, in place when it came from one.

use std::collections::{BTreeSet, HashMap};
use std::sync::Arc;

use doc_opc::rel;
use doc_xml::{escape_attr, escape_text, ns, Document, Element, Node};

use super::read::fresh_package;
use super::styles::append_styles;
use super::{child_or_insert, ct, insert_ordered, qname, Package, SheetPart, SstEntry, WORKSHEET_ORDER};
use crate::error::SheetError;
use crate::formula::adjust::shift_range;
use crate::model::{Cell, Format, Sheet, SheetState, Source, Workbook};
use crate::refs::{col_to_letters, CellRef, Range};
use crate::value::Value;

fn corrupt(e: impl std::fmt::Display) -> SheetError {
    SheetError::Corrupt(e.to_string())
}

const MARKER: &str = "@@DOCHELPER-SHEETDATA-7F3A9C@@";

/// The order of the workbook part's children.
const WORKBOOK_ORDER: &[&str] = &[
    "fileVersion",
    "fileSharing",
    "workbookPr",
    "workbookProtection",
    "bookViews",
    "sheets",
    "functionGroups",
    "externalReferences",
    "definedNames",
    "calcPr",
    "oleSize",
    "customWorkbookViews",
    "pivotCaches",
    "smartTagPr",
    "smartTagTypes",
    "webPublishing",
    "fileRecoveryPr",
    "webPublishObjects",
    "extLst",
];

/// Write the workbook as XLSX (or XLSM). A workbook read from an XLSX file
/// is written in place: unchanged parts are copied as they were.
pub fn write(wb: &mut Workbook, format: Format) -> Result<Vec<u8>, SheetError> {
    let own = matches!(wb.source, Source::Xlsx(_));
    let mut pkg = match std::mem::take(&mut wb.source) {
        Source::Xlsx(p) => *p,
        other => {
            wb.source = other;
            fresh_package(&wb.format_options())?
        }
    };
    let result = write_into(wb, &mut pkg, format);
    if own {
        wb.source = Source::Xlsx(Box::new(pkg));
    } else {
        // The parts belong to a package this workbook does not keep.
        for s in &mut wb.sheets {
            s.xlsx = None;
        }
    }
    result
}

/// Number text as Excel writes it: shortest round-trip form.
fn number_text(n: f64) -> String {
    if n == 0.0 {
        return "0".into();
    }
    let a = n.abs();
    if !(1e-5..1e15).contains(&a) {
        let s = format!("{n:E}");
        return s;
    }
    let s = format!("{n}");
    s
}

/// Shared strings: those the file had, in order, then new ones.
struct SstBuilder {
    entries: Vec<SstEntry>,
    lookup: HashMap<Arc<str>, u32>,
    original: usize,
    refs: usize,
}

impl SstBuilder {
    fn new(existing: &[SstEntry]) -> Self {
        let mut lookup = HashMap::new();
        for (i, e) in existing.iter().enumerate() {
            if let SstEntry::Plain(t) = e {
                lookup.entry(t.clone()).or_insert(i as u32);
            }
        }
        SstBuilder {
            entries: existing.to_vec(),
            lookup,
            original: existing.len(),
            refs: 0,
        }
    }

    fn index(&mut self, cell: &Cell, text: &Arc<str>) -> u32 {
        self.refs += 1;
        if let Some((xml, i)) = cell.extra.as_ref().and_then(|e| e.rich.as_ref()) {
            if let Some(SstEntry::Rich { xml: x, .. }) = self.entries.get(*i as usize) {
                if Arc::ptr_eq(x, xml) || x == xml {
                    return *i;
                }
            }
            self.entries.push(SstEntry::Rich {
                xml: xml.clone(),
                text: text.clone(),
            });
            return (self.entries.len() - 1) as u32;
        }
        if let Some(&i) = self.lookup.get(text) {
            return i;
        }
        let i = self.entries.len() as u32;
        self.entries.push(SstEntry::Plain(text.clone()));
        self.lookup.insert(text.clone(), i);
        i
    }

    fn changed(&self) -> bool {
        self.entries.len() != self.original
    }
}

fn needs_preserve(t: &str) -> bool {
    t.starts_with(char::is_whitespace) || t.ends_with(char::is_whitespace) || t.contains('\n')
}

fn sst_xml(pkg: &Package, sst: &SstBuilder) -> String {
    let (root_name, mut attrs) = pkg.sst_root.clone().unwrap_or_else(|| {
        (
            "sst".to_owned(),
            vec![("xmlns".to_owned(), "http://schemas.openxmlformats.org/spreadsheetml/2006/main".to_owned())],
        )
    });
    let prefix = root_name.rsplit_once(':').map_or("", |(p, _)| p).to_owned();
    let count = sst.refs.max(sst.entries.len()).to_string();
    let unique = sst.entries.len().to_string();
    let mut set = |k: &str, v: &str| match attrs.iter_mut().find(|(n, _)| n == k) {
        Some(a) => a.1 = v.to_owned(),
        None => attrs.push((k.to_owned(), v.to_owned())),
    };
    set("count", &count);
    set("uniqueCount", &unique);
    let mut out = String::with_capacity(sst.entries.len() * 24 + 256);
    out.push_str(r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>"#);
    out.push_str("\r\n<");
    out.push_str(&root_name);
    for (k, v) in &attrs {
        out.push(' ');
        out.push_str(k);
        out.push_str("=\"");
        escape_attr(v, &mut out);
        out.push('"');
    }
    out.push('>');
    let si = qname(&prefix, "si");
    let t = qname(&prefix, "t");
    for e in &sst.entries {
        out.push('<');
        out.push_str(&si);
        out.push('>');
        match e {
            SstEntry::Plain(text) => {
                out.push('<');
                out.push_str(&t);
                if needs_preserve(text) {
                    out.push_str(r#" xml:space="preserve""#);
                }
                out.push('>');
                escape_text(&doc_xml::ooxml::encode_xstring(text), &mut out);
                out.push_str("</");
                out.push_str(&t);
                out.push('>');
            }
            SstEntry::Rich { xml, .. } => out.push_str(xml),
        }
        out.push_str("</");
        out.push_str(&si);
        out.push('>');
    }
    out.push_str("</");
    out.push_str(&root_name);
    out.push('>');
    out
}

fn write_into(wb: &mut Workbook, pkg: &mut Package, format: Format) -> Result<Vec<u8>, SheetError> {
    let options = wb.format_options();
    let workbook_part = pkg.workbook_part.clone();

    // Which sheets need a new part, and which need writing.
    let mut regenerate = vec![false; wb.sheets.len()];
    let mut new_sheet_added = false;
    let active_changed = wb.active != pkg.original_active;
    for (i, sheet) in wb.sheets.iter_mut().enumerate() {
        let usable = sheet
            .xlsx
            .as_ref()
            .is_some_and(|p| !p.detached && (p.chart || pkg.opc.exists(&p.path)) && pkg.workbook_rels.by_id(&p.rel_id).is_some());
        if !usable {
            let mut part = sheet.xlsx.take().map(|b| *b).unwrap_or_else(|| SheetPart {
                path: String::new(),
                rel_id: String::new(),
                sheet_id: 0,
                sheet_attrs: Vec::new(),
                doc: None,
                chart: false,
                original_freeze: None,
                tables: Vec::new(),
                shifts: Vec::new(),
                detached: true,
            });
            part.path = pkg.opc.unused_name("xl/worksheets/sheet{}.xml");
            // Reserve the name now, so the next new sheet gets another.
            pkg.opc.set(&part.path, Vec::new());
            part.rel_id = pkg.workbook_rels.add(&rel::uri(rel::WORKSHEET), &part.path);
            part.chart = false;
            part.detached = false;
            part.tables.clear();
            part.shifts.clear();
            sheet.xlsx = Some(Box::new(part));
            regenerate[i] = true;
            new_sheet_added = true;
        } else if sheet.modified && !sheet.xlsx.as_ref().is_some_and(|p| p.chart) {
            regenerate[i] = true;
        }
        // Only the active sheet may be selected, or Excel groups them.
        if active_changed && !regenerate[i] {
            let selected = sheet
                .xlsx
                .as_ref()
                .and_then(|p| p.doc.as_ref())
                .and_then(|d| d.root.elements().find(|e| e.local() == "sheetViews"))
                .and_then(|v| v.elements().find(|e| e.local() == "sheetView"))
                .is_some_and(|v| v.attr(ns::NONE, "tabSelected").is_some_and(|t| t == "1" || t == "true"));
            if selected != (i == wb.active) && !sheet.xlsx.as_ref().is_some_and(|p| p.chart) {
                regenerate[i] = true;
            }
        }
    }
    // Sheet ids must be unique.
    let mut used_ids: BTreeSet<u32> = BTreeSet::new();
    for sheet in &mut wb.sheets {
        if let Some(p) = &mut sheet.xlsx {
            if p.sheet_id == 0 || !used_ids.insert(p.sheet_id) {
                p.sheet_id = 0;
            }
        }
    }
    for sheet in &mut wb.sheets {
        if let Some(p) = &mut sheet.xlsx {
            if p.sheet_id == 0 {
                p.sheet_id = used_ids.iter().next_back().copied().unwrap_or(0) + 1;
                used_ids.insert(p.sheet_id);
            }
        }
    }

    // Sheets removed since reading: drop their parts and relationships.
    let in_use: BTreeSet<String> = wb.sheets.iter().filter_map(|s| s.xlsx.as_ref().map(|p| p.rel_id.clone())).collect();
    let mut rels_changed = new_sheet_added;
    for r in pkg.workbook_rels.list() {
        if (rel::is(&r.rel_type, rel::WORKSHEET) || rel::is(&r.rel_type, rel::CHARTSHEET)) && !in_use.contains(&r.id) {
            if let Some(part) = pkg.workbook_rels.part_of(&r) {
                pkg.opc.remove_part(&part).map_err(corrupt)?;
            }
            pkg.workbook_rels.remove(&r.id);
            rels_changed = true;
        }
    }

    // Cells of changed sheets, and the shared strings they use.
    let any_regenerated = regenerate.iter().any(|r| *r);
    let mut sst = SstBuilder::new(&pkg.sst);
    for (i, regen) in regenerate.iter().enumerate() {
        if !regen {
            continue;
        }
        let xml = sheet_xml(wb, i, &mut sst)?;
        let part = wb.sheets[i].xlsx.as_ref().expect("every sheet has a part by now");
        let path = part.path.clone();
        pkg.opc.set(&path, xml.into_bytes());
        pkg.opc.set_override(&path, ct::WORKSHEET).map_err(corrupt)?;
        let shifts = part.shifts.clone();
        let tables = part.tables.clone();
        // Tables follow inserted and deleted rows and columns.
        for (table_part, _) in &tables {
            if shifts.is_empty() {
                break;
            }
            if let Ok(mut doc) = pkg.opc.read_xml(table_part) {
                shift_table(&mut doc.root, &shifts);
                pkg.opc.set_xml(table_part, &doc);
            }
        }
        let freeze = wb.sheets[i].freeze;
        if let Some(p) = &mut wb.sheets[i].xlsx {
            p.shifts.clear();
            p.original_freeze = freeze;
        }
    }
    if any_regenerated && (sst.changed() || (pkg.sst_part.is_none() && !sst.entries.is_empty())) {
        let path = match &pkg.sst_part {
            Some(p) => p.clone(),
            None => {
                let p = format!("{}/sharedStrings.xml", doc_opc::dir_of(&workbook_part));
                pkg.workbook_rels.add(&rel::uri(rel::SHARED_STRINGS), &p);
                rels_changed = true;
                pkg.sst_part = Some(p.clone());
                p
            }
        };
        pkg.opc.set(&path, sst_xml(pkg, &sst).into_bytes());
        pkg.opc.set_override(&path, ct::SHARED_STRINGS).map_err(corrupt)?;
        if pkg.sst_root.is_none() {
            pkg.sst_root = Some((
                "sst".into(),
                vec![("xmlns".into(), "http://schemas.openxmlformats.org/spreadsheetml/2006/main".into())],
            ));
        }
    }
    pkg.sst = sst.entries;

    // New styles.
    if wb.styles.len() > pkg.xf_count {
        if pkg.styles.is_none() {
            let fresh = fresh_package(&options)?;
            let path = format!("{}/styles.xml", doc_opc::dir_of(&workbook_part));
            pkg.workbook_rels.add(&rel::uri(rel::STYLES), &path);
            rels_changed = true;
            pkg.styles = fresh.styles;
            pkg.styles_part = Some(path);
            pkg.fonts = fresh.fonts;
            pkg.fills = fresh.fills;
            pkg.borders = fresh.borders;
            pkg.num_fmts = fresh.num_fmts;
            pkg.xf_count = fresh.xf_count;
        }
        append_styles(pkg, wb.styles.all(), &options);
        if let (Some(path), Some(doc)) = (&pkg.styles_part, &pkg.styles) {
            pkg.opc.set_xml(path, doc);
            pkg.opc.set_override(path, ct::STYLES).map_err(corrupt)?;
        }
    }

    // The calculation chain lists formula cells; Excel rebuilds it.
    if any_regenerated || wb.structure_changed {
        for r in pkg.workbook_rels.of_kind(rel::CALC_CHAIN) {
            if let Some(part) = pkg.workbook_rels.part_of(&r) {
                pkg.opc.remove_part(&part).map_err(corrupt)?;
            }
            pkg.workbook_rels.remove(&r.id);
            rels_changed = true;
        }
    }

    // Macros only travel in .xlsm.
    let content_type = match format {
        Format::Xlsm => ct::WORKBOOK_MACRO,
        _ => ct::WORKBOOK,
    };
    if format != Format::Xlsm {
        for r in pkg.workbook_rels.of_kind(rel::VBA_PROJECT) {
            if let Some(part) = pkg.workbook_rels.part_of(&r) {
                pkg.opc.remove_part(&part).map_err(corrupt)?;
            }
            pkg.workbook_rels.remove(&r.id);
            rels_changed = true;
        }
    }
    if pkg.main_content_type != content_type {
        pkg.opc.set_override(&workbook_part, content_type).map_err(corrupt)?;
        pkg.main_content_type = content_type.to_owned();
    }

    // The workbook part.
    if wb.structure_changed || wb.recalculated || active_changed || new_sheet_added || rels_changed {
        write_workbook_part(wb, pkg)?;
        pkg.opc.set_xml(&workbook_part, &pkg.workbook);
    }
    if rels_changed {
        let rels = pkg.workbook_rels.clone();
        pkg.opc.set_rels(&rels);
    }
    pkg.original_active = wb.active;
    pkg.opc.save().map_err(corrupt)
}

fn write_workbook_part(wb: &Workbook, pkg: &mut Package) -> Result<(), SheetError> {
    let doc = &mut pkg.workbook;
    let prefix = doc.prefix_for(ns::S).unwrap_or_default();
    let r_prefix = match doc.prefix_for(ns::R) {
        Some(p) if !p.is_empty() => p,
        _ => {
            doc.root.set_attr("xmlns:r", ns::XMLNS, ns::R.uri());
            "r".to_owned()
        }
    };
    // <sheets>
    let sheets = child_or_insert(&mut doc.root, "sheets", WORKBOOK_ORDER, ns::S, &prefix);
    sheets.children.clear();
    for s in &wb.sheets {
        let Some(p) = &s.xlsx else { continue };
        let mut e = Element::new(qname(&prefix, "sheet"), ns::S)
            .with_attr("name", ns::NONE, s.name.clone())
            .with_attr("sheetId", ns::NONE, p.sheet_id.to_string());
        if s.state != SheetState::Visible {
            e.set_attr("state", ns::NONE, s.state.name());
        }
        e.set_attr(&format!("{r_prefix}:id"), ns::R, p.rel_id.clone());
        for (k, v) in &p.sheet_attrs {
            e.attrs.push(doc_xml::Attr {
                name: k.clone(),
                ns: if k.contains(':') { ns::OVERFLOW } else { ns::NONE },
                value: v.clone(),
            });
        }
        sheets.push(e);
    }
    // <definedNames>
    doc.root.children.retain(|n| !n.as_element().is_some_and(|e| e.local() == "definedNames"));
    if !wb.names.is_empty() {
        let mut names = Element::new(qname(&prefix, "definedNames"), ns::S);
        for n in &wb.names {
            let mut e = Element::new(qname(&prefix, "definedName"), ns::S).with_attr("name", ns::NONE, n.name.clone());
            for (k, v) in &n.extra {
                e.set_attr(k, ns::NONE, v.clone());
            }
            if let Some(s) = n.sheet {
                e.set_attr("localSheetId", ns::NONE, s.to_string());
            }
            if n.hidden {
                e.set_attr("hidden", ns::NONE, "1");
            }
            let text = match &n.expr {
                Some(expr) => crate::formula::unparse_ooxml(expr),
                None => n.text.clone(),
            };
            e.set_text(text);
            names.push(e);
        }
        insert_ordered(&mut doc.root, names, WORKBOOK_ORDER);
    }
    // <bookViews>: the active sheet.
    let views = child_or_insert(&mut doc.root, "bookViews", WORKBOOK_ORDER, ns::S, &prefix);
    if views.elements().next().is_none() {
        views.push(Element::new(qname(&prefix, "workbookView"), ns::S));
    }
    if let Some(v) = views.elements_mut().next() {
        if wb.active == 0 {
            v.remove_attr(ns::NONE, "activeTab");
        } else {
            v.set_attr("activeTab", ns::NONE, wb.active.to_string());
        }
        let first = v.attr(ns::NONE, "firstSheet").and_then(|f| f.parse::<usize>().ok()).unwrap_or(0);
        if first > wb.active {
            v.remove_attr(ns::NONE, "firstSheet");
        }
    }
    // <calcPr>: values changed here, so Excel should recalculate.
    if wb.recalculated {
        let calc = child_or_insert(&mut doc.root, "calcPr", WORKBOOK_ORDER, ns::S, &prefix);
        calc.set_attr("fullCalcOnLoad", ns::NONE, "1");
    }
    Ok(())
}

/// Shift a table's `ref` and its filter's.
fn shift_table(root: &mut Element, shifts: &[crate::formula::adjust::Shift]) {
    let apply = |e: &mut Element| {
        if let Some(r) = e.attr(ns::NONE, "ref").and_then(Range::parse) {
            let mut range = Some(r);
            for s in shifts {
                range = range.and_then(|r| shift_range(r, *s));
            }
            if let Some(r) = range {
                e.set_attr("ref", ns::NONE, r.to_a1());
            }
        }
    };
    apply(root);
    for c in root.elements_mut() {
        if c.local() == "autoFilter" {
            apply(c);
        }
    }
}

/// The worksheet XML for one sheet.
fn sheet_xml(wb: &Workbook, index: usize, sst: &mut SstBuilder) -> Result<String, SheetError> {
    let sheet = &wb.sheets[index];
    let part = sheet.xlsx.as_ref().expect("a part was assigned");
    let mut doc = match &part.doc {
        Some(d) => d.clone(),
        None => Document::parse_str(super::template::WORKSHEET).map_err(corrupt)?,
    };
    let prefix = doc.prefix_for(ns::S).unwrap_or_default();
    let root = &mut doc.root;

    // Dimension.
    let dim = sheet.used_range().map_or_else(|| "A1".to_owned(), |r| r.to_a1());
    child_or_insert(root, "dimension", WORKSHEET_ORDER, ns::S, &prefix).set_attr("ref", ns::NONE, dim);

    // Views: frozen panes, gridlines, selection.
    let views = child_or_insert(root, "sheetViews", WORKSHEET_ORDER, ns::S, &prefix);
    if views.elements().next().is_none() {
        views.push(Element::new(qname(&prefix, "sheetView"), ns::S).with_attr("workbookViewId", ns::NONE, "0"));
    }
    let freeze_changed = part.doc.is_none() || part.original_freeze != sheet.freeze;
    for (vi, view) in views.elements_mut().enumerate() {
        if vi == 0 {
            if index == wb.active {
                view.set_attr("tabSelected", ns::NONE, "1");
            } else {
                view.remove_attr(ns::NONE, "tabSelected");
            }
            if sheet.show_grid {
                view.remove_attr(ns::NONE, "showGridLines");
            } else {
                view.set_attr("showGridLines", ns::NONE, "0");
            }
        } else {
            view.remove_attr(ns::NONE, "tabSelected");
        }
        if !freeze_changed {
            continue;
        }
        view.children.retain(|n| !n.as_element().is_some_and(|e| matches!(e.local(), "pane" | "selection")));
        if let (0, Some(at)) = (vi, sheet.freeze) {
            let active = match (at.row > 0, at.col > 0) {
                (true, true) => "bottomRight",
                (true, false) => "bottomLeft",
                _ => "topRight",
            };
            let mut pane = Element::new(qname(&prefix, "pane"), ns::S);
            if at.col > 0 {
                pane.set_attr("xSplit", ns::NONE, at.col.to_string());
            }
            if at.row > 0 {
                pane.set_attr("ySplit", ns::NONE, at.row.to_string());
            }
            pane.set_attr("topLeftCell", ns::NONE, at.to_a1());
            pane.set_attr("activePane", ns::NONE, active);
            pane.set_attr("state", ns::NONE, "frozen");
            view.children.insert(0, Node::Element(pane));
            view.children.insert(
                1,
                Node::Element(
                    Element::new(qname(&prefix, "selection"), ns::S)
                        .with_attr("pane", ns::NONE, active)
                        .with_attr("activeCell", ns::NONE, at.to_a1())
                        .with_attr("sqref", ns::NONE, at.to_a1()),
                ),
            );
        }
    }

    // Column widths.
    root.children.retain(|n| !n.as_element().is_some_and(|e| e.local() == "cols"));
    if !sheet.cols.is_empty() {
        let mut cols = Element::new(qname(&prefix, "cols"), ns::S);
        let entries: Vec<(&u32, &crate::model::ColInfo)> = sheet.cols.iter().collect();
        let mut i = 0;
        while i < entries.len() {
            let (&start, info) = entries[i];
            let mut end = start;
            while i + 1 < entries.len() && *entries[i + 1].0 == end + 1 && entries[i + 1].1 == info {
                end += 1;
                i += 1;
            }
            let mut c = Element::new(qname(&prefix, "col"), ns::S)
                .with_attr("min", ns::NONE, (start + 1).to_string())
                .with_attr("max", ns::NONE, (end + 1).to_string());
            let width = info.width.unwrap_or(sheet.default_col_width);
            c.set_attr("width", ns::NONE, number_text(width));
            if let Some(s) = info.style {
                c.set_attr("style", ns::NONE, s.to_string());
            }
            if info.hidden {
                c.set_attr("hidden", ns::NONE, "1");
            }
            if info.custom_width || info.width.is_some() {
                c.set_attr("customWidth", ns::NONE, "1");
            }
            for (k, v) in &info.extra {
                c.attrs.push(doc_xml::Attr {
                    name: k.clone(),
                    ns: if k.contains(':') { ns::OVERFLOW } else { ns::NONE },
                    value: v.clone(),
                });
            }
            cols.push(c);
            i += 1;
        }
        insert_ordered(root, cols, WORKSHEET_ORDER);
    }

    // The cells go where the placeholder is.
    let data = child_or_insert(root, "sheetData", WORKSHEET_ORDER, ns::S, &prefix);
    data.children = vec![Node::Text(MARKER.to_owned())];

    // Merged cells.
    root.children.retain(|n| !n.as_element().is_some_and(|e| e.local() == "mergeCells"));
    if !sheet.merges.is_empty() {
        let mut m = Element::new(qname(&prefix, "mergeCells"), ns::S).with_attr("count", ns::NONE, sheet.merges.len().to_string());
        for r in &sheet.merges {
            m.push(Element::new(qname(&prefix, "mergeCell"), ns::S).with_attr("ref", ns::NONE, r.to_a1()));
        }
        insert_ordered(root, m, WORKSHEET_ORDER);
    }

    let xml = doc.to_xml();
    let rows = rows_xml(wb, sheet, &prefix, sst);
    Ok(xml.replacen(MARKER, &rows, 1))
}

fn rows_xml(wb: &Workbook, sheet: &Sheet, prefix: &str, sst: &mut SstBuilder) -> String {
    let row_tag = qname(prefix, "row");
    let c_tag = qname(prefix, "c");
    let f_tag = qname(prefix, "f");
    let v_tag = qname(prefix, "v");
    let mut out = String::with_capacity(sheet.cell_count() * 40);
    let mut rows: BTreeSet<u32> = sheet.cells.keys().copied().collect();
    rows.extend(sheet.rows.keys().copied());
    let empty = std::collections::BTreeMap::new();
    for r in rows {
        let info = sheet.rows.get(&r);
        let cells = sheet.cells.get(&r).unwrap_or(&empty);
        out.push('<');
        out.push_str(&row_tag);
        out.push_str(" r=\"");
        out.push_str(&(r + 1).to_string());
        out.push('"');
        if let Some(info) = info {
            if let Some(s) = info.style {
                out.push_str(&format!(" s=\"{s}\" customFormat=\"1\""));
            }
            if let Some(h) = info.height {
                out.push_str(&format!(" ht=\"{}\"", number_text(h)));
                if info.custom_height {
                    out.push_str(" customHeight=\"1\"");
                }
            }
            if info.hidden {
                out.push_str(" hidden=\"1\"");
            }
            for (k, v) in &info.extra {
                out.push(' ');
                out.push_str(k);
                out.push_str("=\"");
                escape_attr(v, &mut out);
                out.push('"');
            }
        }
        if cells.is_empty() {
            out.push_str("/>");
            continue;
        }
        out.push('>');
        for (&c, cell) in cells {
            write_cell(&mut out, wb, CellRef::new(r, c), cell, &c_tag, &f_tag, &v_tag, sst);
        }
        out.push_str("</");
        out.push_str(&row_tag);
        out.push('>');
    }
    out
}

#[allow(clippy::too_many_arguments)]
fn write_cell(out: &mut String, wb: &Workbook, at: CellRef, cell: &Cell, c_tag: &str, f_tag: &str, v_tag: &str, sst: &mut SstBuilder) {
    out.push('<');
    out.push_str(c_tag);
    out.push_str(" r=\"");
    out.push_str(&col_to_letters(at.col));
    out.push_str(&(at.row + 1).to_string());
    out.push('"');
    if cell.style != 0 && (cell.style as usize) < wb.styles.len() {
        out.push_str(&format!(" s=\"{}\"", cell.style));
    }
    let formula = cell.formula.as_deref();
    let (t, v): (Option<&str>, Option<String>) = match &cell.value {
        Value::Empty => (None, None),
        Value::Number(n) => (None, Some(number_text(*n))),
        Value::Bool(b) => (Some("b"), Some(if *b { "1" } else { "0" }.into())),
        Value::Error(e) => (Some("e"), Some(e.as_str().into())),
        Value::Text(text) => {
            if formula.is_some() {
                (Some("str"), Some(doc_xml::ooxml::encode_xstring(text)))
            } else {
                (Some("s"), Some(sst.index(cell, text).to_string()))
            }
        }
    };
    if let Some(t) = t {
        out.push_str(" t=\"");
        out.push_str(t);
        out.push('"');
    }
    if formula.is_none() && v.is_none() {
        out.push_str("/>");
        return;
    }
    out.push('>');
    if let Some(f) = formula {
        if let Some(raw) = &f.raw_xml {
            out.push_str(raw);
        } else {
            out.push('<');
            out.push_str(f_tag);
            if let Some(a) = f.array {
                out.push_str(" t=\"array\" ref=\"");
                out.push_str(&a.to_a1());
                out.push('"');
            }
            out.push('>');
            let text = match &f.expr {
                Some(e) => crate::formula::unparse_ooxml(e),
                None => f.text.clone(),
            };
            escape_text(&text, out);
            out.push_str("</");
            out.push_str(f_tag);
            out.push('>');
        }
    }
    if let Some(v) = v {
        out.push('<');
        out.push_str(v_tag);
        out.push('>');
        escape_text(&v, out);
        out.push_str("</");
        out.push_str(v_tag);
        out.push('>');
    }
    out.push_str("</");
    out.push_str(c_tag);
    out.push('>');
}
