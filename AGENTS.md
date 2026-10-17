# flutter_sheet_core: notes for coding agents

This is the guide for every coding agent (Codex, Cursor, Claude and others)
working in this repository, on its own or as part of the DocHelper app.
`CLAUDE.md` imports it, so Claude Code reads the same rules. The app's own
guide is `AGENTS.md` at the root of the app repository; read it too when a
change reaches the app.

## What this is

DocHelper is a Flutter app, Android first, whose document engines are
written from scratch in Rust. It is growing from a PDF toolkit into an
all-in-one office app (word processor, spreadsheets, presentations, PDFs)
so the owner's father can do office work on a phone instead of a laptop.

This repository is DocHelper's **spreadsheet engine**.

- **Repository:** `github.com/koundinyalanka1/flutter_sheet_core`, branch
  `main`. It is a git submodule of the app (`koundinyalanka1/pdfhelper`)
  at `packages/flutter_sheet_core`. Its last commit is "Open flat
  OpenDocument spreadsheets (.fods) and save them as .ods".
- **Native library:** `libsheet_ffi` (Rust crate `sheet_ffi`). Every
  exported function starts with `sheet_`.
- **Dart package:** `flutter_sheet_core`, an FFI plugin. Its API is
  `SheetCore` (entry points) and `SheetBook` (an open workbook), plus the
  value classes listed under "The Dart API".
- **File kinds:**

  | Kind | Extensions | Read | Saved as |
  | --- | --- | --- | --- |
  | Excel workbook | `.xlsx .xlsm .xltx .xltm` | yes | itself, in place (macros kept, never run) |
  | OpenDocument spreadsheet | `.ods .ots` | yes | itself, in place |
  | CSV and TSV | `.csv .tsv` (the engine also takes `.txt` as CSV and `.tab` as TSV) | yes | itself (one sheet, values only) |
  | Excel 97-2003, WPS Spreadsheets | `.xls .xlt .et .ett` | yes (BIFF8) | a new `.xlsx`; the app says so |
  | Flat OpenDocument spreadsheet | `.fods` | yes | a new `.ods`; the app says so |

  The app sends it what `DocKind.sheet` lists in `lib/models/doc_kind.dart`.
  The engine tells formats apart by content first (a flat ODF document, an
  OLE2 compound file, a ZIP whose `mimetype` says spreadsheet), then by the
  extension of the `name` it is given.
- **What it does:** a sparse workbook model; formulas parsed, printed back
  in Excel, OOXML and OpenFormula syntax, and computed by 245 worksheet
  functions with Excel's coercion, error and 15-digit rules; recalculation
  of only what an edit affects; the full Excel number-format language with
  Indian lakh/crore grouping; every edit as an operation with undo and
  redo; in-place saving; printing to PDF.
- **In the app:** `lib/screens/sheet_editor_screen.dart` (the screen),
  `lib/widgets/sheet/sheet_grid.dart` (the custom-painted grid) and
  `lib/widgets/sheet/sheet_grid_data.dart` (cell windows fetched and cached
  48 rows by 16 columns at a time). Tests:
  `test/screens/sheet_editor_screen_test.dart`.

## The owner's rules

These apply here in full.

- **Everything is Rust, written from scratch**: the engine and its FFI
  layer. No C or C++ is written or vendored. `extern "C"` on exported Rust
  functions is only the calling convention `dart:ffi` needs.
- **Allowed dependencies** are only these small primitives:
  - `flate2` (raw deflate);
  - RustCrypto (`aes`, `cbc`, `sha2`, `md-5`) and `getrandom`;
  - `serde`/`serde_json`, only at the FFI boundary;
  - `thiserror`.

  The PDF engine also vendors standalone JPEG, JPEG 2000 and JBIG2
  codecs; nothing else does. Do not add document, ZIP, XML, spreadsheet,
  formula, CSV, encoding, font or Office crates. Data such as code-page
  tables and open-licensed fonts is fine. Today `sheet_core` depends on
  `thiserror` and the app's shared crates `doc_zip`, `doc_xml`,
  `doc_encoding`, `doc_opc`, `doc_odf`, `doc_cfb`, `doc_font`, `doc_pdf`
  and `doc_layout`; `sheet_ffi` adds `doc_ffi`, `serde` and `serde_json`.
- **One library per kind of file.** This engine is a self-contained
  Flutter FFI plugin with its own Rust workspace and native library. Code
  every engine needs lives in the Rust-only `packages/doc_common` of the
  app, not here.
- **Never damage a user's file.**
  - Edit documents in place: copy unchanged ZIP entries raw, and keep XML
    you do not understand.
  - Save atomically (`doc_ffi::write_file_atomically`: a temporary file,
    fsync, rename).
  - Formats that cannot be written back (XLS, WPS `.et`, flat `.fods`)
    are saved as their modern equivalent (`.xlsx`, `.ods`) in a new file,
    and the app says so.
