//! A small BIFF8 workbook built here, read back.

use super::*;

fn rec(out: &mut Vec<u8>, id: u16, data: &[u8]) {
    out.extend_from_slice(&id.to_le_bytes());
    out.extend_from_slice(&(data.len() as u16).to_le_bytes());
    out.extend_from_slice(data);
}

fn short_str(s: &str) -> Vec<u8> {
    let mut v = vec![s.len() as u8, 0];
    v.extend_from_slice(s.as_bytes());
    v
}

fn xl_str(s: &str) -> Vec<u8> {
    let mut v = (s.len() as u16).to_le_bytes().to_vec();
    v.push(0);
    v.extend_from_slice(s.as_bytes());
    v
}

fn cell(row: u16, col: u16, xf: u16) -> Vec<u8> {
    let mut v = row.to_le_bytes().to_vec();
    v.extend_from_slice(&col.to_le_bytes());
    v.extend_from_slice(&xf.to_le_bytes());
    v
}

fn font(name: &str, bold: bool) -> Vec<u8> {
    let mut f = vec![];
    f.extend_from_slice(&220u16.to_le_bytes());
    f.extend_from_slice(&0u16.to_le_bytes());
    f.extend_from_slice(&0x7FFFu16.to_le_bytes());
    f.extend_from_slice(&(if bold { 700u16 } else { 400 }).to_le_bytes());
    f.extend_from_slice(&[0, 0, 0, 0, 0, 0]);
    f.extend_from_slice(&short_str(name));
    f
}

fn xf(font: u16, fmt: u16, fill: Option<u16>) -> Vec<u8> {
    let mut x = vec![];
    x.extend_from_slice(&font.to_le_bytes());
    x.extend_from_slice(&fmt.to_le_bytes());
    x.extend_from_slice(&0x0001u16.to_le_bytes());
    x.extend_from_slice(&[0x20, 0, 0, 0]);
    // Thin bottom border, black.
    x.extend_from_slice(&(0x1000u32 | (8 << 16)).to_le_bytes());
    let pattern = if fill.is_some() { 1u32 << 26 } else { 0 };
    x.extend_from_slice(&(pattern | (8 << 7)).to_le_bytes());
    x.extend_from_slice(&fill.unwrap_or(64).to_le_bytes());
    x
}

fn workbook() -> Vec<u8> {
    // The sheet substream, assembled first to know the globals' size.
    let mut sheet = vec![];
    rec(&mut sheet, 0x0809, &[0, 6, 0x10, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0]);
    let mut colinfo = vec![0, 0, 0, 0];
    colinfo.extend_from_slice(&(20u16 * 256).to_le_bytes());
    colinfo.extend_from_slice(&[15, 0, 0, 0, 0, 0]);
    rec(&mut sheet, 0x007D, &colinfo);
    // A1 "Item" (bold, filled), B1 "Qty"; A2 "Rice", B2 10 (RK); B3 =B2*2
    let mut a1 = cell(0, 0, 1);
    a1.extend_from_slice(&0u32.to_le_bytes());
    rec(&mut sheet, 0x00FD, &a1);
    let mut b1 = cell(0, 1, 0);
    b1.extend_from_slice(&1u32.to_le_bytes());
    rec(&mut sheet, 0x00FD, &b1);
    let mut a2 = cell(1, 0, 0);
    a2.extend_from_slice(&2u32.to_le_bytes());
    rec(&mut sheet, 0x00FD, &a2);
    let mut b2 = cell(1, 1, 0);
    b2.extend_from_slice(&((10u32 << 2) | 2).to_le_bytes());
    rec(&mut sheet, 0x027E, &b2);
    let mut f = cell(2, 1, 2);
    f.extend_from_slice(&20.0f64.to_le_bytes());
    f.extend_from_slice(&[0, 0, 0, 0, 0, 0]);
    let rgce = [0x24u8, 1, 0, 1, 0xC0, 0x1E, 2, 0, 0x05];
    f.extend_from_slice(&(rgce.len() as u16).to_le_bytes());
    f.extend_from_slice(&rgce);
    rec(&mut sheet, 0x0006, &f);
    // A3: a string formula =A2&"!" with its STRING record.
    let mut f2 = cell(2, 0, 0);
    f2.extend_from_slice(&[0, 0, 0, 0, 0, 0, 0xFF, 0xFF]);
    f2.extend_from_slice(&[0, 0, 0, 0, 0, 0]);
    let rgce2 = [0x24u8, 1, 0, 0, 0xC0, 0x17, 1, 0, b'!', 0x08];
    f2.extend_from_slice(&(rgce2.len() as u16).to_le_bytes());
    f2.extend_from_slice(&rgce2);
    rec(&mut sheet, 0x0006, &f2);
    rec(&mut sheet, 0x0207, &xl_str("Rice!"));
    let mut merge = 1u16.to_le_bytes().to_vec();
    merge.extend_from_slice(&[4, 0, 4, 0, 0, 0, 2, 0]);
    rec(&mut sheet, 0x00E5, &merge);
    rec(&mut sheet, 0x000A, &[]);

    let mut globals = vec![];
    rec(&mut globals, 0x0809, &[0, 6, 5, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0]);
    for k in 0..5 {
        rec(&mut globals, 0x0031, &font("Arial", k == 4));
    }
    let mut fmt = 164u16.to_le_bytes().to_vec();
    fmt.extend_from_slice(&xl_str("0.000"));
    rec(&mut globals, 0x041E, &fmt);
    rec(&mut globals, 0x00E0, &xf(0, 0, None));
    rec(&mut globals, 0x00E0, &xf(5, 0, Some(13)));
    rec(&mut globals, 0x00E0, &xf(0, 164, None));
    // SST: three strings, the third continued in a CONTINUE record.
    let mut sst = 3u32.to_le_bytes().to_vec();
    sst.extend_from_slice(&3u32.to_le_bytes());
    sst.extend_from_slice(&xl_str("Item"));
    sst.extend_from_slice(&xl_str("Qty"));
    sst.extend_from_slice(&[4, 0, 0, b'R', b'i']);
    // BOUNDSHEET's offset is patched once the globals' length is known.
    let mut bound = vec![0u8; 6];
    bound.extend_from_slice(&short_str("Groceries"));
    let fixed_len = globals.len() + (4 + sst.len()) + (4 + 3) + (4 + bound.len()) + 4;
    bound[..4].copy_from_slice(&(fixed_len as u32).to_le_bytes());
    rec(&mut globals, 0x0085, &bound);
    rec(&mut globals, 0x00FC, &sst);
    rec(&mut globals, 0x003C, &[0, b'c', b'e']);
    rec(&mut globals, 0x000A, &[]);
    assert_eq!(globals.len(), fixed_len);
    globals.extend_from_slice(&sheet);
    doc_cfb::build(&[("Workbook", &globals)])
}

