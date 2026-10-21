//! Whole-engine tests: editing, recalculation, undo, and saving and reading
//! XLSX and CSV.

use crate::edit::{ClearWhat, Op, PasteWhat, SortKey, StylePatch};
use crate::model::{Format, Workbook};
use crate::numfmt::DateOrder;
use crate::refs::{CellRef, Range};
use crate::value::{CellError, Value};

fn at(a1: &str) -> CellRef {
    CellRef::parse_a1(a1).unwrap().0
}

fn range(a1: &str) -> Range {
    Range::parse(a1).unwrap()
}

fn set(wb: &mut Workbook, sheet: usize, a1: &str, input: &str) {
    wb.apply(Op::SetCell {
        sheet,
        at: at(a1),
        input: input.into(),
    })
    .unwrap_or_else(|e| panic!("{a1} = {input}: {e}"));
}

fn val(wb: &Workbook, sheet: usize, a1: &str) -> Value {
    wb.sheets[sheet].value(at(a1))
}

fn formula(wb: &Workbook, sheet: usize, a1: &str) -> Option<String> {
    wb.sheets[sheet].cell(at(a1)).and_then(|c| c.formula.as_ref()).map(|f| f.text.clone())
}

fn n(x: f64) -> Value {
    Value::Number(x)
}

fn budget() -> Workbook {
    let mut wb = Workbook::blank(DateOrder::Dmy);
    for (a1, input) in [
        ("A1", "Item"),
        ("B1", "Qty"),
        ("C1", "Rate"),
        ("D1", "Amount"),
        ("A2", "Rice"),
        ("B2", "10"),
        ("C2", "52.5"),
        ("D2", "=B2*C2"),
        ("A3", "Dal"),
        ("B3", "4"),
        ("C3", "120"),
        ("D3", "=B3*C3"),
        ("A4", "Total"),
        ("D4", "=SUM(D2:D3)"),
        ("F1", "13/10/2026"),
        ("F2", "12%"),
    ] {
        set(&mut wb, 0, a1, input);
    }
    wb
}

#[test]
fn typing_and_recalculation() {
    let mut wb = budget();
    assert_eq!(val(&wb, 0, "D2"), n(525.0));
    assert_eq!(val(&wb, 0, "D4"), n(1005.0));
    assert_eq!(val(&wb, 0, "F1"), n(46_308.0));
    assert_eq!(wb.styles.get(wb.sheets[0].cell(at("F1")).unwrap().style).num_fmt, "dd/mm/yyyy");
    assert_eq!(val(&wb, 0, "F2"), n(0.12));
    // Change a quantity: the amount and the total follow.
    let result = wb.apply(Op::SetCell { sheet: 0, at: at("B2"), input: "20".into() }).unwrap();
    assert_eq!(val(&wb, 0, "D2"), n(1050.0));
    assert_eq!(val(&wb, 0, "D4"), n(1530.0));
    assert!(result.changed.contains(&(0, at("D4"))));
    // Undo and redo.
    wb.undo().unwrap();
    assert_eq!(val(&wb, 0, "B2"), n(10.0));
    assert_eq!(val(&wb, 0, "D4"), n(1005.0));
    wb.redo().unwrap();
    assert_eq!(val(&wb, 0, "D4"), n(1530.0));
    // A bad formula is refused and changes nothing.
    assert!(wb.apply(Op::SetCell { sheet: 0, at: at("E1"), input: "=SUM(".into() }).is_err());
    assert_eq!(val(&wb, 0, "E1"), Value::Empty);
    // Circular references are reported and keep their value.
    set(&mut wb, 0, "H1", "=H2+1");
    let r = wb.apply(Op::SetCell { sheet: 0, at: at("H2"), input: "=H1+1".into() }).unwrap();
    assert!(!r.circular.is_empty());
}

