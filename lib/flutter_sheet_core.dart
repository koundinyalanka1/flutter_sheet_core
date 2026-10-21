/// flutter_sheet_core: DocHelper's spreadsheet engine, built from scratch in
/// Rust.
///
/// The engine reads and writes XLSX, XLSM, ODS, CSV and TSV files in place,
/// formats values the way Excel does, computes about 280 worksheet
/// functions, recalculates only what an edit affects, and keeps undo and
/// redo history.
library;

export 'src/sheet_core_api.dart';
