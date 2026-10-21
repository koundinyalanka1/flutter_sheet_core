//! Functions pinned to the values Excel gives.

use std::collections::BTreeMap;

use super::super::ast::Expr;
use super::super::eval::{evaluate, Grid};
use super::super::parse;
use crate::numfmt::FormatOptions;
use crate::refs::{CellRef, Range};
use crate::value::{CellError, Value};

enum Entry {
    Value(Value),
    Formula(String, Expr),
}

/// A tiny workbook for tests: formulas are computed on demand.
struct TestGrid {
    sheets: Vec<(String, BTreeMap<CellRef, Entry>)>,
    names: Vec<(String, Expr)>,
    hidden_rows: Vec<u32>,
}

impl TestGrid {
    fn new() -> Self {
        TestGrid {
            sheets: vec![("Sheet1".into(), BTreeMap::new()), ("Data".into(), BTreeMap::new())],
            names: Vec::new(),
            hidden_rows: Vec::new(),
        }
    }

    fn set(&mut self, sheet: usize, a1: &str, input: &str) {
        let cell = CellRef::parse_a1(a1).unwrap().0;
        let entry = if let Some(f) = input.strip_prefix('=') {
            Entry::Formula(f.to_owned(), parse(f).unwrap_or_else(|e| panic!("{f}: {e}")))
        } else {
            Entry::Value(match crate::input::parse_input(input, &FormatOptions::default()) {
                crate::input::Input::Number(n, _) => Value::Number(n),
                crate::input::Input::Text(t) => Value::text(t),
                crate::input::Input::Bool(b) => Value::Bool(b),
                crate::input::Input::Error(e) => Value::Error(e),
                _ => Value::Empty,
            })
        };
        self.sheets[sheet].1.insert(cell, entry);
    }

    fn column(&mut self, sheet: usize, col: &str, start_row: u32, values: &[&str]) {
        for (i, v) in values.iter().enumerate() {
            self.set(sheet, &format!("{col}{}", start_row + i as u32), v);
        }
    }

    fn eval(&mut self, formula: &str) -> Value {
        let expr = parse(formula).unwrap_or_else(|e| panic!("{formula}: {e}"));
        evaluate(self, 0, CellRef::new(199, 25), &expr).0
    }
}

impl Grid for TestGrid {
    fn value(&mut self, sheet: usize, cell: CellRef) -> Value {
        let expr = match self.sheets.get(sheet).and_then(|s| s.1.get(&cell)) {
            None => return Value::Empty,
            Some(Entry::Value(v)) => return v.clone(),
            Some(Entry::Formula(_, e)) => e.clone(),
        };
        evaluate(self, sheet, cell, &expr).0
    }

    fn cells_in(&mut self, sheet: usize, range: Range) -> Vec<(CellRef, Value)> {
        let cells: Vec<CellRef> = match self.sheets.get(sheet) {
            Some(s) => s.1.keys().copied().filter(|c| range.contains(*c)).collect(),
            None => Vec::new(),
        };
        let mut out: Vec<(CellRef, Value)> = cells.into_iter().map(|c| (c, self.value(sheet, c))).collect();
        out.retain(|(_, v)| !v.is_empty());
        out
    }

    fn used_range(&self, sheet: usize) -> Option<Range> {
        let s = &self.sheets.get(sheet)?.1;
        let mut it = s.keys();
        let first = *it.next()?;
        Some(s.keys().fold(Range::cell(first), |r, c| r.union(Range::cell(*c))))
    }

    fn sheet_index(&self, name: &str) -> Option<usize> {
        self.sheets.iter().position(|s| s.0.eq_ignore_ascii_case(name))
    }

    fn sheet_count(&self) -> usize {
        self.sheets.len()
    }

    fn defined_name(&self, name: &str, _sheet: usize) -> Option<Expr> {
        self.names.iter().find(|n| n.0.eq_ignore_ascii_case(name)).map(|n| n.1.clone())
    }

    fn format_options(&self) -> FormatOptions {
        FormatOptions::default()
    }

    fn now(&self) -> f64 {
        46_308.75
    }

    fn random(&mut self) -> f64 {
        0.5
    }

    fn formula_text(&self, sheet: usize, cell: CellRef) -> Option<String> {
        match self.sheets.get(sheet)?.1.get(&cell)? {
            Entry::Formula(f, _) => Some(f.clone()),
            _ => None,
        }
    }

    fn row_hidden(&self, _sheet: usize, row: u32) -> bool {
        self.hidden_rows.contains(&row)
    }
}

