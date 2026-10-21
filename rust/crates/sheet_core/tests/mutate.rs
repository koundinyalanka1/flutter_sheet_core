//! Fixed-seed mutation of workbooks built here (XLSX, ODS, CSV): every
//! damaged file opens or is refused without a panic, hang or stack
//! overflow, and whatever opens saves to a file that reads back with the
//! edit in it. An unedited save keeps parts the engine does not own
//! byte for byte.

use std::time::Duration;

use doc_testkit::{mutate, run_limited, Limits, Rng};
use doc_zip::{Compression, ZipArchive, ZipWriter};
use sheet_core::edit::Op;
use sheet_core::numfmt::DateOrder;
use sheet_core::{CellRef, Format, Value, Workbook};

const SEED: u64 = 0x5348_4545_5442_3901;
const NS: &str = "http://schemas.openxmlformats.org/spreadsheetml/2006/main";
const REL: &str = "http://schemas.openxmlformats.org/officeDocument/2006/relationships";
const OPAQUE: &str = "customXml/item1.xml";
const OPAQUE_BYTES: &[u8] = br#"<?xml version="1.0"?><x:data xmlns:x="urn:unknown">kept as it was &amp; untouched</x:data>"#;

fn at(a1: &str) -> CellRef {
    CellRef::parse_a1(a1).unwrap().0
}

/// A small workbook: shared strings, numbers, a formula, a second sheet,
/// a defined name, and a part the engine does not read.
fn xlsx() -> Vec<u8> {
    let types = concat!(
        r#"<?xml version="1.0" encoding="UTF-8"?><Types xmlns="http://schemas.openxmlformats.org/package/2006/content-types">"#,
        r#"<Default Extension="rels" ContentType="application/vnd.openxmlformats-package.relationships+xml"/>"#,
        r#"<Default Extension="xml" ContentType="application/xml"/>"#,
        r#"<Override PartName="/xl/workbook.xml" ContentType="application/vnd.openxmlformats-officedocument.spreadsheetml.sheet.main+xml"/>"#,
        r#"<Override PartName="/xl/worksheets/sheet1.xml" ContentType="application/vnd.openxmlformats-officedocument.spreadsheetml.worksheet+xml"/>"#,
        r#"<Override PartName="/xl/worksheets/sheet2.xml" ContentType="application/vnd.openxmlformats-officedocument.spreadsheetml.worksheet+xml"/>"#,
        r#"<Override PartName="/xl/sharedStrings.xml" ContentType="application/vnd.openxmlformats-officedocument.spreadsheetml.sharedStrings+xml"/>"#,
        "</Types>",
    );
    let rels = format!(
        r#"<?xml version="1.0" encoding="UTF-8"?><Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships"><Relationship Id="rId1" Type="{REL}/officeDocument" Target="xl/workbook.xml"/></Relationships>"#
    );
    let wb_rels = format!(
        r#"<?xml version="1.0" encoding="UTF-8"?><Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships"><Relationship Id="rId1" Type="{REL}/worksheet" Target="worksheets/sheet1.xml"/><Relationship Id="rId2" Type="{REL}/worksheet" Target="worksheets/sheet2.xml"/><Relationship Id="rId3" Type="{REL}/sharedStrings" Target="sharedStrings.xml"/></Relationships>"#
    );
    let workbook = format!(
        r#"<?xml version="1.0" encoding="UTF-8"?><workbook xmlns="{NS}" xmlns:r="{REL}"><sheets><sheet name="Accounts" sheetId="1" r:id="rId1"/><sheet name="Notes" sheetId="2" r:id="rId2"/></sheets><definedNames><definedName name="Total">Accounts!$B$3</definedName></definedNames></workbook>"#
    );
    let sheet1 = format!(
        r#"<?xml version="1.0" encoding="UTF-8"?><worksheet xmlns="{NS}"><cols><col min="1" max="1" width="20" customWidth="1"/></cols><sheetData><row r="1"><c r="A1" t="s"><v>0</v></c><c r="B1"><v>100</v></c></row><row r="2"><c r="A2" t="s"><v>1</v></c><c r="B2"><v>50.5</v></c></row><row r="3"><c r="A3" t="inlineStr"><is><t>Total</t></is></c><c r="B3"><f>SUM(B1:B2)</f><v>150.5</v></c></row></sheetData><mergeCells count="1"><mergeCell ref="C1:D1"/></mergeCells></worksheet>"#
    );
    let sheet2 = format!(
        r#"<?xml version="1.0" encoding="UTF-8"?><worksheet xmlns="{NS}"><sheetData><row r="1"><c r="A1"><f>Total*2</f></c><c r="B1" t="b"><v>1</v></c></row></sheetData></worksheet>"#
    );
    let sst = format!(
        r#"<?xml version="1.0" encoding="UTF-8"?><sst xmlns="{NS}" count="2" uniqueCount="2"><si><t>Rice</t></si><si><r><t>Dal</t></r><r><t xml:space="preserve"> (red)</t></r></si></sst>"#
    );
    let mut w = ZipWriter::new();
    let c = Compression::DEFAULT;
    w.add("[Content_Types].xml", types.as_bytes(), c).unwrap();
    w.add("_rels/.rels", rels.as_bytes(), c).unwrap();
    w.add("xl/workbook.xml", workbook.as_bytes(), c).unwrap();
    w.add("xl/_rels/workbook.xml.rels", wb_rels.as_bytes(), c).unwrap();
    w.add("xl/worksheets/sheet1.xml", sheet1.as_bytes(), c).unwrap();
    w.add("xl/worksheets/sheet2.xml", sheet2.as_bytes(), c).unwrap();
    w.add("xl/sharedStrings.xml", sst.as_bytes(), c).unwrap();
    w.add(OPAQUE, OPAQUE_BYTES, c).unwrap();
    w.finish()
}

