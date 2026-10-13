//! The spreadsheet engine: cells, formulas, number formats, and reading and
//! writing XLSX, ODS and CSV files, written from scratch.

pub mod calc;
pub mod csv;
pub mod datetime;
pub mod display;
pub mod edit;
pub mod error;
pub mod formula;
pub mod input;
pub mod model;
pub mod numfmt;
pub mod pdf;
pub mod ods;
pub mod refs;
pub mod style;
pub mod value;
pub mod xls;
pub mod xlsx;

mod io;

pub use error::SheetError;
pub use model::{Cell, Format, Sheet, Workbook};
pub use refs::{CellRef, Range};
pub use value::{CellError, Value};

#[cfg(test)]
mod tests;
