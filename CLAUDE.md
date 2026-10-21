# flutter_sheet_core: notes for Claude Code sessions

The guide for this repository lives in `AGENTS.md`, so every coding agent
reads the same rules.

@AGENTS.md

## Claude Code only

- **The file-writing tool decodes `\uXXXX` escapes** in file content. To
  put a literal backslash-u sequence in Rust or Dart source (the Rust
  character escapes in `numfmt/parse.rs` and `pdf.rs`, JSON in tests),
  edit around it, or build it with `concat!` or an escaped string.