/// The same kind of content as an OpenDocument spreadsheet, made by the
/// engine itself.
fn ods() -> Vec<u8> {
    let mut wb = Workbook::blank(DateOrder::Dmy);
    for (cell, input) in [("A1", "Rice"), ("B1", "100"), ("A2", "Dal"), ("B2", "50.5"), ("B3", "=SUM(B1:B2)")] {
        wb.apply(Op::SetCell { sheet: 0, at: at(cell), input: input.into() }).unwrap();
    }
    wb.to_bytes(Format::Ods).unwrap()
}

const CSV: &[u8] = b"Item,Amount\nRice,100\n\"Dal, red\",50.5\nTotal,=SUM(B2:B3)\n";

/// Open, calculate, read every used cell, edit A1 and save; whatever
/// saves must pass the save check and reopen with the edit.
fn exercise(bytes: Vec<u8>, name: &str, format: Format) -> bool {
    let Ok(mut wb) = Workbook::open(bytes, name, DateOrder::Dmy) else { return false };
    wb.recalc_all();
    for sheet in &wb.sheets {
        if let Some(used) = sheet.used_range() {
            let rows = used.start.row..=used.end.row.min(used.start.row + 20);
            for row in rows {
                for col in used.start.col..=used.end.col.min(used.start.col + 20) {
                    let _ = sheet.value(CellRef { row, col });
                }
            }
        }
    }
    if wb.sheets.is_empty() || wb.apply(Op::SetCell { sheet: 0, at: at("A1"), input: "edited".into() }).is_err() {
        return true;
    }
    // A workbook that cannot be written back in its own format is saved as
    // XLSX, as the app does.
    let format = if wb.writes_back() { format } else { Format::Xlsx };
    if let Ok(saved) = wb.to_bytes(format) {
        Workbook::check_saved(format, &saved).expect("a save the engine produced passes its own check");
        let reopened = Workbook::open(saved, &format!("saved.{}", ext(format)), DateOrder::Dmy).expect("a saved workbook reopens");
        assert_eq!(reopened.sheets[0].value(at("A1")), Value::Text("edited".into()));
    }
    true
}

fn ext(format: Format) -> &'static str {
    match format {
        Format::Xlsx => "xlsx",
        Format::Xlsm => "xlsm",
        Format::Ods => "ods",
        Format::Csv => "csv",
        Format::Tsv => "tsv",
    }
}