fn num(v: Value) -> f64 {
    match v {
        Value::Number(n) => n,
        other => panic!("expected a number, got {other:?}"),
    }
}

fn close(a: f64, b: f64) -> bool {
    (a - b).abs() <= 1e-9 * b.abs().max(1.0)
}

fn check(g: &mut TestGrid, cases: &[(&str, Value)]) {
    for (f, expected) in cases {
        let got = g.eval(f);
        let ok = match (&got, expected) {
            (Value::Number(a), Value::Number(b)) => close(*a, *b),
            _ => &got == expected,
        };
        assert!(ok, "{f}: got {got:?}, expected {expected:?}");
    }
}

fn n(x: f64) -> Value {
    Value::Number(x)
}

fn t(s: &str) -> Value {
    Value::text(s)
}

fn b(x: bool) -> Value {
    Value::Bool(x)
}

fn e(x: CellError) -> Value {
    Value::Error(x)
}

fn sample() -> TestGrid {
    let mut g = TestGrid::new();
    // A small sales table.
    g.column(0, "A", 1, &["Item", "Apple", "Banana", "Cherry", "apple", "Date"]);
    g.column(0, "B", 1, &["Qty", "10", "20", "30", "5", "x"]);
    g.column(0, "C", 1, &["Price", "1.5", "0.25", "4", "2", "3"]);
    g.column(0, "D", 1, &["Region", "North", "South", "North", "South", "North"]);
    g.set(0, "E1", "=B2*C2");
    g.set(0, "E2", "=SUBTOTAL(9,B2:B4)");
    g.column(1, "A", 1, &["1", "2", "3", "4", "5"]);
    g
}

#[test]
fn arithmetic_and_logic() {
    let mut g = sample();
    check(
        &mut g,
        &[
            ("1+2*3", n(7.0)),
            ("-2^2", n(4.0)),
            ("2^3^2", n(64.0)),
            ("0.1+0.2-0.3", n(5.551115123125783e-17)),
            ("1/0", e(CellError::Div0)),
            ("\"a\"&1&TRUE", t("a1TRUE")),
            ("\"3\"+4", n(7.0)),
            ("\"x\"+1", e(CellError::Value)),
            ("IF(B2>5,\"big\",\"small\")", t("big")),
            ("IF(FALSE,1)", b(false)),
            ("IFS(B2>50,1,B2>5,2)", n(2.0)),
            ("IFS(B2>50,1)", e(CellError::NA)),
            ("IFERROR(1/0,\"oops\")", t("oops")),
            ("IFNA(NA(),0)", n(0.0)),
            ("IFNA(1/0,0)", e(CellError::Div0)),
            ("AND(TRUE,1,B2>5)", b(true)),
            ("OR(FALSE,0)", b(false)),
            ("XOR(TRUE,TRUE,TRUE)", b(true)),
            ("NOT(0)", b(true)),
            ("SWITCH(2,1,\"one\",2,\"two\",\"other\")", t("two")),
            ("SWITCH(9,1,\"one\",\"other\")", t("other")),
            ("SUM(IF(B2:B5>15,B2:B5,0))", n(50.0)),
            // Outside array formulas, cells compared as a range meet the
            // formula's row or column (Z200 meets neither): as Excel computes
            // a formula saved without array marking.
            ("A1:A3=\"item\"", e(CellError::Value)),
        ],
    );
}

#[test]
fn ranges_meet_the_formulas_row_or_column() {
    let mut g = sample();
    let at = |g: &mut TestGrid, cell: &str, f: &str| {
        let expr = parse(f).unwrap();
        evaluate(g, 0, CellRef::parse_a1(cell).unwrap().0, &expr).0
    };
    // In row 3, B:B*2 is B3*2; in column C, A2:D2+1 is C2+1.
    let b3 = num(g.eval("B3"));
    assert_eq!(at(&mut g, "Z3", "$B:$B*2"), n(b3 * 2.0));
    let c2 = g.eval("C2");
    let c2n = match c2 { Value::Number(x) => x, _ => 0.0 };
    assert_eq!(at(&mut g, "C40", "A2:D2+1"), n(c2n + 1.0));
    // A function of a range, applied to each cell, meets it too.
    assert_eq!(at(&mut g, "Z3", "ABS(-$B:$B)"), n(b3.abs()));
    // Aggregates still see every cell.
    assert_eq!(at(&mut g, "Z3", "SUM(B2:B5*2)"), n(2.0 * num(g.eval("SUM(B2:B5)"))));
}