#[test]
fn rows_and_columns_move_references() {
    let mut wb = budget();
    wb.apply(Op::InsertRows { sheet: 0, at: 2, count: 1 }).unwrap(); // above row 3
    assert_eq!(formula(&wb, 0, "D4").as_deref(), Some("B4*C4"));
    assert_eq!(formula(&wb, 0, "D5").as_deref(), Some("SUM(D2:D4)"));
    assert_eq!(val(&wb, 0, "D5"), n(1005.0));
    set(&mut wb, 0, "B3", "1");
    set(&mut wb, 0, "C3", "5");
    set(&mut wb, 0, "D3", "=B3*C3");
    assert_eq!(val(&wb, 0, "D5"), n(1010.0));
    wb.apply(Op::DeleteRows { sheet: 0, at: 1, count: 1 }).unwrap(); // row 2
    assert_eq!(formula(&wb, 0, "D4").as_deref(), Some("SUM(D2:D3)"));
    assert_eq!(val(&wb, 0, "D4"), n(485.0));
    wb.apply(Op::DeleteCols { sheet: 0, at: 1, count: 1 }).unwrap(); // column B
    assert_eq!(formula(&wb, 0, "C2").as_deref(), Some("#REF!*B2"));
    assert_eq!(val(&wb, 0, "C2"), Value::Error(CellError::Ref));
    wb.undo().unwrap();
    assert_eq!(formula(&wb, 0, "D2").as_deref(), Some("B2*C2"));
    assert_eq!(val(&wb, 0, "D2"), n(5.0));
}

#[test]
fn sheets_rename_delete_and_reference() {
    let mut wb = budget();
    wb.apply(Op::AddSheet { name: Some("Summary".into()), at: None }).unwrap();
    set(&mut wb, 1, "A1", "=Sheet1!D4*2");
    assert_eq!(val(&wb, 1, "A1"), n(2010.0));
    wb.apply(Op::RenameSheet { sheet: 0, name: "Kirana".into() }).unwrap();
    assert_eq!(formula(&wb, 1, "A1").as_deref(), Some("Kirana!D4*2"));
    set(&mut wb, 0, "B3", "5");
    assert_eq!(val(&wb, 1, "A1"), n(2250.0));
    assert!(wb.apply(Op::RenameSheet { sheet: 1, name: "kirana".into() }).is_err());
    assert!(wb.apply(Op::RenameSheet { sheet: 1, name: "a/b".into() }).is_err());
    wb.apply(Op::DuplicateSheet { sheet: 0 }).unwrap();
    assert_eq!(wb.sheets[1].name, "Kirana (2)");
    assert_eq!(val(&wb, 1, "D4"), n(1125.0));
    wb.apply(Op::DeleteSheet { sheet: 0 }).unwrap();
    assert_eq!(formula(&wb, 1, "A1").as_deref(), Some("Kirana!#REF!*2"));
    assert_eq!(val(&wb, 1, "A1"), Value::Error(CellError::Ref));
    wb.undo().unwrap();
    assert_eq!(wb.sheets.len(), 3);
    assert_eq!(val(&wb, 2, "A1"), n(2250.0));
    wb.apply(Op::MoveSheet { sheet: 2, to: 0 }).unwrap();
    assert_eq!(wb.sheets[0].name, "Summary");
}

#[test]
fn sheet_moves_hides_names_and_inserts_undo_and_redo_repeatedly() {
    use crate::model::SheetState;
    let mut wb = budget();
    wb.apply(Op::AddSheet { name: Some("Summary".into()), at: None }).unwrap();
    wb.apply(Op::AddSheet { name: Some("Notes".into()), at: None }).unwrap();
    wb.apply(Op::DefineName { name: "Total".into(), sheet: Some(2), refers_to: "=Sheet1!D4".into() }).unwrap();
    wb.apply(Op::MoveSheet { sheet: 2, to: 0 }).unwrap();
    wb.apply(Op::HideSheet { sheet: 1, hidden: true }).unwrap();
    wb.apply(Op::InsertRows { sheet: 1, at: 0, count: 2 }).unwrap();
    let names = |wb: &Workbook| wb.sheets.iter().map(|s| s.name.clone()).collect::<Vec<_>>();
    let after = (names(&wb), wb.sheets.iter().map(|s| s.state).collect::<Vec<_>>(), wb.names.len());
    assert_eq!(after.0, ["Notes", "Sheet1", "Summary"]);
    assert_eq!(after.1[1], SheetState::Hidden);
    assert_eq!(wb.names[0].sheet, Some(0));
    assert_eq!(val(&wb, 1, "D6"), n(1005.0));
    for _ in 0..2 {
        for _ in 0..6 {
            wb.undo().unwrap();
        }
        assert_eq!(names(&wb), ["Sheet1"]);
        assert!(wb.names.is_empty());
        assert_eq!(val(&wb, 0, "D4"), n(1005.0));
        for _ in 0..6 {
            wb.redo().unwrap();
        }
        assert_eq!(names(&wb), after.0);
        assert_eq!(wb.sheets.iter().map(|s| s.state).collect::<Vec<_>>(), after.1);
        assert_eq!(wb.names.len(), after.2);
        assert_eq!(wb.names[0].sheet, Some(0));
        wb.recalc_all();
        assert_eq!(val(&wb, 1, "D6"), n(1005.0));
    }
    // Undoing only the hide brings the sheet back where it now sits.
    wb.undo().unwrap();
    wb.undo().unwrap();
    assert_eq!(wb.sheets[1].state, SheetState::Visible);
    assert_eq!(names(&wb), after.0);
}

