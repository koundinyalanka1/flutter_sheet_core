//! Hostile workbooks, built here, read on phone-sized stacks: each must
//! open or be refused cleanly, quickly, without overflowing the stack.

use std::time::{Duration, Instant};

use doc_zip::{Compression, ZipWriter};
use sheet_core::numfmt::DateOrder;
use sheet_core::{CellRef, SheetError, Value, Workbook};

const NS: &str = "http://schemas.openxmlformats.org/spreadsheetml/2006/main";
const REL: &str = "http://schemas.openxmlformats.org/officeDocument/2006/relationships";

/// A one-sheet XLSX package with this `<sheetData>` (and columns) and,
/// when given, this shared-string table.
fn xlsx(cols: &str, sheet_data: &str, shared: Option<&str>) -> Vec<u8> {
    let mut types = String::from(concat!(
        r#"<?xml version="1.0" encoding="UTF-8"?><Types xmlns="http://schemas.openxmlformats.org/package/2006/content-types">"#,
        r#"<Default Extension="rels" ContentType="application/vnd.openxmlformats-package.relationships+xml"/>"#,
        r#"<Default Extension="xml" ContentType="application/xml"/>"#,
        r#"<Override PartName="/xl/workbook.xml" ContentType="application/vnd.openxmlformats-officedocument.spreadsheetml.sheet.main+xml"/>"#,
        r#"<Override PartName="/xl/worksheets/sheet1.xml" ContentType="application/vnd.openxmlformats-officedocument.spreadsheetml.worksheet+xml"/>"#,
    ));
    let mut wb_rels = format!(
        r#"<?xml version="1.0" encoding="UTF-8"?><Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships"><Relationship Id="rId1" Type="{REL}/worksheet" Target="worksheets/sheet1.xml"/>"#
    );
    if shared.is_some() {
        types.push_str(r#"<Override PartName="/xl/sharedStrings.xml" ContentType="application/vnd.openxmlformats-officedocument.spreadsheetml.sharedStrings+xml"/>"#);
        wb_rels.push_str(&format!(r#"<Relationship Id="rId2" Type="{REL}/sharedStrings" Target="sharedStrings.xml"/>"#));
    }
    types.push_str("</Types>");
    wb_rels.push_str("</Relationships>");
    let rels = format!(
        r#"<?xml version="1.0" encoding="UTF-8"?><Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships"><Relationship Id="rId1" Type="{REL}/officeDocument" Target="xl/workbook.xml"/></Relationships>"#
    );
    let workbook = format!(
        r#"<?xml version="1.0" encoding="UTF-8"?><workbook xmlns="{NS}" xmlns:r="{REL}"><sheets><sheet name="Sheet1" sheetId="1" r:id="rId1"/></sheets></workbook>"#
    );
    let sheet = format!(r#"<?xml version="1.0" encoding="UTF-8"?><worksheet xmlns="{NS}">{cols}<sheetData>{sheet_data}</sheetData></worksheet>"#);
    let mut w = ZipWriter::new();
    let c = Compression::DEFAULT;
    w.add("[Content_Types].xml", types.as_bytes(), c).unwrap();
    w.add("_rels/.rels", rels.as_bytes(), c).unwrap();
    w.add("xl/workbook.xml", workbook.as_bytes(), c).unwrap();
    w.add("xl/_rels/workbook.xml.rels", wb_rels.as_bytes(), c).unwrap();
    w.add("xl/worksheets/sheet1.xml", sheet.as_bytes(), c).unwrap();
    if let Some(sst) = shared {
        let sst = format!(r#"<?xml version="1.0" encoding="UTF-8"?><sst xmlns="{NS}">{sst}</sst>"#);
        w.add("xl/sharedStrings.xml", sst.as_bytes(), c).unwrap();
    }
    w.finish()
}

/// Run `f` on a thread with `stack` bytes of stack, as a phone's worker
/// thread might have, and check it ends within the time allowed.
fn on_small_stack<T: Send + 'static>(stack: usize, f: impl FnOnce() -> T + Send + 'static) -> T {
    let limit = if cfg!(debug_assertions) { Duration::from_secs(30) } else { Duration::from_secs(2) };
    let start = Instant::now();
    let out = std::thread::Builder::new().stack_size(stack).spawn(f).unwrap().join().expect("no panic or stack overflow");
    assert!(start.elapsed() < limit, "took {:?}", start.elapsed());
    out
}

fn at(a1: &str) -> CellRef {
    CellRef::parse_a1(a1).unwrap().0
}

fn chain(len: u32) -> Vec<u8> {
    let mut data = String::from(r#"<row r="1"><c r="A1"><v>1</v></c></row>"#);
    for r in 2..=len {
        data.push_str(&format!(r#"<row r="{r}"><c r="A{r}"><f>A{}+1</f></c></row>"#, r - 1));
    }
    xlsx("", &data, None)
}

#[test]
fn a_chain_of_8000_formulas_calculates_on_small_stacks() {
    for stack in [1 << 20, 512 << 10] {
        let bytes = chain(8000);
        let last = on_small_stack(stack, move || {
            let mut wb = Workbook::open(bytes, "chain.xlsx", DateOrder::Dmy).unwrap();
            wb.recalc_all();
            wb.sheets[0].value(at("A8000"))
        });
        assert_eq!(last, Value::Number(8000.0), "stack {stack}");
    }
}

#[test]
fn twenty_thousand_column_records_open_quickly() {
    let mut cols = String::from("<cols>");
    for i in 1..=20_000u32 {
        cols.push_str(&format!(r#"<col min="{i}" max="{i}" width="12" customWidth="1"/>"#));
    }
    cols.push_str("</cols>");
    let bytes = xlsx(&cols, r#"<row r="1"><c r="A1"><v>7</v></c></row>"#, None);
    let result = on_small_stack(512 << 10, move || {
        Workbook::open(bytes, "cols.xlsx", DateOrder::Dmy).map(|wb| wb.sheets[0].value(at("A1")))
    });
    match result {
        Ok(v) => assert_eq!(v, Value::Number(7.0)),
        Err(e) => assert!(matches!(e, SheetError::TooLarge(_) | SheetError::Corrupt(_)), "{e}"),
    }
}

#[test]
fn a_shared_string_bomb_is_refused() {
    let mut sst = String::with_capacity(12 << 20);
    for _ in 0..600_000 {
        sst.push_str("<si><t>x</t></si>");
    }
    let bytes = xlsx("", r#"<row r="1"><c r="A1" t="s"><v>0</v></c></row>"#, Some(&sst));
    let result = on_small_stack(512 << 10, move || Workbook::open(bytes, "bomb.xlsx", DateOrder::Dmy).map(|_| ()));
    assert!(matches!(result, Err(SheetError::TooLarge(_))), "{result:?}");
}

#[test]
fn a_deeply_nested_formula_is_refused_or_read_on_a_small_stack() {
    let depth = 5_000;
    let formula = format!("{}1{}", "(".repeat(depth), ")".repeat(depth));
    let data = format!(r#"<row r="1"><c r="A1"><f>{formula}</f></c></row>"#);
    let bytes = xlsx("", &data, None);
    on_small_stack(512 << 10, move || {
        if let Ok(mut wb) = Workbook::open(bytes, "deep.xlsx", DateOrder::Dmy) {
            wb.recalc_all();
            let _ = wb.sheets[0].value(at("A1"));
        }
    });
}