- **Never change package identifiers.** Keep the Dart package name
  `flutter_sheet_core`, the Android plugin namespace and group
  `com.yourmateapps.flutter_sheet_core`, the podspec name, the library name
  `libsheet_ffi` and the `sheet_` symbol names. In the app, never change
  `com.yourmateapps.pdfhelper` (the Android `applicationId`, namespace and
  Kotlin package, the iOS bundle ID and its `.debug` suffix scheme) or the
  Dart package name `pdfhelper`: AdMob and Google Play are linked to them.
- **Commit or push only when the owner asks.**
  - Work on `main`. No feature branches.
  - A commit message is one plain imperative sentence saying what the
    change does for the user, without a type prefix (see `git log`: "Open
    Excel 97-2003 workbooks and save them as Excel workbooks").
  - Commit and push in this repository first, then commit the updated
    gitlink in the app.
- **AI features are deferred** in the app (`Features.ai` is false); none
  live in this engine.

## Layout

All Rust paths are under `rust/crates/`.

| Path | What |
| --- | --- |
| `rust/Cargo.toml` | The workspace: `sheet_core` and `sheet_ffi`; release profile with LTO, one codegen unit, `opt-level = 3`; tests at `opt-level = 1` |
| `sheet_core/src/lib.rs` | Module list; re-exports `Workbook`, `Sheet`, `Cell`, `Format`, `Imported`, `CellRef`, `Range`, `Value`, `CellError`, `SheetError` |
| `sheet_core/src/io.rs` | `detect_format`, `Workbook::open` (flat ODF, then CFB, then by format), `blank`, `to_bytes`, `saved` (re-reads the written bytes so later saves build on them) |
| `sheet_core/src/model.rs` | `Workbook`, `Sheet` (cells in `BTreeMap<row, BTreeMap<col, Cell>>`), `Cell`, `CellExtra`, `Formula`, `DefinedName`, `Source`, `Format`, `Imported` (`writes_back`, `save_format`) |
| `sheet_core/src/refs.rs` | `CellRef`/`Range` (0-based), A1 parsing, column letters, `quote_sheet_name`, `MAX_ROWS`/`MAX_COLS` |
| `sheet_core/src/value.rs` | `Value` (`Empty`/`Number`/`Text`/`Bool`/`Error`), `CellError`, coercions, `compare`, `parse_number`, `number_to_text`, `round15` |
| `sheet_core/src/datetime.rs` | 1900 serials (with Lotus's phantom 1900-02-29) and 1904 serials, DATE overflow rules |
| `sheet_core/src/numfmt/` | Format-code parser (`parse.rs`), formatter (`mod.rs`: `NumberFormat::cached`, `format_value`, `general`, `round_half_away`, `INDIAN_NUMBER_FORMAT`, `INDIAN_RUPEE_FORMAT`), `tests.rs` |
| `sheet_core/src/input.rs` | `parse_input`: what typed text means (formula, number with a suggested format, percentage, currency, date or time in the locale's order, boolean, error) |
| `sheet_core/src/style.rs` | `CellStyle`, `Font`, `Fill`, `Border`, `Align`, `ColorSpec`, `Theme` (tints), `StyleTable` (interned, append-only) |
| `sheet_core/src/display.rs` | `shown` (text, colour and kind a cell shows) and `edit_text` (what the formula bar edits) |
| `sheet_core/src/formula/` | `ast.rs`, `parser.rs` (`parse`), `unparse.rs` (`unparse`, `unparse_ooxml` with `_xlfn.`, `unparse_odf`), `adjust.rs` (shift, offset, move, rename and invalidate references), `eval.rs` (`Evaluator`, the `Grid` trait, implicit intersection, element-wise arrays) |
| `sheet_core/src/formula/functions/` | `mod.rs` (dispatch, `KNOWN`, `names()`, `arity`); `criteria`, `logic`, `math`, `stats`, `lookup`, `text`, `date`, `info`, `finance`; `tests.rs` pins results to Excel's |
| `sheet_core/src/calc.rs` | Recalculation: lazy reverse dependency index, Kahn's order, volatile cells, circular references; `recalc_from`, `recalc_all`, `calc_missing`, `evaluate_in` |
| `sheet_core/src/edit.rs` | `Op` (every edit), `apply`, `undo`, `redo`, `History`, `StylePatch`, `find`, `change_decimals` |
| `sheet_core/src/xlsx/` | `read.rs`, `write.rs` (in place), `styles.rs` (styles and theme), `template.rs` (a blank package) |
| `sheet_core/src/ods/` | `read.rs`, `write.rs` (in place), `formula.rs` (OpenFormula to Excel), `styles.rs` (automatic and data styles), `template.rs` |
| `sheet_core/src/csv.rs` | `CsvFormat`, `parse_records`, `detect_delimiter`, `read`, `write` |
| `sheet_core/src/xls/` | `mod.rs` (BIFF8 records, values, styles, merges, sizes, panes), `formula.rs` (formula decompiler), `ftab.rs` (BIFF function table), `tests.rs` |
| `sheet_core/src/pdf.rs` | `export` and `PdfOptions`: pages down then across, fit-to-width for slightly wide sheets, fills, borders, wrapping, merges, overflowing text, a footer |
| `sheet_core/src/tests.rs` | Workbook-level tests: typing and recalculation, rows and columns, sheets, fill/paste/sort, XLSX, CSV and ODS round trips, PDF, flat ODS |
| `sheet_core/examples/sheetcli.rs` | Developer tool: `dump`, `set`, `conv`, `recalc`, `pdf` |
| `sheet_ffi/src/lib.rs` | Every exported `sheet_*` function, the JSON shapes, and the FFI tests |
| `sheet_ffi/build.rs` | Links Android builds with 16 KB pages |
| `lib/flutter_sheet_core.dart` | The library; exports `src/sheet_core_api.dart` |
| `lib/src/bindings.dart` | `SheetBindings`: raw lookups, library loading, `SHEET_CORE_LIB_PATH` |
| `lib/src/sheet_core_api.dart` | `SheetCore`, `SheetBook` and the value classes |
| `test/sheet_core_test.dart` | 6 tests: errors, cell names, version, edit/recalc/undo/save, flat ODS, PDF |
| `android/` | The Android library project (no Java or Kotlin); bundles `src/main/jniLibs/<abi>/libsheet_ffi.so` |
| `macos/flutter_sheet_core.podspec` | Vendors `macos/Frameworks/libsheet_ffi.dylib` |

- There is no `ios/` folder: the plugin declares Android and macOS only.
  `sheet_ffi` also builds a `staticlib` for the iOS umbrella library the
  plan foresees.
- **Binaries are git-ignored** (`android/src/main/jniLibs/*/`,
  `macos/Frameworks/*.dylib`, `rust/target/`). Build them; never commit
  them. Whether to commit them or use Git LFS is an open owner decision.

## Building and testing

Every command below was run on the Linux cloud machine (4 CPUs, Rust
1.97, Flutter 3.47.7) and succeeded as shown. Start with:

```bash
export PATH=/opt/flutter/bin:$HOME/.cargo/bin:$PATH
```

- **It builds only inside the app checkout.** Both crates depend on
  `../../../../doc_common/crates/...` by relative path, which resolves to
  the app's `packages/doc_common`. A clone of this repository alone does
  not build: clone the app and run `git submodule update --init
  --recursive`. Rust 1.88 or newer is needed app-wide.

**Rust, from `packages/flutter_sheet_core/rust`:**

```bash
cargo build --release                  # target/release/libsheet_ffi.so
cargo test --release                   # sheet_core 73 passed, 1 ignored; sheet_ffi 3 passed
cargo clippy --release                 # builds; 28 warnings in sheet_core, 1 in sheet_ffi
SAMPLE_OUT=/tmp/sample.xlsx cargo test --release -p sheet_core write_sample -- --ignored
```

- With warm caches `cargo test --release` takes seconds; the tests
  themselves run in 0.2 s.
- **Clippy is not clean here.** `cargo clippy --release --all-targets`
  fails on four `approx_constant` errors (deny by default: `3.14159` in
  `numfmt/tests.rs`), so `-- -D warnings` fails too. The library warnings
  are style lints (`is_multiple_of`, complex types, range `contains`,
  large enum variants). Fix them only as part of other work in the same
  code; phase 10 does not ask for it here.
- The ignored test writes a sample budget workbook to `SAMPLE_OUT`, for
  opening in LibreOffice or Excel.

**`sheetcli`, from the same folder.** It opens with day-month-year dates,
prints the format, open time, save format and warnings on stderr, and
panics on any error (it is a developer tool). Its usage line omits `pdf`.

```bash
cargo run --release --example sheetcli -- dump FILE         # every sheet's cells, formulas in [=...]
./target/release/examples/sheetcli recalc FILE              # recalculate all; first 50 changes
./target/release/examples/sheetcli set FILE OUT 'Sheet1!A5=2' 'B2==A5*3'
./target/release/examples/sheetcli conv FILE OUT.xlsx       # format from OUT's extension
./target/release/examples/sheetcli pdf FILE OUT.pdf         # fonts from /usr/share/fonts and others
```

- `test_corpus/office` was empty on this machine (phase 10's B0 has not
  run), so there is no corpus sweep yet. LibreOffice ships a real
  spreadsheet, `/usr/lib/libreoffice/program/opencl/cl-test.ods`; with it:
  `dump` printed 171 lines (65 ms cold open, 1.6 ms warm), `recalc`
  changed 73 values, all in the last digits, `conv` to `.xlsx` and `.csv`,
  `set` and `pdf` (70 KB) all worked.
- A cell without `Sheet!` goes to the active sheet. `set` input is typed
  as a person would, so `B2==A5*3` sets the formula `=A5*3`.

**Checking saved files in LibreOffice** (`/usr/bin/soffice`, headless;
give it its own profile so parallel runs do not collide):

```bash
soffice -env:UserInstallation=file:///tmp/lo-profile --headless --convert-to csv --outdir /tmp/lo edited.xlsx
soffice -env:UserInstallation=file:///tmp/lo-profile --headless --convert-to xls --outdir /tmp/lo book.ods
```

The first opened an edited `.xlsx` with the engine's recalculated values;
the second made an `.xls` that `sheetcli dump` imported and `conv` saved
as `.xlsx`. Each conversion took about 3 s.

**Dart, from `packages/flutter_sheet_core`:**

```bash
SHEET_CORE_LIB_PATH=$PWD/rust/target/release/libsheet_ffi.so flutter test   # 6 passed
flutter analyze                                                            # No issues found
dart format --output=none --set-exit-if-changed lib test                   # 0 changed
```

Without `SHEET_CORE_LIB_PATH` the same run reports 2 passed and 4 skipped
and still says "All tests passed!": the tests skip themselves when
`SheetCore.isAvailable` is false. Always set it. The PDF test reads fonts
from `/usr/share/fonts`.

**Through the app's scripts, from the app root:**

```bash
bash scripts/build_engines.sh host --only sheet            # release build for this machine
bash scripts/build_engines.sh test --only sheet            # cargo test (debug): all of doc_common, then sheet
eval "$(bash scripts/build_engines.sh env --only sheet)"   # exports SHEET_CORE_LIB_PATH (and PDF_CORE_LIB_PATH)
flutter test test/screens/sheet_editor_screen_test.dart    # 4 passed
ANDROID_NDK_HOME=/home/user/android-ndk/android-ndk-r27c bash scripts/build_engines.sh android --only sheet
```

- On Linux, `host` also builds the PDF engine's host library, whatever
  `--only` says. `test` always runs every `doc_common` test first; with a
  cold debug build of this engine the run took 1 min 16 s here. `--debug`
  builds debug libraries instead. On a Mac, `host` puts a dylib in
  `macos/Frameworks/`, and `macos` builds a universal one.
- **Android.** `android` runs `cargo ndk -t arm64-v8a -t armeabi-v7a -t
  x86_64 -o android/src/main/jniLibs build -p sheet_ffi --release`. It
  needs `cargo-ndk`, the targets `aarch64-linux-android`,
  `armv7-linux-androideabi` and `x86_64-linux-android`, and an NDK (the
  script finds `$ANDROID_HOME/ndk/<latest>` when `ANDROID_NDK_HOME` is
  unset). On the cloud machine the NDK is
  `/home/user/android-ndk/android-ndk-r27c`; elsewhere fetch
  `dl.google.com/android/repository/android-ndk-r27c-linux.zip`. The
  libraries are 3.1 MB (arm64-v8a), 2.2 MB (armeabi-v7a) and 3.2 MB
  (x86_64).
- **16 KB pages.** `sheet_ffi/build.rs` passes `-z max-page-size=16384`
  and `-z common-page-size=16384` to Android links. Checked with
  `readelf -lW android/src/main/jniLibs/<abi>/libsheet_ffi.so`: every
  `LOAD` segment is aligned `0x4000` on all three ABIs. In an APK, the
  app's `python3 scripts/check_android_native.py <apk>` checks it.
- **The app's Android preflight** (`verifyEngines` in
  `android/app/build.gradle.kts`) refuses a build when a `jniLibs` binary
  is missing or older than the newest `.rs` or `Cargo.toml` under this
  package's `rust/crates` or anywhere under `doc_common/crates`. When cargo
  finds the build up to date, `cargo ndk` leaves the copied files' times
  alone, so after a change to a `doc_common` crate this engine does not
  use, the preflight can still refuse. Rebuild; if cargo reports nothing
  to do, `touch` the three libraries.

## The FFI layer

Shared conventions come from `doc_ffi` in `packages/doc_common`.

- Rust functions exported with `#[no_mangle] pub extern "C"`, all named
  `sheet_*`. There is no C code.
- Strings are UTF-8 and NUL-terminated. Returned strings belong to the
  caller and are freed with `sheet_free_string`. `sheet_last_error` and
  `sheet_core_version` return borrowed strings: never free them.
- Failures return -1 or NULL; `sheet_last_error` then gives
  `"CODE: message"` for this thread. Codes this engine raises:

  | Code | When |
  | --- | --- |
  | `IO` | the file cannot be read or written |
  | `INVALID_ARGUMENT` | a missing or non-UTF-8 argument, bad JSON (`bad request: ...`), no such sheet, or a refused edit (`EditError`, worded for the person: "There's a problem with this formula...") |
  | `UNSUPPORTED` | not a spreadsheet, Excel 95 or older, a non-spreadsheet ODF file, an unknown save format |
  | `ENCRYPTED` | password-protected XLSX (`EncryptedPackage`) or XLS (`FILEPASS`); there is no password argument |
  | `CORRUPT` | a damaged package or part |
  | `TOO_LARGE` | a file over 256 MB, or more than 20 million cells |
  | `BAD_HANDLE` | an unknown handle |
  | `PANIC` | a caught panic |
  | `ERROR` | JSON encoding failed |

  The shared set also has `WRONG_PASSWORD` and `NOT_FOUND`; this engine
  does not use them.
- Every call runs inside `call_i64` or `call_str`, which catch panics and
  turn them into `PANIC`. Allocation aborts and stack overflows cannot be
  caught; bound inputs instead (see "Limits").
- Open workbooks live in `static BOOKS: Registry<Workbook>`. Handles start
  at 1, are process-wide and usable from any isolate; each workbook has its
  own lock, so calls on one workbook serialize. Today `Registry::with`
  ignores a poisoned lock, so a panic mid-edit can leave a half-edited
  session that later calls keep using (phase 10, B1 item 4).
- Structured results are JSON (serde only in `sheet_ffi`). Rows, columns
  and sheets are 0-based integers; ranges are `[r0, c0, r1, c1]`
  inclusive; colours are `#rrggbb`. No text offsets cross this boundary.

**Exported functions:**

| Purpose | Functions |
| --- | --- |
| Library | `sheet_core_version`, `sheet_last_error`, `sheet_free_string`, `sheet_function_names_json` (for autocomplete), `sheet_format_preview(value, code, date_order)` (0 dmy, 1 mdy, 2 ymd; `{"text","color"}`) |
| Sessions | `sheet_open(path, name, options_json)` (`name` NULL uses `path`; it gives the extension when a content-URI copy has none; options `{"dateOrder":"dmy","utcOffsetMinutes":330}`), `sheet_new(options_json)`, `sheet_close`, `sheet_set_options` |
| Reading | `sheet_info_json` (format, `importedFrom`, `writesBack`, `saveFormat`, sheets with state, used range, freeze, grid, default sizes and `chart`; active, modified, canUndo/canRedo, warnings, circular, names, date1904), `sheet_cells_json(handle, sheet, r0, c0, r1, c1)` (`{"cells": [[row, col, text, kind, style, color, formula]], "merges", "cols", "rows"}`; the window is clamped to the sheet), `sheet_layout_json` (a whole sheet's widths, heights and merges), `sheet_styles_json(handle, ids_json)`, `sheet_cell_json` (the formula bar: `edit`, `text`, `kind`, `style`, `formula`, `array`, `numFmt`) |
| Editing | `sheet_apply(handle, op_json)`, `sheet_undo`, `sheet_redo` (`null` when nothing to do), `sheet_recalc` (everything); each returns `{"changed": [[sheet,row,col]], "all", "structure", "circular", "canUndo", "canRedo"}`; past 5,000 changed cells `changed` is empty and `all` is true |
| Queries | `sheet_find_json(handle, query, flags, sheet, limit)` (flags 1 match case, 2 whole cell, 4 in formulas; sheet -1 for all; limit clamped to 1..100,000), `sheet_evaluate_json(handle, sheet, row, col, formula)` (a formula without `=`, evaluated in place without storing it: the status bar's sums) |
| Saving | `sheet_save(handle, path, format)` (atomic; later saves build on the written file; `{"warnings"}`), `sheet_write_copy` (atomic; the workbook keeps its own format and state), `sheet_export_pdf(handle, path, options_json)` (`{"fontDirs", "sheets", "gridlines", "title", "page": [w, h]}`; returns `{"pages"}`) |

`sheet_apply` operations (`"op"` is the tag, fields camelCase): `set`,
`setBlock`, `clear` (`what`: `all`, `contents`, `formats`), `format`
(`patch`: bold, italic, underline, strike, font, size, color, fill,
hAlign, vAlign, wrap, indent, numFmt, border, decimals), `insertRows`,
`deleteRows`, `insertCols`, `deleteCols`, `colWidth`, `rowHeight`,
`hideRows`, `hideCols`, `merge`, `unmerge`, `freeze` (row and col 0
unfreeze), `addSheet`, `renameSheet`, `deleteSheet`, `moveSheet`,
`duplicateSheet`, `hideSheet`, `setActive`, `fill`, `paste` (`what`:
`all`, `values`, `formats`, `formulas`; `cut`), `sort`, `replace`,
`defineName`, `deleteName`. The full list with fields is `OpJson` in
`sheet_ffi/src/lib.rs`.

## The Dart API

- `SheetBindings` loads `SHEET_CORE_LIB_PATH` when set; otherwise
  `libsheet_ffi.so` on Android and Linux, the process and then
  `libsheet_ffi.dylib` on macOS and iOS, `sheet_ffi.dll` on Windows. One
  instance per isolate. Every function is a required lookup: a library
  missing one fails to load.
- `SheetCore`: `isAvailable`, `version`, `open(path, name:, dateOrder:,
  utcOffsetMinutes:)`, `create(...)`, `functionNames()`,
  `formatPreview(value, code)`.
- `SheetBook`: `info()`, `window(sheet, r0, c0, r1, c1)`, `layout(sheet)`,
  `styles(ids)`, `cell(...)`, `apply(op)`, `applyAsync(op)`, `setCell(...)`,
  `undo()`, `redo()`, `recalculate()`, `save(path, format)`,
  `writeCopy(...)`, `exportPdf(...)`, `find(...)`, `evaluate(...)`,
  `setOptions(...)`, `close()` (safe twice; a closed book throws
  `StateError`).
- Value classes: `WorkbookInfo`, `SheetInfo`, `SheetName`, `CellWindow`,
  `GridCell`, `SheetLayout`, `CellStyleInfo`, `CellEdge`, `CellDetail`,
  `SheetEditResult`, `CellRange`, the enums `SheetFormat` (with
  `forName`) and `SheetDateOrder`, and `columnName`/`cellName`.
- **Heavy calls run in `Isolate.run`:** `SheetCore.open`, `applyAsync`,
  `save`, `writeCopy` and `exportPdf`. Everything else runs on the calling
  isolate, including `apply`, `undo`, `redo`, `recalculate`, `find` and
  `layout`. The screen uses `applyAsync` for inserting and deleting rows
  and columns, pasting and sorting, and `apply` for typing and
  formatting; phase 10 moves the rest.
- A synchronous call on a workbook that an `Isolate.run` call is using
  waits for that workbook's lock, so it blocks the UI isolate until the
  background call finishes.
- Errors throw `SheetCoreException(code, message)`, parsed from
  `"CODE: message"`. For `INVALID_ARGUMENT` from an edit, `message` is a
  sentence to show the person.

## Engine conventions and pitfalls

**The model.**

- Cells are sparse (`BTreeMap` of rows of columns). A cell holds a
  `Value`, an optional `Formula` (text in Excel's English, comma-separated
  syntax, the parsed `Expr`, an array range, or raw XML), a style id and
  rarely used extras (rich text, CSV source text, array membership, ODS
  attributes).
- Style ids are XLSX `cellXfs` indexes. The style table is only appended
  to: existing ids are never renumbered, so untouched cells keep their
  look byte for byte.
- `Sheet.modified` decides whether a sheet's part is written again;
  otherwise it is copied as it was. `Workbook.dirty` is "unsaved edits".

**Round trip.**

- **XLSX.** Saving copies every unchanged ZIP entry raw and keeps entry
  order. A changed sheet's `sheetData` is regenerated; the rest of the
  worksheet is kept as a tree and written back. New styles and shared
  strings are appended (rich strings re-emitted from their raw XML);
  `calcChain.xml` is dropped with its relationship and override; after a
  recalculation `workbook.xml` gets `<calcPr fullCalcOnLoad="1"/>` so
  Excel recomputes. Seen here: an unedited save of a DocHelper-written
  `.xlsx` was byte-identical, and after one cell edit only
  `xl/workbook.xml` and the sheet changed.
- Regenerated parts are equivalent XML, not byte-identical; the owner's
  default (phase 10 decision 1) is to reword PLAN §3.3 that way and keep
  the attributes not modelled.
- Kept but not modelled: charts, pivots, drawings, tables, conditional
  formats, validations, hyperlinks, comments, macros (`.xlsm`, never run),
  what-if data tables (`t="dataTable"`, as raw XML) and chart sheets
  (`chart: true`, kept but not shown, with a warning).
- Lost today: a shared formula whose anchor does not parse leaves its
  other cells as values, with a warning; `cm`, `vm` and `ca` cell
  attributes and the `_xlws.`, `SINGLE` and `ANCHORARRAY` spellings of
  dynamic-array formulas are not kept (B6 items 11 and 12).
- **ODS.** Only changed tables are regenerated inside `content.xml`;
  unchanged tables and every other part are kept as written, and so are a
  table's shapes, forms, named expressions and conditional formats and a
  cell's validations and comments. `mimetype` stays first and
  uncompressed. A changed table's formulas are written back from the
  parsed formula (`unparse_odf`), not from the original text.
- OpenFormula is read from `of:`, `oooc:` and `msoxl:` formulas; the
  function prefixes `COM.MICROSOFT.`, `ORG.OPENOFFICE.` and `_XLFN.` are
  dropped. `LEGACY.` names are not mapped: LibreOffice's own
  `cl-test.ods` has `LEGACY.NORMSDIST`, which stays in the formula text,
  is written that way into an `.xlsx`, and keeps its saved value
  (NORMSDIST is not in the library).
- **CSV and TSV.** The delimiter (`,` `;` tab `|`, or Excel's `sep=`
  line), encoding (`doc_encoding`), byte-order mark, line ending and
  trailing newline are kept. Each field's exact text is kept until its
  cell is edited, so `007` and `1.50` save as they were. Saving writes the
  active sheet only, with formulas as values, and returns warnings saying
  so. A new CSV uses CRLF.
- **XLS and WPS `.et`** (BIFF8 in an OLE2 file through `doc_cfb`):
  values, shared strings, formulas decompiled to text (shared and array
  formulas, 3-D references, names, add-in functions), number formats,
  fonts, fills, borders, merges, sizes and frozen panes. Chart and macro
  sheets are left out, and formulas using features the decompiler cannot
  read keep their last values; both with warnings. The workbook reports
  `importedFrom: "xls"`, `writesBack: false`, `saveFormat: "xlsx"`, and
  its first warning says it will be saved as `.xlsx`.
- **Flat `.fods`**: turned into a package by `doc_odf::unflatten` and
  read as ODS; `importedFrom: "fods"`, saved as `.ods` to a new file.
- `sheet_save` writes the file, then `Workbook::saved` re-reads the bytes
  (XLSX and ODS) so later saves build on them. If that re-read fails, the
  call reports a failure although the file was already replaced (B6 item
  13). Nothing yet reopens the bytes to verify them before the rename.

**Formulas and recalculation.**

- **Never recalculate on open.** The values the file was saved with are
  shown; only formulas without a saved value are computed
  (`calc_missing`).
- After an edit, only the transitive dependents of the changed cells are
  computed, in dependency order, plus volatile cells (NOW, TODAY, RAND,
  RANDBETWEEN, OFFSET, INDIRECT). Structural edits drop the dependency
  index; it is rebuilt lazily.
- A formula that uses something the engine cannot compute (an unknown
  function, a structured table reference) keeps its saved value. A formula
  that does not parse keeps its text and value and is never recalculated.
- Circular references keep their values and are reported (`circular` in
  `info` and in every edit result).
- Excel's rules are matched on purpose: 15 significant digits,
  `ROUND(2.675, 2)` is 2.68, `=0.3-0.2-0.1` is -2.78E-17 (LibreOffice
  saves 0), the phantom 1900-02-29, the 1904 system, implicit intersection
  (`=$A:$A*2` is this row's A doubled) outside array formulas and
  aggregates.
- Not supported: dynamic-array spilling (an array result shows its
  top-left value), and table references. Text functions count Unicode
  characters, where Excel counts UTF-16 units; they differ only for emoji
  and other characters outside the Basic Multilingual Plane.
- OOXML stores newer functions with `_xlfn.`; `unparse_ooxml` adds it from
  `XLFN_FUNCTIONS` in `formula/unparse.rs`. Without it Excel shows
  `#NAME?`.
- `dateOrder` (default `dmy`, as in India) decides how typed dates and
  dates in criteria are read; `utcOffsetMinutes` sets NOW and TODAY.

**Limits.**

| Limit | Value | Where |
| --- | --- | --- |
| File opened | 256 MB, else `TOO_LARGE` | `MAX_FILE_BYTES`, `sheet_ffi` |
| Rows, columns | 1,048,576, 16,384 | `refs::MAX_ROWS`, `MAX_COLS` |
| Cells read from XLSX or ODS | 20,000,000, else `TOO_LARGE` (array members do not count; XLS and CSV have no cap) | `MAX_CELLS`, `xlsx/read.rs`, `ods/read.rs` |
| Cells one array formula marks | 1,000,000 | `xlsx/read.rs` |
| Formula nesting when parsing | 200 brackets and calls (operator chains are not counted) | `parser::MAX_DEPTH` |
| Evaluation recursion (names, nested INDIRECT) | 64 | `eval::MAX_DEPTH` |
| Cells computed on demand inside one computation | 48 | `calc::MAX_NESTING` |
| Text some text functions build | 32,767 characters (`&` does not check) | `MAX_TEXT`, `functions/text.rs` |
| Undo | 200 steps, 4,000,000 cells (a structural edit counts every cell twice) | `UNDO_STEPS`, `UNDO_BUDGET`, `edit.rs` |
| Changed cells listed per edit | 5,000, then `all` | `CHANGED_LIMIT`, `sheet_ffi` |
| Find results | 1 to 100,000 | `sheet_find_json` |
| PDF pages | 1,000 | `pdf::MAX_PAGES` |
| Date serial | 2,958,465 (31/12/9999) | `datetime::MAX_DATE_SERIAL` |

These limits do not yet stop the crashes phase 10 found: operator
chains, nested evaluation and range broadcasting can overflow the stack
or abort on allocation, and 20 million cells is far past a phone's memory
(see the next section).

**Changing the engine.**

- *A worksheet function:* implement it in its category module's `call`,
  add the name to `functions::KNOWN` (autocomplete and `is_known` use it),
  add it to `XLFN_FUNCTIONS` if Excel stores it with `_xlfn.`, and pin
  Excel's results in `functions/tests.rs`, quirks included.
- *An edit:* add the `Op` variant and its handling (and undo) in
  `edit.rs`, add the matching `OpJson` variant in `sheet_ffi` and list it
  in `sheet_apply`'s doc comment, and test it in `sheet_core/src/tests.rs`
  and through `sheet_apply`. `SheetBook.apply` takes a map, so Dart needs
  no change.
- *An export:* add it to `sheet_ffi/src/lib.rs` through `call_i64` or
  `call_str` with a Rust test; add its typedef and lookup in
  `lib/src/bindings.dart` and a method in `lib/src/sheet_core_api.dart`
  (through `Isolate.run` when it can be slow); add a Dart test; rebuild the
  host and Android libraries. A new required lookup makes older libraries
  fail to load.
- Changes to a reader or writer need a round-trip test: open, save,
  reopen and compare, and untouched entries still raw.

**Style.** There is no `rustfmt.toml`, and the code is hand-formatted to
about 120 columns: `cargo fmt --check` reports some 670 differences. Do
not run `cargo fmt` over the workspace; match the surrounding style.
`dart format` the Dart files you change.

**Tooling on the owner's Mac.** Cargo lives in `~/.cargo/bin`, which is
not on `PATH`; zsh does not word-split unquoted variables (use `xargs`);
BSD `sed` has no `\b`. Microsoft Excel is installed there for checking
saved files. Device checks go on the emulator `pdfhelper_api36_16kb`, not
his phones.

## Where work stands and what is next

- Phase 3 built the engine and the spreadsheet screen, phase 6 added PDF
  printing, phase 7 XLS and WPS import, phase 9 flat `.fods`. The engine
  is committed and pushed. The screen has not run on a device or emulator
  since it was written.
- Phase 10 (hardening and release) is planned in the app's
  `docs/PHASE10_PLAN.md`. This engine is **B6, Sheet** (wave 2, size L,
  four agents, not started), which also covers `sheet_editor_screen.dart`
  and `lib/widgets/sheet/`:
  - **A, formulas and recalculation:** fold operator chains (an
    8,000-term formula aborts at open), cap formulas at 8,192 characters
    and make `Drop`, `unparse` and `walk` iterative; replace the recursion
    through `Grid::value` with a work stack; a 4M-element budget with
    `checked_mul` for broadcasting (one formula asks for 393 GB); defer
    cells past the nesting limit and report Kahn's leftovers as circular;
    a calculation deadline and cancel flag.
  - **B, functions:** magnitude guards on FACTDOUBLE, PERMUT, COMBIN,
    COMBINA, DDB, CUMIPMT, CUMPRINC and any loop sized by a user number;
    the `IF` budget; length checks before SUBSTITUTE and other text
    builders; exact comparison first and reused key columns (5,000
    VLOOKUPs take 18 s).
  - **C, readers and writers:** array members count toward the cell
    budget; a phone budget for cells and memory passed from Dart; caps on
    shared strings, ODS repeats checked before expanding, CSV streamed,
    `<col>` stored as ranges; unparseable shared formulas kept as raw XML;
    keep `cm`/`vm`/`ca`, `_xlws.`, `SINGLE`/`ANCHORARRAY` and unchanged
    ODS formula text; saves verified through `save_verified` and failures
    reported only when nothing was written; clamped XLS and ODS spans.
  - **D, editing and the screen:** snapshots of only the touched sheets,
    insert and delete as inverse operations, undo budgeted by bytes;
    every apply, undo, redo and recalculation through `applyAsync` with a
    busy overlay; evict far grid blocks; `logError` in every catch.
  - **Verify:** `cargo test --release`; a hostile suite on 1 MiB and
    512 KiB threads, each case under 2 s and 300 MB; 5,000 VLOOKUPs
    recalculated in under 1 s; `sheetcli` over the corpus; `sharedext`,
    `dyn` and `odsf` round-tripped through headless LibreOffice; screen
    tests that fail on any synchronous call over 16 ms on a large fixture.
- Other batches reach this engine. B1 changes the shared crates under it:
  zip-bomb and per-part budgets in `doc_zip` and `doc_opc`, an XML node
  budget and lower depth in `doc_xml`, a 16 MB-stack backstop around every
  FFI call, `save_verified` with a backup, and poisoned sessions refusing
  edits. B2 bounds `doc_cfb` reservations (XLS). B9 adds a sheet sweep
  (comparing recomputed values with cached ones) and XLSX, ODS and XLS
  mutation fuzzers, run again with `RUST_MIN_STACK=524288`. B10 adds draft
  autosave and accessibility to the screen.
- Owner decisions that touch it: phone budgets (2-4M cells by device RAM,
  64 MB per XML part), equivalent rather than byte-identical XML for
  edited sheets, backups, and keeping armeabi-v7a (512 KB isolate stacks).
- After changing a `doc_common` crate this engine uses, run `bash
  scripts/build_engines.sh test` and `host` for all engines, then the app's
  `flutter test` with the env exports.