#[test]
fn sums_counts_and_conditions() {
    let mut g = sample();
    check(
        &mut g,
        &[
            ("SUM(B2:B6)", n(65.0)),
            ("SUM(B:B)", n(65.0)),
            ("SUM(1,\"2\",TRUE)", n(4.0)),
            ("SUM(B2:B6,C2)", n(66.5)),
            ("SUMPRODUCT(B2:B5,C2:C5)", n(15.0 + 5.0 + 120.0 + 10.0)),
            ("SUMPRODUCT((D2:D6=\"North\")*B2:B6)", e(CellError::Value)),
            ("SUMPRODUCT((D2:D5=\"North\")*B2:B5)", n(40.0)),
            ("COUNT(B2:B6)", n(4.0)),
            ("COUNTA(A1:A6)", n(6.0)),
            ("COUNTBLANK(A1:A8)", n(2.0)),
            ("COUNTIF(A2:A6,\"apple\")", n(2.0)),
            ("COUNTIF(A2:A6,\"?a*\")", n(2.0)),
            ("COUNTIF(B2:B6,\">10\")", n(2.0)),
            ("COUNTIFS(D2:D6,\"North\",B2:B6,\">=10\")", n(2.0)),
            ("SUMIF(D2:D6,\"South\",B2:B6)", n(25.0)),
            ("SUMIF(D2:D6,\"South\",B2)", n(25.0)),
            ("SUMIF(B2:B6,\">10\")", n(50.0)),
            ("SUMIFS(B2:B6,D2:D6,\"North\",A2:A6,\"<>Cherry\")", n(10.0)),
            ("AVERAGEIF(D2:D6,\"North\",C2:C6)", n((1.5 + 4.0 + 3.0) / 3.0)),
            ("AVERAGEIFS(C2:C6,D2:D6,\"Nowhere\")", e(CellError::Div0)),
            ("MAXIFS(B2:B6,D2:D6,\"South\")", n(20.0)),
            ("MINIFS(B2:B6,D2:D6,\"North\")", n(10.0)),
            ("PRODUCT(2,3,4)", n(24.0)),
            ("SUMSQ(3,4)", n(25.0)),
            ("SUBTOTAL(9,B2:B6)", n(65.0)),
            ("SUBTOTAL(109,B2:B6)", n(65.0)),
            ("SUM(E1:E2)", n(15.0 + 60.0)),
            ("SUBTOTAL(9,E1:E2)", n(15.0)),
            ("AGGREGATE(14,6,C2:C6,2)", n(3.0)),
            ("AGGREGATE(9,0,B2:B6)", n(65.0)),
            ("SUM(Data!A1:A5)", n(15.0)),
            ("SUM(Sheet1:Data!A1)", n(1.0)),
        ],
    );
    g.hidden_rows.push(2); // row 3
    check(&mut g, &[("SUBTOTAL(109,B2:B6)", n(45.0)), ("SUBTOTAL(9,B2:B6)", n(65.0))]);
}

#[test]
fn statistics() {
    let mut g = sample();
    check(
        &mut g,
        &[
            ("AVERAGE(B2:B6)", n(16.25)),
            ("AVERAGEA(B2:B6)", n(13.0)),
            ("MAX(B2:B6)", n(30.0)),
            ("MIN(B2:B6)", n(5.0)),
            ("MAX(A1:A3)", n(0.0)),
            ("MEDIAN(1,2,3,4)", n(2.5)),
            ("MODE(1,2,2,3,3)", n(2.0)),
            ("MODE(1,2,3)", e(CellError::NA)),
            ("LARGE(B2:B6,2)", n(20.0)),
            ("SMALL(B2:B6,1)", n(5.0)),
            ("SMALL(B2:B6,9)", e(CellError::Num)),
            ("RANK(20,B2:B6)", n(2.0)),
            ("RANK(20,B2:B6,1)", n(3.0)),
            ("RANK.AVG(2,{1,2,2,3})", n(2.5)),
            ("PERCENTILE({1,2,3,4},0.25)", n(1.75)),
            ("PERCENTILE.EXC({1,2,3,4},0.25)", n(1.25)),
            ("QUARTILE({1,2,3,4,5},3)", n(4.0)),
            ("STDEV(2,4,4,4,5,5,7,9)", n(2.138_089_935_299_395)),
            ("STDEVP(2,4,4,4,5,5,7,9)", n(2.0)),
            ("VAR.S(1,2,3,4)", n(5.0 / 3.0)),
            ("VARP(1,2,3,4)", n(1.25)),
            ("AVEDEV(1,2,3,4)", n(1.0)),
            ("DEVSQ(1,2,3,4)", n(5.0)),
            ("GEOMEAN(2,8)", n(4.0)),
            ("HARMEAN(1,2,4)", n(3.0 / 1.75)),
            ("CORREL({1,2,3},{2,4,6})", n(1.0)),
            ("SLOPE({2,4,6},{1,2,3})", n(2.0)),
            ("INTERCEPT({3,5,7},{1,2,3})", n(1.0)),
            ("FORECAST(4,{3,5,7},{1,2,3})", n(9.0)),
            ("COVARIANCE.S({1,2,3},{1,2,3})", n(1.0)),
            ("PERCENTRANK({1,2,3,4,5},2)", n(0.25)),
            ("PERCENTRANK({1,2,3,4,5},2.5)", n(0.375)),
        ],
    );
}

