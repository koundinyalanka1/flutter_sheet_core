use super::*;
use crate::value::Value;

fn fmt(code: &str, value: f64) -> String {
    format_value(&Value::Number(value), code, &FormatOptions::default()).text
}

fn fmt_text(code: &str, text: &str) -> String {
    format_value(&Value::text(text), code, &FormatOptions::default()).text
}

#[test]
fn general() {
    let cases = [
        (0.0, "0"),
        (1234.5, "1234.5"),
        (-5.25, "-5.25"),
        (1.0 / 3.0, "0.333333333"),
        (2.0 / 3.0, "0.666666667"),
        (12_345_678_901.0, "12345678901"),
        (123_456_789_012.0, "1.23457E+11"),
        (1e20, "1E+20"),
        (0.00001, "0.00001"),
        (1e-12, "1E-12"),
        (100.0, "100"),
    ];
    for (n, expected) in cases {
        assert_eq!(fmt("General", n), expected, "{n}");
    }
}

#[test]
fn digit_placeholders_and_rounding() {
    let cases = [
        ("0", 1234.5, "1235"),
        ("0", 2.5, "3"),
        ("0", -2.5, "-3"),
        ("0.00", 2.675, "2.68"),
        ("0.00", 1.005, "1.01"),
        ("0.00", -2.675, "-2.68"),
        ("0.00", -0.001, "0.00"),
        ("000", 5.0, "005"),
        ("#", 0.0, ""),
        ("0.##", 1.5, "1.5"),
        ("0.##", 2.0, "2."),
        ("#.00", 0.5, ".50"),
        ("0.0?", 1.5, "1.5 "),
        ("?.??", 1.5, "1.5 "),
        ("00.000", 3.14159, "03.142"),
        ("0", 12345678901234.0, "12345678901234"),
    ];
    for (code, n, expected) in cases {
        assert_eq!(fmt(code, n), expected, "{code} {n}");
    }
}

#[test]
fn thousands_and_scaling() {
    let cases = [
        ("#,##0", 1_234_567.0, "1,234,567"),
        ("#,##0", 999.0, "999"),
        ("#,##0", 0.0, "0"),
        ("#,##0.00", 1234.5, "1,234.50"),
        ("#,##0", -1234.0, "-1,234"),
        ("#,##0,", 1_234_567.0, "1,235"),
        ("0.0,,", 123_456_789.0, "123.5"),
        ("#,##0.0,,\"M\"", 123_456_789.0, "123.5M"),
        ("0,000", 1234.0, "1,234"),
    ];
    for (code, n, expected) in cases {
        assert_eq!(fmt(code, n), expected, "{code} {n}");
    }
}

#[test]
fn percent_scientific_literals() {
    let cases = [
        ("0%", 0.256, "26%"),
        ("0.0%", 0.256, "25.6%"),
        ("0.00E+00", 12345.0, "1.23E+04"),
        ("0.00E+00", 0.000_123_45, "1.23E-04"),
        ("0.0E-0", 12345.0, "1.2E4"),
        ("0.00E+00", 9.999, "1.00E+01"),
        ("##0.0E+0", 12345.0, "12.3E+3"),
        ("\"$\"#,##0.00", 1234.5, "$1,234.50"),
        ("$#,##0.00", 1234.5, "$1,234.50"),
        ("[$₹-4009]#,##0.00", 99.0, "₹99.00"),
        ("000-00-0000", 123_456_789.0, "123-45-6789"),
        ("0\" units\"", 5.0, "5 units"),
        ("0_);(0)", 5.0, "5 "),
        ("\\(0\\)", 5.0, "(5)"),
    ];
    for (code, n, expected) in cases {
        assert_eq!(fmt(code, n), expected, "{code} {n}");
    }
}

#[test]
fn sections_and_conditions() {
    assert_eq!(fmt("0;(0)", -5.0), "(5)");
    assert_eq!(fmt("0;(0)", 5.0), "5");
    assert_eq!(fmt("0;-0;\"zero\"", 0.0), "zero");
    assert_eq!(fmt("0.00;[Red]-0.00", 3.0), "3.00");
    let red = format_value(&Value::Number(-3.0), "0.00;[Red]-0.00", &FormatOptions::default());
    assert_eq!(red.text, "-3.00");
    assert_eq!(red.color, Some(0xFF0000));
    assert_eq!(fmt("[Blue]0", 1.0), "1");
    assert_eq!(fmt("[>=100]\"big\";[<0]\"neg\";\"small\"", 150.0), "big");
    assert_eq!(fmt("[>=100]\"big\";[<0]\"neg\";\"small\"", -1.0), "neg");
    assert_eq!(fmt("[>=100]\"big\";[<0]\"neg\";\"small\"", 5.0), "small");
    // An empty section shows nothing.
    assert_eq!(fmt("0;;0", -5.0), "");
}