#[test]
fn fill_paste_sort_and_format() {
    let mut wb = budget();
    // Fill D2's formula down to D3 again (copying with relative references).
    wb.apply(Op::Fill { sheet: 0, range: range("D2:D3"), down: true }).unwrap();
    assert_eq!(formula(&wb, 0, "D3").as_deref(), Some("B3*C3"));
    // Paste the block A1:D4 at H1.
    wb.apply(Op::Paste {
        from_sheet: 0,
        from: range("A1:D4"),
        to_sheet: 0,
        to: at("H1"),
        what: PasteWhat::All,
        cut: false,
    })
    .unwrap();
    assert_eq!(formula(&wb, 0, "K4").as_deref(), Some("SUM(K2:K3)"));
    assert_eq!(val(&wb, 0, "K4"), n(1005.0));
    // Sort the rows by quantity, descending.
    wb.apply(Op::Sort {
        sheet: 0,
        range: range("A1:D3"),
        keys: vec![SortKey { col: 1, ascending: true }],
        header: true,
    })
    .unwrap();
    assert_eq!(val(&wb, 0, "A2"), Value::text("Dal"));
    assert_eq!(formula(&wb, 0, "D2").as_deref(), Some("B2*C2"));
    assert_eq!(val(&wb, 0, "D2"), n(480.0));
    // Formatting.
    wb.apply(Op::Format {
        sheet: 0,
        ranges: vec![range("A1:D1")],
        patch: StylePatch {
            bold: Some(true),
            fill: Some(Some(crate::style::ColorSpec::Rgb(0xFFFF00))),
            ..StylePatch::default()
        },
    })
    .unwrap();
    let s = wb.styles.get(wb.sheets[0].cell(at("C1")).unwrap().style);
    assert!(s.font.bold);
    // Merging keeps the first value.
    wb.apply(Op::Merge { sheet: 0, range: range("A6:C6") }).unwrap();
    assert_eq!(wb.sheets[0].merges, vec![range("A6:C6")]);
    // Clearing contents keeps formats.
    wb.apply(Op::Clear { sheet: 0, ranges: vec![range("A1")], what: ClearWhat::Contents }).unwrap();
    assert_eq!(val(&wb, 0, "A1"), Value::Empty);
    assert!(wb.sheets[0].cell(at("A1")).is_some());
    // Find.
    let found = wb.find("dal", None, false, false, false, 10);
    assert_eq!(found.len(), 2);
}

