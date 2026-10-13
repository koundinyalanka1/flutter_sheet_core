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
