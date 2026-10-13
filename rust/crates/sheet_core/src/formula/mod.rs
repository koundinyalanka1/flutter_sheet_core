//! Formulas: parsing, printing, keeping references right as sheets change,
//! and evaluation.

pub mod adjust;
pub mod ast;
pub mod parser;
pub mod unparse;

// In progress (see PLAN.md, phase 3): the evaluator (eval.rs) and the
// function library (functions/, whose per-category modules are not written
// yet) join the build once the library is complete.
// pub mod eval;
// pub mod functions;

pub use ast::{Area, BinaryOp, CellAddr, Expr, Reference, UnaryOp};
pub use parser::{parse, ParseError};
pub use unparse::{unparse, unparse_ooxml};

#[cfg(test)]
mod tests {
    use super::adjust::{offset_references, rename_sheet, shift_references, invalidate_sheet, Shift};
    use super::*;

    fn round_trip(formula: &str) -> String {
        unparse(&parse(formula).unwrap_or_else(|e| panic!("{formula}: {e}")))
    }

    #[test]
    fn round_trips_typical_formulas() {
        for f in [
            "SUM(A1:A10)",
            "A1+B1*C1",
            "(A1+B1)*C1",
            "IF(A1>=10,\"big\",\"small\")",
            "VLOOKUP($B2,Rates!$A$2:$C$100,3,FALSE)",
            "'My Sheet'!A1+'Bob''s'!B2",
            "SUM(Sheet1:Sheet3!B5)",
            "SUM(A:A)",
            "SUM($A:$C)",
            "SUM(2:2)",
            "SUM($3:$5)",
            "-A1^2",
            "50%",
            "A1&\" \"&B1",
            "{1,2;3,4}",
            "SUM({1,-2,3.5})",
            "IF(A1,,1)",
            "NOW()",
            "#N/A",
            "ISERROR(#DIV/0!)",
            "SUM(A1:INDEX(B:B,5))",
            "SUM((A1:A3,C1:C3))",
            "A1:B5 B2:C6",
            "Table1[Amount]",
            "SUM(Table1[[#This Row],[Qty]])",
            "TaxRate*A1",
            "Sheet2!TaxRate",
            "1E+20+1E-10",
            "\"say \"\"hi\"\"\"",
            "STDEV.S(A1:A9)",
            "#REF!+1",
            "Sheet1!#REF!",
        ] {
            assert_eq!(round_trip(f), f, "{f}");
        }
    }

    #[test]
    fn normalizes_harmless_differences() {
        assert_eq!(round_trip(" sum( a1 , b2 ) "), "SUM(A1,B2)");
        assert_eq!(round_trip("_xlfn.XLOOKUP(A1,B:B,C:C)"), "XLOOKUP(A1,B:B,C:C)");
        assert_eq!(round_trip("true"), "TRUE");
        assert_eq!(round_trip("1.50"), "1.5");
    }

    #[test]
    fn precedence_matches_excel() {
        // -2^2 is (-2)^2 in Excel.
        let e = parse("-2^2").unwrap();
        assert!(matches!(e, Expr::Binary(BinaryOp::Pow, ref l, _) if matches!(**l, Expr::Unary(UnaryOp::Neg, _))));
        // ^ is left-associative: 2^3^2 = (2^3)^2.
        let e = parse("2^3^2").unwrap();
        assert!(matches!(e, Expr::Binary(BinaryOp::Pow, ref l, _) if matches!(**l, Expr::Binary(BinaryOp::Pow, _, _))));
        // Comparison binds looser than &.
        let e = parse("A1&B1=C1").unwrap();
        assert!(matches!(e, Expr::Binary(BinaryOp::Eq, _, _)));
        // Percent binds tighter than ^.
        let e = parse("2^50%").unwrap();
        assert!(matches!(e, Expr::Binary(BinaryOp::Pow, _, ref r) if matches!(**r, Expr::Percent(_))));
    }

    #[test]
    fn rejects_bad_formulas() {
        for bad in ["", "SUM(A1", "1+", "\"open", "{1,2", "A1 B1 +", ")", "SUM(A1,,", "{1,2;3}", "{A1}", "1 2"] {
            assert!(parse(bad).is_err(), "{bad:?} should not parse");
        }
    }