#[test]
fn indian_grouping() {
    assert_eq!(fmt(INDIAN_NUMBER_FORMAT, 123_456_789.5), "12,34,56,789.50");
    assert_eq!(fmt(INDIAN_NUMBER_FORMAT, 1_234_567.0), "12,34,567.00");
    assert_eq!(fmt(INDIAN_NUMBER_FORMAT, 100_000.0), "1,00,000.00");
    assert_eq!(fmt(INDIAN_NUMBER_FORMAT, 12_345.0), "12,345.00");
    assert_eq!(fmt(INDIAN_NUMBER_FORMAT, 5.0), "5.00");
    assert_eq!(fmt(INDIAN_RUPEE_FORMAT, 250_000.0), "₹2,50,000.00");
    assert_eq!(fmt(INDIAN_NUMBER_FORMAT, -1_234_567.0), "-1,234,567.00");
}

#[test]
fn fractions() {
    assert_eq!(fmt("# ?/?", 1.25), "1 1/4");
    assert_eq!(fmt("# ??/??", 3.14159), "3 14/99");
    assert_eq!(fmt("?/?", 0.75), "3/4");
    assert_eq!(fmt("# ?/8", 1.3), "1 2/8");
    assert_eq!(fmt("# ?/?", 2.0), "2    ");
    assert_eq!(fmt("?/?", 1.5), "3/2");
}

#[test]
fn text_sections() {
    assert_eq!(fmt_text("@", "abc"), "abc");
    assert_eq!(fmt_text("\"Name: \"@", "Bob"), "Name: Bob");
    assert_eq!(fmt_text("0;0;0;\"[\"@\"]\"", "x"), "[x]");
    assert_eq!(fmt_text("0.00", "plain"), "plain");
    assert_eq!(
        format_value(&Value::Bool(true), "0.00", &FormatOptions::default()).text,
        "TRUE"
    );
}

#[test]
fn dates_and_times() {
    let day = 46_308.0; // 2026-10-13, a Tuesday
    let cases = [
        ("yyyy-mm-dd", day, "2026-10-13"),
        ("d-mmm-yy", day, "13-Oct-26"),
        ("dddd, mmmm d, yyyy", day, "Tuesday, October 13, 2026"),
        ("ddd dd/mm/yy", day, "Tue 13/10/26"),
        ("m/d/yyyy", day, "10/13/2026"),
        ("mmmmm", day, "O"),
        ("h:mm AM/PM", 0.75, "6:00 PM"),
        ("h:mm am/pm", 0.25, "6:00 am"),
        ("h:mm:ss A/P", 0.5, "12:00:00 P"),
        ("hh:mm", 1.0 / 24.0, "01:00"),
        ("h:mm:ss", 0.5 + 65.0 / 86_400.0, "12:01:05"),
        ("[h]:mm:ss", 1.5, "36:00:00"),
        ("[mm]:ss", 2.0 / 24.0, "120:00"),
        ("mm:ss.0", 65.25 / 86_400.0, "01:05.3"),
        ("h:mm", 0.999_999_9, "0:00"),
        ("yyyy-mm-dd hh:mm", day + 0.5, "2026-10-13 12:00"),
        ("mm/dd/yyyy", 60.0, "02/29/1900"),
    ];
    for (code, n, expected) in cases {
        assert_eq!(fmt(code, n), expected, "{code} {n}");
    }
    assert_eq!(fmt("yyyy-mm-dd", -1.0), "###########");
}

#[test]
fn builtin_formats_follow_the_locale() {
    let dmy = FormatOptions::default();
    let mdy = FormatOptions {
        date_order: DateOrder::Mdy,
        date1904: false,
    };
    let code = builtin_format(14, &dmy).unwrap();
    assert_eq!(format_value(&Value::Number(46_308.0), &code, &dmy).text, "13-10-2026");
    let code = builtin_format(14, &mdy).unwrap();
    assert_eq!(format_value(&Value::Number(46_308.0), &code, &mdy).text, "10/13/2026");
    assert_eq!(builtin_format(9, &dmy).as_deref(), Some("0%"));
    assert_eq!(builtin_format(100, &dmy), None);
    // The 1904 system starts four years later.
    let mac = FormatOptions {
        date_order: DateOrder::Ymd,
        date1904: true,
    };
    assert_eq!(format_value(&Value::Number(0.0), "yyyy-mm-dd", &mac).text, "1904-01-01");
}

#[test]
fn format_kinds() {
    assert!(NumberFormat::parse("dd/mm/yyyy").is_date());
    assert!(NumberFormat::parse("[h]:mm").is_date());
    assert!(!NumberFormat::parse("0.00").is_date());
    assert!(!NumberFormat::parse("\"d\"0").is_date());
    assert!(NumberFormat::parse("@").is_text());
    assert!(NumberFormat::parse("0.0%").is_percent());
    assert!(NumberFormat::parse("General").is_general());
    assert_eq!(NumberFormat::parse("#,##0.000").decimals(), 3);
}

#[test]
fn rounding_helper() {
    assert_eq!(round_half_away(2.675, 2), 2.68);
    assert_eq!(round_half_away(-2.5, 0), -3.0);
    assert_eq!(round_half_away(0.0049, 2), 0.0);
    assert_eq!(round_half_away(999.995, 2), 1000.0);
    assert_eq!(round_half_away(123.456, -1), 120.0);
}
