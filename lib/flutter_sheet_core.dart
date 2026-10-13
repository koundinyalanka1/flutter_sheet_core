/// flutter_sheet_core: DocHelper's spreadsheet engine, built from scratch in
/// Rust.
///
/// The engine evaluates formulas, formats values the way Excel does, and
/// reads and writes XLSX, ODS and CSV files. The Dart API grows as the
/// engine lands; for now it reports the native library and its version.
library;

export 'src/sheet_core_api.dart' show SheetCore, SheetCoreException;