    #[test]
    fn ooxml_prefixes_newer_functions() {
        let e = parse("IFERROR(XLOOKUP(A1,B:B,C:C),CONCAT(D1,TEXTJOIN(\",\",TRUE,E1:E3)))").unwrap();
        assert_eq!(
            unparse_ooxml(&e),
            "IFERROR(_xlfn.XLOOKUP(A1,B:B,C:C),_xlfn.CONCAT(D1,_xlfn.TEXTJOIN(\",\",TRUE,E1:E3)))"
        );
        assert_eq!(unparse_ooxml(&parse("STDEV.S(A1:A3)").unwrap()), "_xlfn.STDEV.S(A1:A3)");
        assert_eq!(unparse_ooxml(&parse("SUM(A1)").unwrap()), "SUM(A1)");
    }

    fn shifted(formula: &str, shift: Shift) -> String {
        let mut e = parse(formula).unwrap();
        shift_references(&mut e, "Sheet1", "Sheet1", shift);
        unparse(&e)
    }

    #[test]
    fn inserting_and_deleting_rows_moves_references() {
        let insert = Shift::InsertRows { at: 4, count: 2 }; // before row 5
        assert_eq!(shifted("A10", insert), "A12");
        assert_eq!(shifted("$A$10", insert), "$A$12");
        assert_eq!(shifted("A3", insert), "A3");
        assert_eq!(shifted("SUM(A1:A10)", insert), "SUM(A1:A12)");
        assert_eq!(shifted("SUM(A5:A10)", insert), "SUM(A7:A12)");
        assert_eq!(shifted("SUM(B:B)", insert), "SUM(B:B)");
        assert_eq!(shifted("SUM(3:6)", insert), "SUM(3:8)");

        let delete = Shift::DeleteRows { at: 4, count: 2 }; // rows 5-6
        assert_eq!(shifted("A10", delete), "A8");
        assert_eq!(shifted("A5", delete), "#REF!");
        assert_eq!(shifted("SUM(A1:A10)", delete), "SUM(A1:A8)");
        assert_eq!(shifted("SUM(A5:A6)", delete), "SUM(#REF!)");
        assert_eq!(shifted("SUM(A6:A10)", delete), "SUM(A5:A8)");
        assert_eq!(shifted("SUM(A2:A5)", delete), "SUM(A2:A4)");

        let cols = Shift::InsertCols { at: 1, count: 1 };
        assert_eq!(shifted("B2+$C$3", cols), "C2+$D$3");
        assert_eq!(shifted("SUM(A:C)", cols), "SUM(A:D)");
        assert_eq!(shifted("SUM(1:1)", cols), "SUM(1:1)");

        // Other sheets' references are left alone.
        let mut e = parse("Sheet2!A10+A10").unwrap();
        shift_references(&mut e, "Sheet1", "Sheet1", insert);
        assert_eq!(unparse(&e), "Sheet2!A10+A12");
        let mut e = parse("Sheet2!A10+A10").unwrap();
        shift_references(&mut e, "Sheet1", "Sheet2", insert);
        assert_eq!(unparse(&e), "Sheet2!A12+A10");
    }

    #[test]
    fn copying_moves_relative_references_only() {
        let mut e = parse("A1+$B$1+C$1+$D1").unwrap();
        offset_references(&mut e, 2, 1);
        assert_eq!(unparse(&e), "B3+$B$1+D$1+$D3");
        let mut e = parse("A1").unwrap();
        offset_references(&mut e, -1, 0);
        assert_eq!(unparse(&e), "#REF!");
        let mut e = parse("SUM(A:A)").unwrap();
        offset_references(&mut e, 5, 1);
        assert_eq!(unparse(&e), "SUM(B:B)");
    }

    #[test]
    fn sheet_renames_and_deletions() {
        let mut e = parse("Data!A1+'Old Name'!B2").unwrap();
        rename_sheet(&mut e, "old name", "New Name");
        assert_eq!(unparse(&e), "Data!A1+'New Name'!B2");
        invalidate_sheet(&mut e, "Data");
        assert_eq!(unparse(&e), "Data!#REF!+'New Name'!B2");
    }
}