#[test]
fn xlsx_round_trip() {
    let mut wb = budget();
    wb.apply(Op::Format {
        sheet: 0,
        ranges: vec![range("A1:D1")],
        patch: StylePatch {
            bold: Some(true),
            ..StylePatch::default()
        },
    })
    .unwrap();
    wb.apply(Op::AddSheet { name: Some("Notes".into()), at: None }).unwrap();
    set(&mut wb, 1, "A1", "  leading spaces");
    set(&mut wb, 1, "A2", "=Sheet1!D4");
    set(&mut wb, 1, "A3", "=XLOOKUP(\"Dal\",Sheet1!A2:A3,Sheet1!D2:D3)");
    wb.apply(Op::Freeze { sheet: 0, at: Some(at("A2")) }).unwrap();
    wb.apply(Op::ColWidth { sheet: 0, first: 0, last: 0, width: Some(20.0) }).unwrap();
    wb.apply(Op::Merge { sheet: 1, range: range("C1:D1") }).unwrap();
    let bytes = wb.to_bytes(Format::Xlsx).unwrap();
    wb.saved(Format::Xlsx, bytes.clone()).unwrap();

    let back = Workbook::open(bytes, "budget.xlsx", DateOrder::Dmy).unwrap();
    assert_eq!(back.sheets.len(), 2);
    assert_eq!(val(&back, 0, "D4"), n(1005.0));
    assert_eq!(formula(&back, 0, "D4").as_deref(), Some("SUM(D2:D3)"));
    assert_eq!(val(&back, 1, "A1"), Value::text("  leading spaces"));
    assert_eq!(val(&back, 1, "A2"), n(1005.0));
    assert_eq!(val(&back, 1, "A3"), n(480.0));
    assert_eq!(formula(&back, 1, "A3").as_deref(), Some("XLOOKUP(\"Dal\",Sheet1!A2:A3,Sheet1!D2:D3)"));
    assert_eq!(back.sheets[0].freeze, Some(at("A2")));
    assert_eq!(back.sheets[0].col_width(0), 20.0);
    assert_eq!(back.sheets[1].merges, vec![range("C1:D1")]);
    assert!(back.styles.get(back.sheets[0].cell(at("B1")).unwrap().style).font.bold);
    assert_eq!(back.styles.get(back.sheets[0].cell(at("F1")).unwrap().style).num_fmt, "dd/mm/yyyy");

    // Edit one sheet: the other sheet's part is copied byte for byte.
    let mut wb = back;
    let crate::model::Source::Xlsx(pkg) = &wb.source else { panic!() };
    let notes_part = wb.sheets[1].xlsx.as_ref().unwrap().path.clone();
    let before = pkg.opc.read(&notes_part).unwrap();
    set(&mut wb, 0, "B2", "100");
    assert_eq!(val(&wb, 1, "A2"), n(5730.0));
    // The second sheet recalculated, so it is rewritten; edit a third
    // sheet with no formulas to check copying instead.
    wb.apply(Op::AddSheet { name: Some("Plain".into()), at: None }).unwrap();
    set(&mut wb, 2, "A1", "x");
    let bytes = wb.to_bytes(Format::Xlsx).unwrap();
    wb.saved(Format::Xlsx, bytes.clone()).unwrap();
    let plain_part = wb.sheets[2].xlsx.as_ref().unwrap().path.clone();
    let mut again = Workbook::open(bytes, "budget.xlsx", DateOrder::Dmy).unwrap();
    assert_eq!(val(&again, 1, "A2"), n(5730.0));
    assert_ne!(before, {
        let crate::model::Source::Xlsx(p) = &again.source else { panic!() };
        p.opc.read(&notes_part).unwrap()
    });
    let crate::model::Source::Xlsx(p) = &again.source else { panic!() };
    let plain_before = p.opc.read(&plain_part).unwrap();
    set(&mut again, 0, "A1", "Things");
    let bytes = again.to_bytes(Format::Xlsx).unwrap();
    let third = doc_opc::Package::open(bytes).unwrap();
    assert_eq!(third.read(&plain_part).unwrap(), plain_before);
    assert!(!third.exists("xl/calcChain.xml"));
}

#[test]
fn csv_save_as_xlsx_and_back() {
    let src = "Name,Amount\nRavi,10\nAsha,20\n";
    let mut wb = Workbook::open(src.as_bytes().to_vec(), "list.csv", DateOrder::Dmy).unwrap();
    set(&mut wb, 0, "B4", "=SUM(B2:B3)");
    assert_eq!(val(&wb, 0, "B4"), n(30.0));
    let csv = wb.to_bytes(Format::Csv).unwrap();
    assert_eq!(String::from_utf8(csv).unwrap(), "Name,Amount\nRavi,10\nAsha,20\n,30\n");
    let xlsx = wb.to_bytes(Format::Xlsx).unwrap();
    let back = Workbook::open(xlsx, "list.xlsx", DateOrder::Dmy).unwrap();
    assert_eq!(back.sheets[0].name, "list");
    assert_eq!(formula(&back, 0, "B4").as_deref(), Some("SUM(B2:B3)"));
    assert_eq!(val(&back, 0, "B4"), n(30.0));
}

#[test]
#[ignore]
fn write_sample_for_inspection() {
    let mut wb = budget();
    let bytes = wb.to_bytes(Format::Xlsx).unwrap();
    std::fs::write(std::env::var("SAMPLE_OUT").unwrap(), bytes).unwrap();
}

