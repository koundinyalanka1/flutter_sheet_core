//! What a cell shows, and what its formula bar shows for editing.

use crate::datetime::serial_to_date;
use crate::model::{Cell, Workbook};
use crate::numfmt::{DateOrder, NumberFormat};
use crate::value::{number_to_text, Value};

/// A cell as the grid draws it.
#[derive(Clone, Debug, PartialEq)]
pub struct Shown {
    pub text: String,
    /// A colour the number format asks for (`[Red]`), `0xRRGGBB`.
    pub color: Option<u32>,
    /// `n` number, `s` text, `b` boolean, `e` error, `` empty.
    pub kind: &'static str,
}

pub fn shown(wb: &Workbook, cell: &Cell) -> Shown {
    let style = wb.styles.get(cell.style);
    let kind = match &cell.value {
        Value::Number(_) => "n",
        Value::Text(_) => "s",
        Value::Bool(_) => "b",
        Value::Error(_) => "e",
        Value::Empty => "",
    };
    let f = NumberFormat::cached(&style.num_fmt).format(&cell.value, &wb.format_options());
    Shown {
        text: f.text,
        color: f.color,
        kind,
    }
}

pub fn shown_text(wb: &Workbook, cell: &Cell) -> String {
    shown(wb, cell).text
}

/// The text to put in the formula bar: the formula, or the value written
/// the way it would be typed (dates in the locale's order, percentages
/// with `%`).
pub fn edit_text(wb: &Workbook, cell: &Cell) -> String {
    if let Some(f) = &cell.formula {
        return format!("={}", f.text);
    }
    let style = wb.styles.get(cell.style);
    let fmt = NumberFormat::cached(&style.num_fmt);
    match &cell.value {
        Value::Empty => String::new(),
        Value::Text(t) => {
            // Text that would be read as something else keeps its apostrophe.
            let reads_as_other = !fmt.is_text()
                && !matches!(crate::input::parse_input(t, &wb.format_options()), crate::input::Input::Text(_));
            if reads_as_other {
                format!("'{t}")
            } else {
                t.to_string()
            }
        }
        Value::Bool(b) => if *b { "TRUE" } else { "FALSE" }.into(),
        Value::Error(e) => e.as_str().into(),
        Value::Number(n) => {
            if fmt.is_date() {
                if let Some(text) = date_for_editing(*n, wb) {
                    return text;
                }
            }
            if fmt.is_percent() {
                return format!("{}%", number_to_text(n * 100.0));
            }
            number_to_text(*n)
        }
    }
}

fn date_for_editing(n: f64, wb: &Workbook) -> Option<String> {
    let (y, m, d) = serial_to_date(n, wb.date1904)?;
    let date = match wb.date_order {
        DateOrder::Dmy => format!("{d:02}/{m:02}/{y}"),
        DateOrder::Mdy => format!("{m}/{d}/{y}"),
        DateOrder::Ymd => format!("{y}-{m:02}-{d:02}"),
    };
    let frac = n - n.floor();
    if frac.abs() < 1e-9 {
        if n < 1.0 {
            return Some("00:00:00".into());
        }
        return Some(date);
    }
    let (h, mi, s, _) = crate::datetime::time_of_day(n);
    let time = format!("{h}:{mi:02}:{s:02}");
    if n < 1.0 {
        Some(time)
    } else {
        Some(format!("{date} {time}"))
    }
}
