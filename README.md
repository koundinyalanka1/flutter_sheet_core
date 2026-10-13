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

Next: the evaluator and function library (`formula/eval.rs` and
`formula/functions/` are written in part and not yet built), recalculation,
XLSX/CSV/ODS reading and writing, the Rust FFI layer and the Dart API.

```bash
(cd rust && cargo test)
```