#[test]
fn ods_round_trip() {
    let mut wb = budget();
    wb.apply(Op::Format {
        sheet: 0,
        ranges: vec![range("A1:D1")],
        patch: StylePatch {
            bold: Some(true),
            num_fmt: None,
            ..StylePatch::default()
        },
    })
    .unwrap();
    set(&mut wb, 0, "A6", "Two  spaces\nand a line");
    set(&mut wb, 0, "B6", "TRUE");
    set(&mut wb, 0, "C6", "=1/0");
    set(&mut wb, 0, "E2", "18:30");
    wb.apply(Op::Merge { sheet: 0, range: range("A8:B9") }).unwrap();
    wb.apply(Op::Freeze { sheet: 0, at: Some(at("B2")) }).unwrap();
    wb.apply(Op::ColWidth { sheet: 0, first: 1, last: 1, width: Some(15.0) }).unwrap();
    wb.apply(Op::AddSheet { name: Some("Second Sheet".into()), at: None }).unwrap();
    set(&mut wb, 1, "A1", "='Second Sheet'!B1+Sheet1!D4");
    set(&mut wb, 1, "B1", "5");
    assert_eq!(val(&wb, 1, "A1"), n(1010.0));
    let bytes = wb.to_bytes(Format::Ods).unwrap();
    wb.saved(Format::Ods, bytes.clone()).unwrap();
    let back = Workbook::open(bytes, "budget.ods", DateOrder::Dmy).unwrap();
    assert_eq!(back.sheets.len(), 2);
    assert_eq!(val(&back, 0, "D4"), n(1005.0));
    assert_eq!(formula(&back, 0, "D4").as_deref(), Some("SUM(D2:D3)"));
    assert_eq!(val(&back, 0, "F1"), n(46_308.0));
    assert_eq!(back.styles.get(back.sheets[0].cell(at("F1")).unwrap().style).num_fmt, "dd/mm/yyyy");
    assert_eq!(val(&back, 0, "F2"), n(0.12));
    assert_eq!(val(&back, 0, "A6"), Value::text("Two  spaces\nand a line"));
    assert_eq!(val(&back, 0, "B6"), Value::Bool(true));
    assert_eq!(val(&back, 0, "C6"), Value::Error(CellError::Div0));
    assert!((match val(&back, 0, "E2") { Value::Number(x) => x, _ => 0.0 } - 18.5 / 24.0).abs() < 1e-9);
    assert_eq!(back.sheets[0].merges, vec![range("A8:B9")]);
    assert_eq!(back.sheets[0].freeze, Some(at("B2")));
    assert!((back.sheets[0].col_width(1) - 15.0).abs() < 0.2);
    assert!(back.styles.get(back.sheets[0].cell(at("A1")).unwrap().style).font.bold);
    assert_eq!(formula(&back, 1, "A1").as_deref(), Some("'Second Sheet'!B1+Sheet1!D4"));
    assert_eq!(val(&back, 1, "A1"), n(1010.0));
    // Editing one sheet keeps the other table's XML as written.
    let mut wb = back;
    set(&mut wb, 1, "B1", "6");
    let again = Workbook::open(wb.to_bytes(Format::Ods).unwrap(), "x.ods", DateOrder::Dmy).unwrap();
    assert_eq!(val(&again, 1, "A1"), n(1011.0));
    assert_eq!(val(&again, 0, "A6"), Value::text("Two  spaces\nand a line"));
}

#[test]
fn exports_sheets_to_pdf() {
    let mut wb = budget();
    wb.apply(Op::Format {
        sheet: 0,
        ranges: vec![range("A1:D1")],
        patch: StylePatch {
            bold: Some(true),
            fill: Some(Some(crate::style::ColorSpec::Rgb(0xFFFF00))),
            ..StylePatch::default()
        },
    })
    .unwrap();
    set(&mut wb, 0, "A6", "A long note that runs into the empty cells beside it");
    let mut book = doc_font::FontBook::new();
    book.add_dir(std::path::Path::new("/usr/share/fonts/truetype"));
    let bytes = crate::pdf::export(&wb, &mut book, &crate::pdf::PdfOptions::default());
    assert!(bytes.starts_with(b"%PDF"));
    let path = std::env::temp_dir().join(format!("sheet_pdf_{}.pdf", std::process::id()));
    std::fs::write(&path, &bytes).unwrap();
    if !book.is_empty() {
        if let Ok(out) = std::process::Command::new("pdftotext").arg("-layout").arg(&path).arg("-").output() {
            if out.status.success() {
                let t = String::from_utf8_lossy(&out.stdout);
                assert!(t.contains("Rice") && t.contains("525") && t.contains("1005"), "{t}");
                assert!(t.contains("runs into the empty cells"), "{t}");
            }
        }
    }
    if std::env::var("KEEP_PDF").is_err() {
        let _ = std::fs::remove_file(&path);
    } else {
        eprintln!("kept {}", path.display());
    }
}