#[test]
fn rounding_and_maths() {
    let mut g = sample();
    check(
        &mut g,
        &[
            ("ROUND(2.675,2)", n(2.68)),
            ("ROUND(-1.5,0)", n(-2.0)),
            ("ROUND(1234.5,-2)", n(1200.0)),
            ("ROUNDUP(1.21,1)", n(1.3)),
            ("ROUNDDOWN(-1.29,1)", n(-1.2)),
            ("TRUNC(8.9)", n(8.0)),
            ("INT(-8.9)", n(-9.0)),
            ("MROUND(10,3)", n(9.0)),
            ("MROUND(-10,3)", e(CellError::Num)),
            ("CEILING(2.5,1)", n(3.0)),
            ("CEILING(-2.5,-2)", n(-4.0)),
            ("CEILING(-2.5,2)", n(-2.0)),
            ("CEILING.MATH(-5.5,2,1)", n(-6.0)),
            ("FLOOR(-2.5,-2)", n(-2.0)),
            ("FLOOR(2.5,0)", e(CellError::Div0)),
            ("FLOOR.MATH(-5.5)", n(-6.0)),
            ("EVEN(1.5)", n(2.0)),
            ("EVEN(-3)", n(-4.0)),
            ("ODD(2)", n(3.0)),
            ("ODD(-1.5)", n(-3.0)),
            ("MOD(-3,2)", n(1.0)),
            ("MOD(3,-2)", n(-1.0)),
            ("QUOTIENT(-7,2)", n(-3.0)),
            ("POWER(2,10)", n(1024.0)),
            ("SQRT(-1)", e(CellError::Num)),
            ("LOG(8,2)", n(3.0)),
            ("LN(EXP(2))", n(2.0)),
            ("ABS(-3)", n(3.0)),
            ("SIGN(-0.5)", n(-1.0)),
            ("FACT(5)", n(120.0)),
            ("FACTDOUBLE(7)", n(105.0)),
            ("COMBIN(8,2)", n(28.0)),
            ("PERMUT(5,2)", n(20.0)),
            ("GCD(24,36)", n(12.0)),
            ("LCM(4,6)", n(12.0)),
            ("DEGREES(PI())", n(180.0)),
            ("ATAN2(1,1)", n(std::f64::consts::FRAC_PI_4)),
            ("RANDBETWEEN(1,10)", n(6.0)),
            ("SUMPRODUCT(ABS({-1,2,-3}))", n(6.0)),
            ("SUMX2MY2({2,3},{1,1})", n(11.0)),
        ],
    );
}

