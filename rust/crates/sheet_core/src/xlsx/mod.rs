//! XLSX (SpreadsheetML) reading and in-place writing.
//!
//! Reading keeps what saving needs: the package, the workbook and styles
//! parts as trees, the shared strings as read, and each worksheet as a tree
//! without its cell data. Saving copies every part that did not change byte
//! for byte, regenerates the cell data of changed sheets only, appends new
//! styles and strings without renumbering existing ones, and drops the
//! calculation chain so Excel rebuilds it.

mod read;
mod styles;
mod template;
mod write;

use std::sync::Arc;

use doc_xml::{Document, Element, Node};

pub use read::read;
pub use write::write;

use crate::formula::adjust::{shift_range, Shift};
use crate::refs::{CellRef, Range};
use crate::style::{Border, Fill, Font};

/// A shared string as the file had it.
#[derive(Clone, Debug, PartialEq)]
pub enum SstEntry {
    Plain(Arc<str>),
    /// Rich text: the raw XML inside `<si>`, and its plain text.
    Rich { xml: Arc<str>, text: Arc<str> },
}

impl SstEntry {
    pub fn text(&self) -> &Arc<str> {
        match self {
            SstEntry::Plain(t) => t,
            SstEntry::Rich { text, .. } => text,
        }
    }
}

/// An XLSX package and the parsed parts saving needs.
#[derive(Debug)]
pub struct Package {
    pub opc: doc_opc::Package,
    pub workbook_part: String,
    pub workbook: Document,
    pub workbook_rels: doc_opc::Rels,
    pub styles_part: Option<String>,
    pub styles: Option<Document>,
    /// `cellXfs` entries in the styles part.
    pub xf_count: usize,
    /// Fonts, fills and borders as listed, to reuse them for new styles.
    pub fonts: Vec<Font>,
    pub fills: Vec<Fill>,
    pub borders: Vec<Border>,
    /// Custom number formats: (id, code).
    pub num_fmts: Vec<(u32, String)>,
    pub sst_part: Option<String>,
    pub sst: Vec<SstEntry>,
    /// The `<sst>` start tag's name and attributes.
    pub sst_root: Option<(String, Vec<(String, String)>)>,
    /// The workbook's content type when read (xlsx, xlsm, template...).
    pub main_content_type: String,
    /// The active sheet as read.
    pub original_active: usize,
}

/// What saving a worksheet back in place needs.
#[derive(Clone, Debug)]
pub struct SheetPart {
    /// The part, e.g. `xl/worksheets/sheet1.xml`.
    pub path: String,
    pub rel_id: String,
    pub sheet_id: u32,
    /// Attributes of the workbook's `<sheet>` element besides name,
    /// sheetId, state and r:id.
    pub sheet_attrs: Vec<(String, String)>,
    /// The worksheet without its cells: `<sheetData>` is an empty
    /// placeholder.
    pub doc: Option<Document>,
    /// A chart sheet (kept as it is; it has no cells).
    pub chart: bool,
    /// Frozen panes as read, to know whether they changed.
    pub original_freeze: Option<CellRef>,
    /// Tables on this sheet: (part, range).
    pub tables: Vec<(String, Range)>,
    /// Rows and columns inserted or deleted since reading, applied to the
    /// tables' parts on saving.
    pub shifts: Vec<Shift>,
    /// A copy of another sheet: written as a new part.
    pub detached: bool,
}

impl SheetPart {
    /// Make this a new part (for a duplicated sheet): forget the original
    /// part and anything that points at other parts.
    pub fn detach(&mut self) {
        self.detached = true;
        self.tables.clear();
        self.shifts.clear();
        if let Some(doc) = &mut self.doc {
            strip_relationship_refs(&mut doc.root);
            doc.root.children.retain(|n| {
                !n.as_element()
                    .is_some_and(|e| matches!(e.local(), "tableParts" | "drawing" | "legacyDrawing" | "legacyDrawingHF" | "picture" | "oleObjects" | "controls"))
            });
        }
    }

    /// Follow rows or columns being inserted or deleted: move the ranges
    /// the worksheet names (conditional formats, validations, hyperlinks,
    /// filters).
    pub fn shift(&mut self, shift: Shift) {
        self.shifts.push(shift);
        for (_, range) in &mut self.tables {
            if let Some(r) = shift_range(*range, shift) {
                *range = r;
            }
        }
        if let Some(doc) = &mut self.doc {
            shift_element(&mut doc.root, shift);
        }
    }
}

fn strip_relationship_refs(e: &mut Element) {
    e.children.retain(|n| match n {
        Node::Element(c) => !(c.local() == "hyperlink" && c.attrs.iter().any(|a| a.ns == doc_xml::ns::R)),
        _ => true,
    });
    for c in e.elements_mut() {
        strip_relationship_refs(c);
    }
}