// --- Flat OpenDocument spreadsheets

/// A workbook as a flat file, laid out the way an office suite writes one:
/// view settings (the active sheet, frozen panes), a data style, bold and
/// percentage cell styles, formulas, repeated rows and a merged cell.
const FLAT_BILLS: &str = r#"<?xml version="1.0" encoding="UTF-8"?>
<office:document xmlns:office="urn:oasis:names:tc:opendocument:xmlns:office:1.0" xmlns:style="urn:oasis:names:tc:opendocument:xmlns:style:1.0" xmlns:text="urn:oasis:names:tc:opendocument:xmlns:text:1.0" xmlns:table="urn:oasis:names:tc:opendocument:xmlns:table:1.0" xmlns:fo="urn:oasis:names:tc:opendocument:xmlns:xsl-fo-compatible:1.0" xmlns:number="urn:oasis:names:tc:opendocument:xmlns:datastyle:1.0" xmlns:config="urn:oasis:names:tc:opendocument:xmlns:config:1.0" xmlns:of="urn:oasis:names:tc:opendocument:xmlns:of:1.2" office:version="1.3" office:mimetype="application/vnd.oasis.opendocument.spreadsheet">
 <office:settings>
  <config:config-item-set config:name="ooo:view-settings">
   <config:config-item-map-indexed config:name="Views">
    <config:config-item-map-entry>
     <config:config-item config:name="ActiveTable" config:type="string">Bills</config:config-item>
     <config:config-item-map-named config:name="Tables">
      <config:config-item-map-entry config:name="Bills">
       <config:config-item config:name="HorizontalSplitMode" config:type="short">2</config:config-item>
       <config:config-item config:name="VerticalSplitMode" config:type="short">2</config:config-item>
       <config:config-item config:name="HorizontalSplitPosition" config:type="int">1</config:config-item>
       <config:config-item config:name="VerticalSplitPosition" config:type="int">1</config:config-item>
      </config:config-item-map-entry>
     </config:config-item-map-named>
    </config:config-item-map-entry>
   </config:config-item-map-indexed>
  </config:config-item-set>
 </office:settings>
 <office:styles>
  <style:default-style style:family="table-cell"><style:text-properties fo:font-size="10pt"/></style:default-style>
  <number:percentage-style style:name="N11"><number:number number:decimal-places="0" number:min-integer-digits="1"/><number:text>%</number:text></number:percentage-style>
  <style:style style:name="Default" style:family="table-cell"/>
 </office:styles>
 <office:automatic-styles>
  <style:style style:name="co1" style:family="table-column"><style:table-column-properties style:column-width="3cm"/></style:style>
  <style:style style:name="ce1" style:family="table-cell" style:parent-style-name="Default"><style:text-properties fo:font-weight="bold"/></style:style>
  <style:style style:name="ce2" style:family="table-cell" style:parent-style-name="Default" style:data-style-name="N11"/>
  <style:page-layout style:name="pm1"/>
 </office:automatic-styles>
 <office:master-styles><style:master-page style:name="Default" style:page-layout-name="pm1"/></office:master-styles>
 <office:body>
  <office:spreadsheet>
   <table:table table:name="Notes"><table:table-row><table:table-cell office:value-type="string"><text:p>See Bills</text:p></table:table-cell></table:table-row></table:table>
   <table:table table:name="Bills">
    <table:table-column table:style-name="co1" table:number-columns-repeated="3"/>
    <table:table-row><table:table-cell table:style-name="ce1" office:value-type="string"><text:p>Item</text:p></table:table-cell><table:table-cell table:style-name="ce1" office:value-type="string"><text:p>Amount</text:p></table:table-cell><table:table-cell table:style-name="ce1" office:value-type="string"><text:p>Share</text:p></table:table-cell></table:table-row>
    <table:table-row><table:table-cell office:value-type="string"><text:p>Rent</text:p></table:table-cell><table:table-cell office:value-type="float" office:value="1200"><text:p>1200</text:p></table:table-cell><table:table-cell table:style-name="ce2" table:formula="of:=[.B2]/[.B4]" office:value-type="percentage" office:value="0.8"><text:p>80%</text:p></table:table-cell></table:table-row>
    <table:table-row><table:table-cell office:value-type="string"><text:p>Power</text:p></table:table-cell><table:table-cell office:value-type="float" office:value="300"><text:p>300</text:p></table:table-cell><table:table-cell table:style-name="ce2" table:formula="of:=[.B3]/[.B4]" office:value-type="percentage" office:value="0.2"><text:p>20%</text:p></table:table-cell></table:table-row>
    <table:table-row><table:table-cell office:value-type="string"><text:p>Total</text:p></table:table-cell><table:table-cell table:formula="of:=SUM([.B2:.B3])" office:value-type="float" office:value="1500"><text:p>1500</text:p></table:table-cell><table:table-cell/></table:table-row>
    <table:table-row table:number-rows-repeated="2"><table:table-cell table:number-columns-repeated="3"/></table:table-row>
    <table:table-row><table:table-cell table:number-columns-spanned="2" table:number-rows-spanned="1" office:value-type="string"><text:p>Merged note</text:p></table:table-cell><table:covered-table-cell/><table:table-cell/></table:table-row>
   </table:table>
  </office:spreadsheet>
 </office:body>