#[test]
fn lookups() {
    let mut g = sample();
    g.column(0, "G", 1, &["0", "1000", "5000", "10000"]);
    g.column(0, "H", 1, &["0%", "5%", "10%", "20%"]);
    check(
        &mut g,
        &[
            ("VLOOKUP(\"cherry\",A2:C6,3,FALSE)", n(4.0)),
            ("VLOOKUP(\"ch*\",A2:C6,2,FALSE)", n(30.0)),
            ("VLOOKUP(\"zzz\",A2:C6,2,FALSE)", e(CellError::NA)),
            ("VLOOKUP(\"Apple\",A2:C6,4,FALSE)", e(CellError::Ref)),
            ("VLOOKUP(7500,G1:H4,2)", n(0.1)),
            ("VLOOKUP(7500,G1:H4,2,TRUE)", n(0.1)),
            ("VLOOKUP(-1,G1:H4,2)", e(CellError::NA)),
            ("VLOOKUP(\"Apple\",A:D,4,FALSE)", t("North")),
            ("HLOOKUP(\"Price\",A1:D6,3,FALSE)", n(0.25)),
            ("MATCH(\"Banana\",A1:A6,0)", n(3.0)),
            ("MATCH(5000,G1:G4)", n(3.0)),
            ("MATCH(6000,G1:G4,1)", n(3.0)),
            ("MATCH(25,{40,30,20,10},-1)", n(2.0)),
            ("INDEX(A1:D6,3,2)", n(20.0)),
            ("INDEX(A1:A6,4)", t("Cherry")),
            ("INDEX(A1:D1,3)", t("Price")),
            ("SUM(INDEX(B2:C6,0,1))", n(65.0)),
            ("SUM(B2:INDEX(B2:B6,3))", n(60.0)),
            ("INDEX(A1:D6,MATCH(\"Cherry\",A1:A6,0),MATCH(\"Region\",A1:D1,0))", t("North")),
            ("XLOOKUP(\"Date\",A2:A6,C2:C6)", n(3.0)),
            ("XLOOKUP(\"Fig\",A2:A6,C2:C6,\"none\")", t("none")),
            ("XLOOKUP(\"apple\",A2:A6,B2:B6,,0,-1)", n(5.0)),
            ("XLOOKUP(7500,G1:G4,H1:H4,,-1)", n(0.1)),
            ("XLOOKUP(7500,G1:G4,H1:H4,,1)", n(0.2)),
            ("XMATCH(\"b*\",A2:A6,2)", n(2.0)),
            ("XMATCH(4000,G1:G4,1)", n(3.0)),
            ("LOOKUP(7500,G1:G4,H1:H4)", n(0.1)),
            ("LOOKUP(3,{1,2,3;\"a\",\"b\",\"c\"})", t("c")),
            ("CHOOSE(2,\"a\",\"b\",\"c\")", t("b")),
            ("SUM(CHOOSE(1,B2:B3,C2:C3))", n(30.0)),
            ("ROW(C5)", n(5.0)),
            ("COLUMN(C5)", n(3.0)),
            ("ROW()", n(200.0)),
            ("ROWS(A1:D6)", n(6.0)),
            ("COLUMNS(A1:D6)", n(4.0)),
            ("SUM(OFFSET(A1,1,1,2,1))", n(30.0)),
            ("OFFSET(A1,2,0)", t("Banana")),
            ("INDIRECT(\"B3\")", n(20.0)),
            ("INDIRECT(\"Data!A2\")", n(2.0)),
            ("SUM(INDIRECT(\"B2:B3\"))", n(30.0)),
            ("INDIRECT(\"R3C2\",FALSE)", n(20.0)),
            ("ADDRESS(2,3)", t("$C$2")),
            ("ADDRESS(2,3,4)", t("C2")),
            ("ADDRESS(2,3,2,FALSE)", t("R2C[3]")),
            ("ADDRESS(1,1,1,TRUE,\"My Sheet\")", t("'My Sheet'!$A$1")),
            ("SUM(TRANSPOSE({1,2,3}))", n(6.0)),
            ("ROWS(TRANSPOSE({1,2,3}))", n(3.0)),
            ("AREAS((A1,B2:C3))", n(2.0)),
            ("SUMPRODUCT(ROW(A1:A3))", n(6.0)),
        ],
    );
}