#[test]
fn reads_a_biff8_workbook() {
    let wb = Workbook::open(workbook(), "groceries.xls", DateOrder::Dmy).unwrap();
    assert_eq!(wb.sheets.len(), 1);
    let s = &wb.sheets[0];
    assert_eq!(s.name, "Groceries");
    let v = |a: &str| s.value(CellRef::parse_a1(a).unwrap().0);
    assert_eq!(v("A1"), Value::text("Item"));
    assert_eq!(v("A2"), Value::text("Rice"));
    assert_eq!(v("B2"), Value::Number(10.0));
    assert_eq!(v("B3"), Value::Number(20.0));
    assert_eq!(v("A3"), Value::text("Rice!"));
    let f = s.cell(CellRef::parse_a1("B3").unwrap().0).unwrap().formula.as_ref().unwrap();
    assert_eq!(f.text, "B2*2");
    let f2 = s.cell(CellRef::parse_a1("A3").unwrap().0).unwrap().formula.as_ref().unwrap();
    assert_eq!(f2.text, "A2&\"!\"");
    let a1 = wb.styles.get(s.cell(CellRef::parse_a1("A1").unwrap().0).unwrap().style);
    assert!(a1.font.bold);
    assert_eq!(a1.fill.pattern, "solid");
    assert!(a1.border.bottom.is_some());
    let b3 = wb.styles.get(s.cell(CellRef::parse_a1("B3").unwrap().0).unwrap().style);
    assert_eq!(b3.num_fmt, "0.000");
    assert_eq!(s.merges.len(), 1);
    assert!((s.col_width(0) - 20.0).abs() < 0.01);
    assert!(wb.warnings.iter().any(|w| w.contains(".xlsx")));
}

#[test]
fn saves_as_xlsx() {
    let mut wb = Workbook::open(workbook(), "groceries.xls", DateOrder::Dmy).unwrap();
    let bytes = wb.to_bytes(crate::model::Format::Xlsx).unwrap();
    let back = Workbook::open(bytes, "groceries.xlsx", DateOrder::Dmy).unwrap();
    assert_eq!(back.sheets[0].value(CellRef::parse_a1("B3").unwrap().0), Value::Number(20.0));
    assert_eq!(back.sheets[0].cell(CellRef::parse_a1("B3").unwrap().0).unwrap().formula.as_ref().unwrap().text, "B2*2");
}