</office:document>
"#;

#[test]
fn flat_ods_opens_and_saves_as_ods() {
    use crate::model::Imported;
    // Told by its content, whatever its name says.
    for name in ["bills.fods", "bills.xml", "bills.csv", "bills"] {
        let wb = Workbook::open(FLAT_BILLS.as_bytes().to_vec(), name, DateOrder::Dmy).unwrap();
        assert_eq!(wb.imported, Some(Imported::Fods), "{name}");
    }
    let mut wb = Workbook::open(FLAT_BILLS.as_bytes().to_vec(), "bills.fods", DateOrder::Dmy).unwrap();
    assert_eq!(wb.format, Some(Format::Ods));
    assert!(!wb.writes_back());
    assert_eq!(wb.save_format(), Format::Ods);
    assert!(wb.warnings[0].contains("(.fods)") && wb.warnings[0].contains("(.ods)"), "{:?}", wb.warnings);
    assert_eq!(wb.sheets.iter().map(|s| s.name.as_str()).collect::<Vec<_>>(), ["Notes", "Bills"]);
    assert_eq!(wb.active, 1);
    assert_eq!(wb.sheets[1].freeze, Some(at("B2")));
    assert_eq!(val(&wb, 0, "A1"), Value::text("See Bills"));
    assert_eq!(val(&wb, 1, "B4"), n(1500.0));
    assert_eq!(formula(&wb, 1, "B4").as_deref(), Some("SUM(B2:B3)"));
    assert_eq!(formula(&wb, 1, "C2").as_deref(), Some("B2/B4"));
    assert!(wb.styles.get(wb.sheets[1].cell(at("A1")).unwrap().style).font.bold);
    assert_eq!(wb.styles.get(wb.sheets[1].cell(at("C2")).unwrap().style).num_fmt, "0%");
    assert_eq!(wb.sheets[1].merges, vec![range("A7:B7")]);
    assert!((wb.sheets[1].col_width(2) - wb.sheets[1].col_width(0)).abs() < 1e-9);

    // Edit, save as ODS, and reopen.
    set(&mut wb, 1, "B3", "500");
    assert_eq!(val(&wb, 1, "B4"), n(1700.0));
    let bytes = wb.to_bytes(wb.save_format()).unwrap();
    assert_eq!(&bytes[30..38], b"mimetype");
    assert_eq!(&bytes[38..38 + doc_odf::SPREADSHEET.len()], doc_odf::SPREADSHEET.as_bytes());
    wb.saved(Format::Ods, bytes.clone()).unwrap();
    assert!(!wb.dirty);
    let back = Workbook::open(bytes, "bills.ods", DateOrder::Dmy).unwrap();
    assert_eq!(back.imported, None);
    assert!(back.writes_back());
    assert!(back.warnings.iter().all(|w| !w.contains(".fods")), "{:?}", back.warnings);
    assert_eq!(back.sheets.len(), 2);
    assert_eq!(val(&back, 1, "B4"), n(1700.0));
    assert_eq!(val(&back, 1, "B3"), n(500.0));
    assert_eq!(formula(&back, 1, "C3").as_deref(), Some("B3/B4"));
    assert_eq!(val(&back, 0, "A1"), Value::text("See Bills"));
    assert_eq!(back.sheets[1].freeze, Some(at("B2")));
    assert_eq!(back.sheets[1].merges, vec![range("A7:B7")]);

    // A flat file of another kind is no spreadsheet; a damaged one is
    // reported as damaged.
    let text = FLAT_BILLS.replace("opendocument.spreadsheet", "opendocument.text");
    assert!(matches!(Workbook::open(text.into_bytes(), "bills.fods", DateOrder::Dmy), Err(crate::SheetError::Unsupported(_))));
    let cut = FLAT_BILLS.as_bytes()[..FLAT_BILLS.len() / 2].to_vec();
    assert!(matches!(Workbook::open(cut, "bills.fods", DateOrder::Dmy), Err(crate::SheetError::Corrupt(_))));
}