#[test]
fn text_functions() {
    let mut g = sample();
    check(
        &mut g,
        &[
            ("LEN(\"héllo\")", n(5.0)),
            ("LEFT(\"Excel\",2)", t("Ex")),
            ("LEFT(\"Excel\")", t("E")),
            ("RIGHT(\"Excel\",3)", t("cel")),
            ("MID(\"Excel\",2,3)", t("xce")),
            ("MID(\"Excel\",0,3)", e(CellError::Value)),
            ("UPPER(\"abc\")", t("ABC")),
            ("PROPER(\"ram kumar\")", t("Ram Kumar")),
            ("TRIM(\"  a   b  \")", t("a b")),
            ("CONCAT(A2:A3,\"!\")", t("AppleBanana!")),
            ("CONCATENATE(\"a\",1,TRUE)", t("a1TRUE")),
            ("TEXTJOIN(\", \",TRUE,A2:A4,\"\",\"x\")", t("Apple, Banana, Cherry, x")),
            ("TEXTJOIN(\"-\",FALSE,\"a\",\"\",\"b\")", t("a--b")),
            ("REPT(\"ab\",3)", t("ababab")),
            ("EXACT(\"a\",\"A\")", b(false)),
            ("FIND(\"l\",\"Hello\")", n(3.0)),
            ("FIND(\"L\",\"Hello\")", e(CellError::Value)),
            ("SEARCH(\"L\",\"Hello\")", n(3.0)),
            ("SEARCH(\"l?o\",\"Hello\")", n(3.0)),
            ("SUBSTITUTE(\"a-b-c\",\"-\",\"/\")", t("a/b/c")),
            ("SUBSTITUTE(\"a-b-c\",\"-\",\"/\",2)", t("a-b/c")),
            ("REPLACE(\"abcdef\",2,3,\"X\")", t("aXef")),
            ("CHAR(65)", t("A")),
            ("CODE(\"A\")", n(65.0)),
            ("UNICHAR(8377)", t("₹")),
            ("UNICODE(\"₹\")", n(8377.0)),
            ("VALUE(\"1,234.5\")", n(1234.5)),
            ("VALUE(\"12%\")", n(0.12)),
            ("VALUE(\"abc\")", e(CellError::Value)),
            ("NUMBERVALUE(\"1.234,5\",\",\",\".\")", n(1234.5)),
            ("TEXT(1234.567,\"#,##0.00\")", t("1,234.57")),
            ("TEXT(0.25,\"0%\")", t("25%")),
            ("TEXT(46308,\"dd-mmm-yyyy\")", t("13-Oct-2026")),
            ("TEXT(\"46308\",\"yyyy\")", t("2026")),
            ("FIXED(1234.567,1)", t("1,234.6")),
            ("FIXED(1234.567,1,TRUE)", t("1234.6")),
            ("FIXED(-1234.567)", t("-1,234.57")),
            ("DOLLAR(-12.5)", t("($12.50)")),
            ("T(1)", t("")),
            ("T(\"x\")", t("x")),
            ("TEXTBEFORE(\"a-b-c\",\"-\")", t("a")),
            ("TEXTBEFORE(\"a-b-c\",\"-\",-1)", t("a-b")),
            ("TEXTAFTER(\"a-b-c\",\"-\",2)", t("c")),
            ("TEXTAFTER(\"a-b-c\",\"x\")", e(CellError::NA)),
            ("TEXTAFTER(\"a-B-c\",\"b\",1,1)", t("-c")),
            ("CLEAN(CHAR(9)&\"a\")", t("a")),
            ("SUMPRODUCT(LEN(A2:A4))", n(17.0)),
        ],
    );
}

