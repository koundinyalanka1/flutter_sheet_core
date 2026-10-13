# flutter_sheet_core

DocHelper's spreadsheet engine (Excel, OpenDocument and CSV), written from
scratch in Rust. **In progress**: see phase 3 in the app's `PLAN.md`.

Done and tested in `rust/crates/sheet_core`:

- `refs`: A1 references and ranges.
- `value`: Excel's value coercion and comparison rules.
- `datetime`: 1900/1904 date serials.
- `numfmt`: Excel number formats, including Indian lakh/crore grouping.
- `input`: what typed text means (numbers, percentages, currency, dates,
  times).
- `formula`: parsing, printing and reference adjustment.

The Rust FFI layer (`rust/crates/sheet_ffi`) and the Dart API (`lib/`) now
carry the library version and its error plumbing, so the workbook surface
can grow there. Next: the evaluator and function library (`formula/eval.rs`
and `formula/functions/` are written in part and not yet built),
recalculation, and XLSX/CSV/ODS reading and writing.

```bash
(cd rust && cargo test)
```
