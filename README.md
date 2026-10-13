# flutter_sheet_core

DocHelper's spreadsheet engine (Excel, OpenDocument and CSV), written from
scratch in Rust, with a Dart API over `dart:ffi`.

`rust/crates/sheet_core`:

| Module | What it does |
| --- | --- |
| `refs`, `value`, `datetime` | A1 references; Excel's coercion, comparison and 15-digit rules; 1900/1904 date serials |
| `numfmt` | The full Excel number-format language, including Indian lakh/crore grouping |
| `input` | What typed text means (formulas, numbers, percentages, currency, dates and times in the locale's order) |
| `formula` | Parser, printer (Excel, OOXML `_xlfn.` and OpenFormula forms), reference adjustment, the evaluator, and about 280 worksheet functions |
| `model`, `style` | Workbooks, sheets of sparse cells, styles that are only ever appended |
| `calc` | Dependency-driven recalculation: only what an edit affects, plus volatile formulas; circular references kept and reported |
| `edit` | Every edit as an `Op` (typing, formatting, rows and columns, sheets, fill, paste, sort, replace, names) with undo and redo |
| `xlsx` | XLSX/XLSM read and in-place write: unchanged parts copied byte for byte, only changed sheets regenerated, styles and shared strings appended, `calcChain.xml` dropped |
| `ods` | ODS read and in-place write: unchanged tables kept verbatim, OpenFormula translated both ways, comments, validations and conditional formats kept |
| `csv` | CSV/TSV with encoding, delimiter and `sep=` detection; each field's text kept until edited |
| `pdf` | Printing to PDF: pages down then across, fit-to-width for slightly wide sheets, fills, borders, alignment, wrapping, merged cells, overflowing text |

`rust/crates/sheet_ffi` exports the engine to Dart (`sheet_open`,
`sheet_cells_json`, `sheet_apply`, `sheet_save`, ...). `lib/` holds the Dart
API: `SheetCore.open/create` return a `SheetBook`.

```bash
(cd rust && cargo test)
cargo run --release --example sheetcli -- dump book.xlsx   # from rust/
```