#[test]
fn dates_and_times() {
    let mut g = sample();
    check(
        &mut g,
        &[
            ("DATE(2026,10,13)", n(46_308.0)),
            ("DATE(2026,13,1)", n(46_388.0)),
            ("DATE(2026,1,0)", n(46_022.0)),
            ("DATE(1900,2,29)", n(60.0)),
            ("YEAR(46308)", n(2026.0)),
            ("MONTH(46308)", n(10.0)),
            ("DAY(46308)", n(13.0)),
            ("YEAR(\"2026-10-13\")", n(2026.0)),
            ("HOUR(0.75)", n(18.0)),
            ("MINUTE(TIME(10,45,30))", n(45.0)),
            ("SECOND(TIME(10,45,30))", n(30.0)),
            ("TIME(25,0,0)", n(1.0 / 24.0)),
            ("TODAY()", n(46_308.0)),
            ("NOW()", n(46_308.75)),
            ("WEEKDAY(46308)", n(3.0)),
            ("WEEKDAY(46308,2)", n(2.0)),
            ("WEEKDAY(46308,3)", n(1.0)),
            ("WEEKNUM(DATE(2026,1,1))", n(1.0)),
            ("WEEKNUM(DATE(2026,10,13))", n(42.0)),
            ("WEEKNUM(DATE(2026,10,13),2)", n(42.0)),
            ("ISOWEEKNUM(DATE(2026,10,13))", n(42.0)),
            ("ISOWEEKNUM(DATE(2027,1,1))", n(53.0)),
            ("EDATE(DATE(2026,1,31),1)", n(46_081.0)),
            ("EOMONTH(DATE(2026,10,13),0)", n(46_326.0)),
            ("EOMONTH(DATE(2026,10,13),-1)", n(46_295.0)),
            ("DATEDIF(DATE(2000,5,20),DATE(2026,10,13),\"Y\")", n(26.0)),
            ("DATEDIF(DATE(2026,1,31),DATE(2026,3,1),\"M\")", n(1.0)),
            ("DATEDIF(DATE(2026,1,15),DATE(2026,3,10),\"MD\")", n(23.0)),
            ("DATEDIF(DATE(2025,11,20),DATE(2026,10,13),\"YM\")", n(10.0)),
            ("DATEDIF(DATE(2025,11,20),DATE(2026,10,13),\"YD\")", n(327.0)),
            ("DATEDIF(DATE(2026,2,1),DATE(2026,1,1),\"D\")", e(CellError::Num)),
            ("DAYS(DATE(2026,12,25),DATE(2026,10,13))", n(73.0)),
            ("DAYS360(DATE(2026,1,30),DATE(2026,2,28))", n(28.0)),
            ("DAYS360(DATE(2026,1,31),DATE(2026,3,31))", n(60.0)),
            ("NETWORKDAYS(DATE(2026,10,1),DATE(2026,10,31))", n(22.0)),
            ("NETWORKDAYS(DATE(2026,10,1),DATE(2026,10,31),{\"2026-10-02\",\"2026-10-20\"})", n(20.0)),
            ("NETWORKDAYS.INTL(DATE(2026,10,1),DATE(2026,10,31),11)", n(27.0)),
            ("NETWORKDAYS.INTL(DATE(2026,10,1),DATE(2026,10,31),\"0000011\")", n(22.0)),
            ("WORKDAY(DATE(2026,10,16),1)", n(46_314.0)),
            ("WORKDAY(DATE(2026,10,19),-1)", n(46_311.0)),
            ("YEARFRAC(DATE(2026,1,1),DATE(2026,7,1))", n(0.5)),
            ("YEARFRAC(DATE(2026,1,1),DATE(2026,7,1),1)", n(181.0 / 365.0)),
            ("YEARFRAC(DATE(2024,1,1),DATE(2024,7,1),1)", n(182.0 / 366.0)),
            ("YEARFRAC(DATE(2026,1,1),DATE(2026,7,1),3)", n(181.0 / 365.0)),
            ("DATEVALUE(\"13/10/2026\")", n(46_308.0)),
            ("TIMEVALUE(\"18:00\")", n(0.75)),
        ],
    );
}

#[test]
fn information_and_finance() {
    let mut g = sample();
    check(
        &mut g,
        &[
            ("ISBLANK(Z1)", b(true)),
            ("ISBLANK(A1)", b(false)),
            ("ISNUMBER(B2)", b(true)),
            ("ISTEXT(A2)", b(true)),
            ("ISERROR(1/0)", b(true)),
            ("ISERR(NA())", b(false)),
            ("ISNA(NA())", b(true)),
            ("ISEVEN(4)", b(true)),
            ("ISODD(-3)", b(true)),
            ("ISLOGICAL(TRUE)", b(true)),
            ("ISREF(A1)", b(true)),
            ("ISFORMULA(E1)", b(true)),
            ("FORMULATEXT(E1)", t("=B2*C2")),
            ("N(TRUE)", n(1.0)),
            ("N(\"x\")", n(0.0)),
            ("TYPE(\"x\")", n(2.0)),
            ("TYPE({1,2})", n(64.0)),
            ("ERROR.TYPE(1/0)", n(2.0)),
            ("SUMPRODUCT(--ISNUMBER(B2:B6))", n(4.0)),
            ("PMT(0.09/12,240,1000000)", n(-8997.259_558_501_704)),
            ("FV(0.06/12,10,-200,-500,1)", n(2581.403_374_060_136)),
            ("ROUND(RATE(48,-200,8000),6)", n(0.007_701)),
            ("ROUND(IRR({-70000,12000,15000,18000,21000,26000}),6)", n(0.086_631)),
            ("ROUND(NPV(0.1,-10000,3000,4200,6800),2)", n(1188.44)),
            ("ROUND(XNPV(0.09,{-10000,2750,4250,3250,2750},{39448,39508,39751,39859,39904}),2)", n(2086.65)),
            ("ROUND(XIRR({-10000,2750,4250,3250,2750},{39448,39508,39751,39859,39904}),6)", n(0.373_363)),
            ("SLN(30000,7500,10)", n(2250.0)),
            ("SYD(30000,7500,10,10)", n(409.090_909_090_909_1)),
            ("ROUND(EFFECT(0.0525,4),6)", n(0.053_543)),
            ("ROUND(NOMINAL(0.053543,4),5)", n(0.0525)),
            ("ROUND(CUMIPMT(0.09/12,360,125000,13,24,0),2)", n(-11135.23)),
            ("ROUND(CUMPRINC(0.09/12,360,125000,13,24,0),2)", n(-934.11)),
            ("ROUND(PDURATION(0.025,2000,2200),4)", n(3.8599)),
            ("ROUND(RRI(96,10000,11000),6)", n(0.000_993)),
        ],
    );
}