/// Shift `sqref`/`ref` attributes below an element; drop elements whose
/// ranges were deleted entirely.
fn shift_element(e: &mut Element, shift: Shift) {
    let mut i = 0;
    while i < e.children.len() {
        let mut remove = false;
        if let Node::Element(c) = &mut e.children[i] {
            match c.local() {
                // Selections would point at moved cells; Excel recreates them.
                "selection" => remove = true,
                "dimension" | "mergeCells" => {}
                _ => {
                    for attr in ["sqref", "ref"] {
                        if let Some(v) = c.attr(doc_xml::ns::NONE, attr).map(str::to_owned) {
                            let shifted: Vec<String> = v
                                .split_whitespace()
                                .filter_map(|part| Range::parse(part).and_then(|r| shift_range(r, shift)).map(|r| r.to_a1()))
                                .collect();
                            if shifted.is_empty() {
                                remove = true;
                            } else {
                                c.set_attr(attr, doc_xml::ns::NONE, shifted.join(" "));
                            }
                        }
                    }
                    if !remove {
                        shift_element(c, shift);
                    }
                }
            }
        }
        if remove {
            e.children.remove(i);
        } else {
            i += 1;
        }
    }
}

/// The order of a worksheet's child elements (CT_Worksheet).
const WORKSHEET_ORDER: &[&str] = &[
    "sheetPr",
    "dimension",
    "sheetViews",
    "sheetFormatPr",
    "cols",
    "sheetData",
    "sheetCalcPr",
    "sheetProtection",
    "protectedRanges",
    "scenarios",
    "autoFilter",
    "sortState",
    "dataConsolidate",
    "customSheetViews",
    "mergeCells",
    "phoneticPr",
    "conditionalFormatting",
    "dataValidations",
    "hyperlinks",
    "printOptions",
    "pageMargins",
    "pageSetup",
    "headerFooter",
    "rowBreaks",
    "colBreaks",
    "customProperties",
    "cellWatches",
    "ignoredErrors",
    "smartTags",
    "drawing",
    "legacyDrawing",
    "legacyDrawingHF",
    "picture",
    "oleObjects",
    "controls",
    "webPublishItems",
    "tableParts",
    "extLst",
];

/// The order of a stylesheet's child elements.
const STYLES_ORDER: &[&str] = &[
    "numFmts",
    "fonts",
    "fills",
    "borders",
    "cellStyleXfs",
    "cellXfs",
    "cellStyles",
    "dxfs",
    "tableStyles",
    "colors",
    "extLst",
];

/// Insert `child` among `parent`'s children where `order` says it goes.
fn insert_ordered(parent: &mut Element, child: Element, order: &[&str]) -> usize {
    let rank = |name: &str| order.iter().position(|o| *o == name).unwrap_or(order.len());
    let my = rank(child.local());
    let at = parent
        .children
        .iter()
        .position(|n| n.as_element().is_some_and(|e| rank(e.local()) > my))
        .unwrap_or(parent.children.len());
    parent.children.insert(at, Node::Element(child));
    at
}

/// A child element by local name, created in order if missing.
fn child_or_insert<'e>(parent: &'e mut Element, local: &str, order: &[&str], ns: doc_xml::Ns, prefix: &str) -> &'e mut Element {
    let idx = parent.children.iter().position(|n| n.as_element().is_some_and(|e| e.local() == local));
    let idx = match idx {
        Some(i) => i,
        None => {
            let name = if prefix.is_empty() { local.to_owned() } else { format!("{prefix}:{local}") };
            insert_ordered(parent, Element::new(name, ns), order)
        }
    };
    parent.children[idx].as_element_mut().expect("just found or inserted an element")
}

/// The qualified name for a new element in a document whose root binds the
/// SpreadsheetML namespace with `prefix`.
fn qname(prefix: &str, local: &str) -> String {
    if prefix.is_empty() {
        local.to_owned()
    } else {
        format!("{prefix}:{local}")
    }
}

/// Content types.
pub(crate) mod ct {
    pub const WORKBOOK: &str = "application/vnd.openxmlformats-officedocument.spreadsheetml.sheet.main+xml";
    pub const WORKBOOK_MACRO: &str = "application/vnd.ms-excel.sheet.macroEnabled.main+xml";
    pub const WORKSHEET: &str = "application/vnd.openxmlformats-officedocument.spreadsheetml.worksheet+xml";
    pub const SHARED_STRINGS: &str = "application/vnd.openxmlformats-officedocument.spreadsheetml.sharedStrings+xml";
    pub const STYLES: &str = "application/vnd.openxmlformats-officedocument.spreadsheetml.styles+xml";
}
