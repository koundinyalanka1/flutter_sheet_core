//! OpenDocument spreadsheets (`.ods`), as LibreOffice and WPS write them.
//!
//! Reading takes the tables, cell values and formulas (OpenFormula
//! translated to Excel syntax), styles, merged cells, column widths, row
//! heights and frozen panes. Saving regenerates only the tables that
//! changed inside `content.xml`; unchanged tables, every other part of the
//! file, and anything inside a table this engine does not model (shapes,
//! forms, conditional formats, validations, comments) are kept as written.

pub mod formula;
mod read;
pub mod styles;
mod template;
mod write;

pub use read::read;
pub use write::write;

/// What saving an ODS package back needs.
#[derive(Debug)]
pub struct Package {
    pub opc: doc_opc::Package,
    /// `content.xml` as read.
    pub content: String,
    /// The automatic styles element's span in `content`, if any.
    pub auto_styles: Option<(usize, usize)>,
    /// The font declarations element's span.
    pub font_decls: Option<(usize, usize)>,
    /// Where the tables are: from the first table's start to the last
    /// table's end (or an insertion point when there are none).
    pub tables_region: (usize, usize),
    /// Each table's span, in order.
    pub tables: Vec<(usize, usize)>,
    /// Font face names declared, to their family.
    pub fonts: std::collections::HashMap<String, String>,
}

/// What saving one table back needs.
#[derive(Clone, Debug, Default)]
pub struct TablePart {
    /// Which table of the original `content.xml` this is.
    pub original: Option<usize>,
    /// Its name and visibility as read.
    pub name: String,
    pub hidden: bool,
    /// The table element's attributes other than its name and style.
    pub attrs: Vec<(String, String)>,
    /// Elements before the columns (shapes, forms), as written.
    pub head: String,
    /// Elements after the rows (named expressions, conditional formats).
    pub tail: String,
}

/// Unmodelled parts of a cell kept for saving: its other attributes and
/// its annotation, as written.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct OdsCellExtra {
    pub attrs: Vec<(String, String)>,
    pub annotation: Option<std::sync::Arc<str>>,
}