#[test]
fn names_and_unknowns() {
    let mut g = sample();
    g.names.push(("TaxRate".into(), parse("0.18").unwrap()));
    g.names.push(("Items".into(), parse("Sheet1!$A$2:$A$6").unwrap()));
    check(
        &mut g,
        &[
            ("100*TaxRate", n(18.0)),
            ("COUNTA(Items)", n(5.0)),
            ("Nope*2", e(CellError::Name)),
        ],
    );
    let expr = parse("FOOBAR(1)+1").unwrap();
    let (v, unsupported, _) = evaluate(&mut g, 0, CellRef::new(0, 0), &expr);
    assert_eq!(v, e(CellError::Name));
    assert!(unsupported);
    let expr = parse("NOW()").unwrap();
    assert!(evaluate(&mut g, 0, CellRef::new(0, 0), &expr).2);
    assert!(super::is_known("XLOOKUP"));
    assert!(super::names().len() > 200);
    let _ = num(n(1.0));
}

#[test]
fn broadcasting_past_the_array_budget_is_num() {
    // A column broadcast against a row multiplies their sizes: whole ones
    // asked for 393 GB. Past 4M values the result is #NUM!.
    let mut g = TestGrid::new();
    g.set(0, "A1", "2");
    assert_eq!(g.eval("SUM(A1:A10000*A1:ZZ1)"), Value::Error(CellError::Num));
    assert_eq!(g.eval("SUM(IF(A1:A10000,1,A1:ZZ1))"), Value::Error(CellError::Num));
    assert_eq!(g.eval("SUM(ROUND(A1:A10000,A1:ZZ1))"), Value::Error(CellError::Num));
    // Within it, broadcasting works as before.
    assert_eq!(g.eval("SUM(A1:A3*A1:C1)"), Value::Number(4.0));
    assert_eq!(g.eval("SUM(A1:A2000*A1:B1)"), Value::Number(4.0));
}

#[test]
fn loops_sized_by_a_cell_end_at_once() {
    // Each of these looped as many times as an argument said: 1e15 never
    // finished. Past the guards the results overflow (or are absurd), so
    // they are #NUM!.
    let mut g = TestGrid::new();
    let num = Value::Error(CellError::Num);
    for f in [
        "FACTDOUBLE(1E15)",
        "COMBIN(1E15,5E14)",
        "COMBINA(1E15,5E14)",
        "PERMUT(1E15,1E14)",
        "DDB(1000,0,1E15,1E15)",
        "CUMIPMT(0.01,1E15,1000,1,1E15,0)",
        "CUMPRINC(0.01,1E15,1000,1,1E15,0)",
    ] {
        let started = std::time::Instant::now();
        assert_eq!(g.eval(f), num, "{f}");
        assert!(started.elapsed().as_millis() < 500, "{f}");
    }
    assert_eq!(g.eval("FACTDOUBLE(6)"), Value::Number(48.0));
    assert_eq!(g.eval("COMBIN(10,3)"), Value::Number(120.0));
    assert_eq!(g.eval("PERMUT(5,2)"), Value::Number(20.0));
    assert_eq!(g.eval("COMBIN(2000,1000)"), num);
    // SUBSTITUTE knows its length before building: 30,000 matches of 30,000
    // characters each would have been 900 million.
    assert_eq!(g.eval("SUBSTITUTE(REPT(\"a\",30000),\"a\",REPT(\"b\",30000))"), Value::Error(CellError::Value));
    assert_eq!(g.eval("SUBSTITUTE(\"banana\",\"a\",\"o\")"), Value::text("bonono"));
}

#[test]
fn concatenation_stops_at_excels_text_limit() {
    let mut g = TestGrid::new();
    assert_eq!(g.eval("REPT(\"a\",30000)&REPT(\"b\",30000)"), Value::Error(CellError::Value));
    assert_eq!(g.eval("LEN(REPT(\"a\",30000)&REPT(\"b\",2767))"), Value::Number(32767.0));
    assert_eq!(g.eval("\"a\"&1&TRUE"), Value::text("a1TRUE"));
}