#[test]
fn unedited_and_edited_saves_keep_unknown_parts_byte_for_byte() {
    run_limited("unedited_and_edited_saves_keep_unknown_parts_byte_for_byte", Limits::mutation(Duration::from_secs(20)), || {
        let original = xlsx();
        let before = ZipArchive::new(original.clone()).unwrap();
        let opaque = before.find(OPAQUE).unwrap();
        let mut wb = Workbook::open(original, "book.xlsx", DateOrder::Dmy).unwrap();
        wb.recalc_all();
        assert_eq!(wb.sheets[1].value(at("A1")), Value::Number(301.0));

        let unedited = wb.to_bytes(Format::Xlsx).unwrap();
        let after = ZipArchive::new(unedited.clone()).unwrap();
        let kept = after.find(OPAQUE).expect("the unknown part is kept");
        assert_eq!(after.raw_data(kept), before.raw_data(opaque));
        let reread = Workbook::open(unedited, "book.xlsx", DateOrder::Dmy).unwrap();
        for cell in ["A1", "B1", "A2", "B2", "A3", "B3"] {
            assert_eq!(reread.sheets[0].value(at(cell)), wb.sheets[0].value(at(cell)), "{cell}");
        }

        wb.apply(Op::SetCell { sheet: 0, at: at("B1"), input: "200".into() }).unwrap();
        let edited = wb.to_bytes(Format::Xlsx).unwrap();
        let after = ZipArchive::new(edited.clone()).unwrap();
        assert_eq!(after.raw_data(after.find(OPAQUE).unwrap()), before.raw_data(opaque));
        let mut reread = Workbook::open(edited, "book.xlsx", DateOrder::Dmy).unwrap();
        reread.recalc_all();
        assert_eq!(reread.sheets[0].value(at("B3")), Value::Number(250.5));
        assert_eq!(reread.sheets[0].value(at("A2")), Value::Text("Dal (red)".into()));
        assert_eq!(reread.sheets[1].value(at("A1")), Value::Number(501.0));
    }).unwrap();
}

#[test]
fn fuzz_xlsx_fixed_seed() {
    run_limited("fuzz_xlsx_fixed_seed", Limits::mutation(Duration::from_secs(120)), || {
        let seed = xlsx();
        let mut rng = Rng::new(SEED);
        let mut opened = 0;
        for _ in 0..2048 {
            let bytes = mutate(&mut rng, &seed, OPAQUE_BYTES);
            opened += usize::from(exercise(bytes, "fuzz.xlsx", Format::Xlsx));
        }
        assert!(opened > 0);
        eprintln!("XLSX seed={SEED:x} iterations=2048 opened={opened}");
    }).unwrap();
}

#[test]
fn fuzz_ods_fixed_seed() {
    run_limited("fuzz_ods_fixed_seed", Limits::mutation(Duration::from_secs(120)), || {
        let seed = ods();
        let mut rng = Rng::new(SEED ^ 1);
        let mut opened = 0;
        for _ in 0..2048 {
            let bytes = mutate(&mut rng, &seed, CSV);
            opened += usize::from(exercise(bytes, "fuzz.ods", Format::Ods));
        }
        assert!(opened > 0);
        eprintln!("ODS seed={:x} iterations=2048 opened={opened}", SEED ^ 1);
    }).unwrap();
}

#[test]
fn fuzz_csv_and_formulas_fixed_seed() {
    run_limited("fuzz_csv_and_formulas_fixed_seed", Limits::mutation(Duration::from_secs(60)), || {
        let formulas: &[u8] = b"=IF(SUM(B2:B3)>100,\"big\",VLOOKUP(\"Rice\",A1:B4,2,FALSE))&TEXT(NOW(),\"dd/mm\")";
        let mut rng = Rng::new(SEED ^ 2);
        let mut opened = 0;
        for _ in 0..4096 {
            let bytes = mutate(&mut rng, CSV, formulas);
            opened += usize::from(exercise(bytes, "fuzz.csv", Format::Csv));
        }
        assert!(opened > 0);
        eprintln!("CSV seed={:x} iterations=4096 opened={opened}", SEED ^ 2);
    }).unwrap();
}
