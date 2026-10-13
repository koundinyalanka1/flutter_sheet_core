//! A developer tool for trying real files.
//!
//! ```text
//! sheetcli dump  FILE                 print every sheet's cells
//! sheetcli set   FILE OUT CELL=INPUT  edit cells (Sheet!A1=text) and save
//! sheetcli conv  FILE OUT             open and save in OUT's format
//! sheetcli recalc FILE                recalculate everything, show differences
//! sheetcli pdf   FILE OUT              print to PDF
//! ```

use sheet_core::edit::Op;
use sheet_core::numfmt::DateOrder;
use sheet_core::{CellRef, Format, Workbook};

fn open(path: &str) -> Workbook {
    let bytes = std::fs::read(path).unwrap_or_else(|e| panic!("{path}: {e}"));
    let started = std::time::Instant::now();
    let wb = Workbook::open(bytes, path, DateOrder::Dmy).unwrap_or_else(|e| panic!("{path}: {e}"));
    eprintln!("opened in {:?}", started.elapsed());
    for w in &wb.warnings {
        eprintln!("warning: {w}");
    }
    wb
}

fn save(wb: &mut Workbook, out: &str) {
    let format = Format::from_name(out.rsplit('.').next().unwrap_or("xlsx")).unwrap_or(Format::Xlsx);
    let bytes = wb.to_bytes(format).unwrap_or_else(|e| panic!("{out}: {e}"));
    std::fs::write(out, &bytes).unwrap();
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match args.first().map(String::as_str) {
        Some("dump") => {
            let wb = open(&args[1]);
            for (i, s) in wb.sheets.iter().enumerate() {
                println!("== {i}: {} ({:?}) used {:?}", s.name, s.state, s.used_range().map(|r| r.to_a1()));
                for (&r, row) in &s.cells {
                    for (&c, cell) in row {
                        let at = CellRef::new(r, c);
                        let shown = sheet_core::display::shown_text(&wb, cell);
                        match &cell.formula {
                            Some(f) => println!("{} = {:?}  [={}]", at.to_a1(), shown, f.text),
                            None => println!("{} = {:?}", at.to_a1(), shown),
                        }
                    }
                }
            }
        }
        Some("pdf") => {
            let wb = open(&args[1]);
            let mut book = doc_font::FontBook::new();
            for dir in ["/usr/share/fonts", "/system/fonts", "/System/Library/Fonts", "/Library/Fonts"] {
                book.add_dir(std::path::Path::new(dir));
            }
            let bytes = sheet_core::pdf::export(&wb, &mut book, &sheet_core::pdf::PdfOptions::default());
            std::fs::write(&args[2], bytes).unwrap();
            println!("wrote {}", args[2]);
        }
        Some("set") => {
            let mut wb = open(&args[1]);
            for edit in &args[3..] {
                let (target, input) = edit.split_once('=').expect("CELL=INPUT");
                let (sheet, cell) = match target.split_once('!') {
                    Some((s, c)) => (wb.sheet_index(s).expect("sheet"), c),
                    None => (wb.active, target),
                };
                let at = CellRef::parse_a1(cell).expect("cell").0;
                wb.apply(Op::SetCell { sheet, at, input: input.into() }).unwrap();
            }
            save(&mut wb, &args[2]);
        }
        Some("conv") => {
            let mut wb = open(&args[1]);
            save(&mut wb, &args[2]);
        }
        Some("recalc") => {
            let mut wb = open(&args[1]);
            let before: Vec<_> = wb.sheets.iter().map(|s| s.cells.clone()).collect();
            let started = std::time::Instant::now();
            let changed = wb.recalc_all();
            eprintln!("recalculated in {:?}; {} values changed", started.elapsed(), changed.len());
            for (s, at) in changed.iter().take(50) {
                let old = before[*s].get(&at.row).and_then(|r| r.get(&at.col)).map(|c| c.value.clone());
                let new = wb.sheets[*s].value(*at);
                println!("{}!{}: {:?} -> {:?}  [={}]", wb.sheets[*s].name, at.to_a1(), old, new,
                    wb.sheets[*s].cell(*at).and_then(|c| c.formula.as_ref()).map(|f| f.text.as_str()).unwrap_or(""));
            }
        }
        _ => eprintln!("usage: sheetcli dump|set|conv|recalc ..."),
    }
}