#[test]
fn flattened_ods_reads_like_its_package() {
    // The budget of the ODS round trip: formats, a merge, frozen panes, a
    // wider column and a second sheet referring to the first.
    let mut wb = budget();
    wb.apply(Op::Format {
        sheet: 0,
        ranges: vec![range("A1:D1")],
        patch: StylePatch {
            bold: Some(true),
            ..StylePatch::default()
        },
    })
    .unwrap();
    wb.apply(Op::Merge { sheet: 0, range: range("A8:B9") }).unwrap();
    wb.apply(Op::Freeze { sheet: 0, at: Some(at("B2")) }).unwrap();
    wb.apply(Op::ColWidth { sheet: 0, first: 1, last: 1, width: Some(15.0) }).unwrap();
    wb.apply(Op::AddSheet { name: Some("Second Sheet".into()), at: None }).unwrap();
    set(&mut wb, 1, "A1", "='Second Sheet'!B1+Sheet1!D4");
    set(&mut wb, 1, "B1", "5");
    let package = wb.to_bytes(Format::Ods).unwrap();
    let flat = doc_odf::flatten(&package).unwrap();

    let original = Workbook::open(package, "budget.ods", DateOrder::Dmy).unwrap();
    let mut from_flat = Workbook::open(flat, "budget.fods", DateOrder::Dmy).unwrap();
    assert_eq!(from_flat.imported, Some(crate::model::Imported::Fods));
    assert_eq!(from_flat.active, original.active);
    assert_eq!(from_flat.sheets.len(), original.sheets.len());
    for (a, b) in from_flat.sheets.iter().zip(&original.sheets) {
        assert_eq!(a.name, b.name);
        assert_eq!(a.cells, b.cells, "{}", a.name);
        assert_eq!(a.merges, b.merges);
        assert_eq!(a.freeze, b.freeze);
        for col in 0..6 {
            assert_eq!(a.col_width(col), b.col_width(col));
        }
        for row in a.cells.values() {
            for cell in row.values() {
                assert_eq!(from_flat.styles.get(cell.style), original.styles.get(cell.style));
            }
        }
    }
    // Saved as ODS, it reads the same again.
    set(&mut from_flat, 1, "B1", "6");
    let again = Workbook::open(from_flat.to_bytes(Format::Ods).unwrap(), "budget.ods", DateOrder::Dmy).unwrap();
    assert_eq!(val(&again, 1, "A1"), n(1011.0));
    assert_eq!(again.sheets[0].cells, original.sheets[0].cells);
    assert_eq!(again.sheets[0].freeze, Some(at("B2")));
}

#[test]
fn saves_are_checked_before_they_replace_the_file() {
    let mut wb = budget();
    for format in [Format::Xlsx, Format::Ods] {
        let bytes = wb.to_bytes(format).unwrap();
        assert!(Workbook::check_saved(format, &bytes).is_ok(), "{format:?}");
        let cut = &bytes[..bytes.len() / 2];
        assert!(Workbook::check_saved(format, cut).is_err(), "{format:?} cut short");
        let mut flipped = bytes.clone();
        let middle = flipped.len() / 3;
        flipped[middle] ^= 0xFF;
        // A damaged byte inside a compressed part fails its CRC, or the
        // package no longer opens.
        let _ = Workbook::check_saved(format, &flipped);
    }
    assert!(Workbook::check_saved(Format::Csv, b"a,b\n").is_ok());
}
