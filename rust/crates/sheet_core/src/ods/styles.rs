//! OpenDocument styles: cell styles to and from [`CellStyle`], data styles
//! to and from number format codes, column widths and row heights.

use std::collections::HashMap;

use doc_xml::{escape_attr, Element};

use crate::numfmt::{NumberFormat, Section, Token};
use crate::style::{CellStyle, ColorSpec, Edge, Fill, Underline};

/// A length (`0.6689in`, `1.9cm`, `17mm`, `11pt`) in points.
pub fn length_pt(v: &str) -> Option<f64> {
    let v = v.trim();
    let split = v.find(|c: char| c.is_ascii_alphabetic() || c == '%')?;
    let (num, unit) = v.split_at(split);
    let n: f64 = num.trim().parse().ok()?;
    Some(match unit {
        "pt" => n,
        "in" => n * 72.0,
        "cm" => n * 72.0 / 2.54,
        "mm" => n * 72.0 / 25.4,
        "pc" => n * 12.0,
        "px" => n * 0.75,
        _ => return None,
    })
}

/// Excel column width (characters of the default font) from points.
pub fn chars_from_pt(pt: f64) -> f64 {
    (((pt / 0.75) - 5.0) / 7.0 * 100.0).round().max(0.0) / 100.0
}

/// Points from an Excel column width.
pub fn pt_from_chars(chars: f64) -> f64 {
    (chars * 7.0 + 5.0).round() * 0.75
}

/// The value of an attribute by its local name, whatever its prefix.
pub fn attr<'e>(e: &'e Element, local: &str) -> Option<&'e str> {
    e.attrs.iter().find(|a| a.local() == local).map(|a| a.value.as_str())
}

fn parse_color(v: &str) -> Option<ColorSpec> {
    if v == "transparent" {
        return None;
    }
    ColorSpec::parse_hex(v)
}

fn parse_border(v: &str) -> Option<Edge> {
    let v = v.trim();
    if v == "none" || v == "hidden" || v.is_empty() {
        return None;
    }
    let mut width = 0.75;
    let mut style = "solid";
    let mut color = None;
    for part in v.split_whitespace() {
        if let Some(w) = length_pt(part) {
            width = w;
        } else if part.starts_with('#') {
            color = parse_color(part);
        } else {
            style = match part {
                "solid" | "dashed" | "dotted" | "double" | "dash-dot" | "dash-dot-dot" | "fine-dashed" => part,
                _ => style,
            };
        }
    }
    let name = match (style, width) {
        ("double", _) => "double",
        ("dotted", _) => "dotted",
        ("dashed" | "fine-dashed", w) if w > 1.0 => "mediumDashed",
        ("dashed" | "fine-dashed", _) => "dashed",
        ("dash-dot", _) => "dashDot",
        ("dash-dot-dot", _) => "dashDotDot",
        (_, w) if w >= 2.25 => "thick",
        (_, w) if w >= 1.25 => "medium",
        (_, w) if w < 0.5 => "hair",
        _ => "thin",
    };
    Some(Edge {
        style: name.into(),
        color,
    })
}

fn border_text(e: &Edge) -> String {
    let (width, style) = match e.style.as_str() {
        "double" => ("2.01pt", "double"),
        "dotted" => ("0.74pt", "dotted"),
        "dashed" => ("0.74pt", "dashed"),
        "mediumDashed" => ("1.76pt", "dashed"),
        "dashDot" | "mediumDashDot" | "slantDashDot" => ("0.74pt", "dash-dot"),
        "dashDotDot" | "mediumDashDotDot" => ("0.74pt", "dash-dot-dot"),
        "thick" => ("2.49pt", "solid"),
        "medium" => ("1.76pt", "solid"),
        "hair" => ("0.06pt", "solid"),
        _ => ("0.74pt", "solid"),
    };
    let color = match &e.color {
        Some(ColorSpec::Rgb(c)) => format!("#{c:06x}"),
        _ => "#000000".into(),
    };
    format!("{width} {style} {color}")
}

/// Apply one `style:style` element's properties on top of `base`.
pub fn apply_props(base: &mut CellStyle, e: &Element, fonts: &HashMap<String, String>) {
    for p in e.elements() {
        match p.local() {
            "table-cell-properties" => {
                if let Some(bg) = attr(p, "background-color") {
                    base.fill = match parse_color(bg) {
                        Some(c) => Fill::solid(c),
                        None => Fill::none(),
                    };
                }
                if let Some(b) = attr(p, "border") {
                    let edge = parse_border(b);
                    base.border.left = edge.clone();
                    base.border.right = edge.clone();
                    base.border.top = edge.clone();
                    base.border.bottom = edge;
                }
                for (name, slot) in [("border-left", 0), ("border-right", 1), ("border-top", 2), ("border-bottom", 3)] {
                    if let Some(b) = attr(p, name) {
                        let edge = parse_border(b);
                        match slot {
                            0 => base.border.left = edge,
                            1 => base.border.right = edge,
                            2 => base.border.top = edge,
                            _ => base.border.bottom = edge,
                        }
                    }
                }
                if let Some(w) = attr(p, "wrap-option") {
                    base.align.wrap = w == "wrap";
                }
                if let Some(v) = attr(p, "vertical-align") {
                    base.align.vertical = match v {
                        "top" => "top".into(),
                        "middle" => "center".into(),
                        _ => String::new(),
                    };
                }
                if let Some(r) = attr(p, "rotation-angle").and_then(|r| r.trim_end_matches("deg").parse::<f64>().ok()) {
                    let r = r.round() as i16 % 360;
                    base.align.rotation = if (0..=90).contains(&r) { r } else if r >= 270 { 90 + (360 - r) } else { 0 };
                }
                if let Some(s) = attr(p, "shrink-to-fit") {
                    base.align.shrink = s == "true";
                }
                if let Some(c) = attr(p, "cell-protect") {
                    base.locked = c.contains("protected") && !c.contains("none");
                    base.hidden = c.contains("formula-hidden") || c.contains("hidden-and-protected");
                }
            }
            "paragraph-properties" => {
                if let Some(a) = attr(p, "text-align") {
                    base.align.horizontal = match a {
                        "center" => "center".into(),
                        "end" | "right" => "right".into(),
                        "justify" => "justify".into(),
                        "start" | "left" => "left".into(),
                        _ => String::new(),
                    };
                }
                if let Some(m) = attr(p, "margin-left").and_then(length_pt) {
                    base.align.indent = (m / 9.0).round().clamp(0.0, 250.0) as u8;
                }
            }
            "text-properties" => {
                if let Some(w) = attr(p, "font-weight") {
                    base.font.bold = w == "bold" || w.parse::<u32>().is_ok_and(|n| n >= 600);
                }
                if let Some(s) = attr(p, "font-style") {
                    base.font.italic = s == "italic" || s == "oblique";
                }
                if let Some(u) = attr(p, "text-underline-style") {
                    base.font.underline = if u == "none" {
                        Underline::None
                    } else if attr(p, "text-underline-type") == Some("double") {
                        Underline::Double
                    } else {
                        Underline::Single
                    };
                }
                if let Some(s) = attr(p, "text-line-through-style") {
                    base.font.strike = s != "none";
                }
                if let Some(sz) = attr(p, "font-size").and_then(length_pt) {
                    base.font.size = (sz * 100.0).round() as u32;
                }
                if let Some(c) = attr(p, "color") {
                    base.font.color = parse_color(c);
                }
                if attr(p, "use-window-font-color") == Some("true") {
                    base.font.color = None;
                }
                if let Some(f) = attr(p, "font-name") {
                    base.font.name = fonts.get(f).cloned().unwrap_or_else(|| f.to_owned());
                    base.font.scheme = None;
                } else if let Some(f) = attr(p, "font-family") {
                    base.font.name = f.trim_matches('\'').to_owned();
                    base.font.scheme = None;
                }
                if let Some(pos) = attr(p, "text-position") {
                    base.font.vert_align = if pos.starts_with("super") {
                        Some("superscript".into())
                    } else if pos.starts_with("sub") {
                        Some("subscript".into())
                    } else {
                        None
                    };
                }
            }
            _ => {}
        }
    }
}

/// A data style element (`number:number-style`, `number:date-style`...)
/// as an Excel format code.
pub fn data_style_code(e: &Element, all: &HashMap<String, Element>, depth: usize) -> String {
    let mut code = String::new();
    let kind = e.local();
    if kind == "boolean-style" {
        return "\"TRUE\";\"TRUE\";\"FALSE\"".into();
    }
    let long = |c: &Element| attr(c, "style") == Some("long");
    let mut color = String::new();
    for c in e.elements() {
        match c.local() {
            "number" => {
                let dec: usize = attr(c, "decimal-places").and_then(|v| v.parse().ok()).unwrap_or(0);
                let min_dec: usize = attr(c, "min-decimal-places").and_then(|v| v.parse().ok()).unwrap_or(dec);
                let min_int: usize = attr(c, "min-integer-digits").and_then(|v| v.parse().ok()).unwrap_or(1);
                let grouping = attr(c, "grouping") == Some("true");
                let mut int = "0".repeat(min_int.max(if grouping { 1 } else { 0 }));
                if int.is_empty() {
                    int.push('#');
                }
                if grouping {
                    while int.len() < 4 {
                        int.insert(0, '#');
                    }
                    let l = int.len();
                    int.insert(l - 3, ',');
                }
                code.push_str(&int);
                if dec > 0 {
                    code.push('.');
                    code.push_str(&"0".repeat(min_dec.min(dec)));
                    code.push_str(&"#".repeat(dec - min_dec.min(dec)));
                }
            }
            "scientific-number" => {
                let dec: usize = attr(c, "decimal-places").and_then(|v| v.parse().ok()).unwrap_or(2);
                code.push('0');
                if dec > 0 {
                    code.push('.');
                    code.push_str(&"0".repeat(dec));
                }
                code.push_str("E+00");
            }
            "fraction" => {
                let d: usize = attr(c, "min-denominator-digits").and_then(|v| v.parse().ok()).unwrap_or(1);
                code.push_str(&format!("# {}/{}", "?".repeat(d), "?".repeat(d)));
            }
            "currency-symbol" => {
                let sym = c.text();
                code.push('"');
                code.push_str(&sym.replace('"', ""));
                code.push('"');
            }
            "text" => {
                let t = c.text();
                let date_like = matches!(kind, "date-style" | "time-style");
                let safe = |ch: char| matches!(ch, ' ' | '-' | '(' | ')' | '/' | ':') || (date_like && matches!(ch, '.' | ','));
                if t == "%" {
                    code.push('%');
                } else if !t.is_empty() && t.chars().all(safe) {
                    code.push_str(&t);
                } else if !t.is_empty() {
                    code.push('"');
                    code.push_str(&t.replace('"', ""));
                    code.push('"');
                }
            }
            "text-content" => code.push('@'),
            "day" => code.push_str(if long(c) { "dd" } else { "d" }),
            "day-of-week" => code.push_str(if long(c) { "dddd" } else { "ddd" }),
            "month" => {
                let textual = attr(c, "textual") == Some("true");
                code.push_str(match (textual, long(c)) {
                    (true, true) => "mmmm",
                    (true, false) => "mmm",
                    (false, true) => "mm",
                    (false, false) => "m",
                });
            }
            "year" => code.push_str(if long(c) { "yyyy" } else { "yy" }),
            "hours" => {
                let elapsed = attr(e, "truncate-on-overflow") == Some("false");
                let h = if long(c) { "hh" } else { "h" };
                if elapsed {
                    code.push('[');
                    code.push_str(h);
                    code.push(']');
                } else {
                    code.push_str(h);
                }
            }
            "minutes" => code.push_str(if long(c) { "mm" } else { "m" }),
            "seconds" => {
                code.push_str(if long(c) { "ss" } else { "s" });
                let dec: usize = attr(c, "decimal-places").and_then(|v| v.parse().ok()).unwrap_or(0);
                if dec > 0 {
                    code.push('.');
                    code.push_str(&"0".repeat(dec));
                }
            }
            "am-pm" => code.push_str("AM/PM"),
            "quarter" => code.push_str("\"Q\"0"),
            _ => {}
        }
    }
    if kind == "percentage-style" && !code.contains('%') {
        code.push('%');
    }
    if let Some(tp) = e.elements().find(|c| c.local() == "text-properties") {
        if let Some(c) = attr(tp, "color") {
            color = match c.to_ascii_lowercase().as_str() {
                "#ff0000" => "[Red]".into(),
                "#0000ff" => "[Blue]".into(),
                "#00ff00" => "[Green]".into(),
                _ => String::new(),
            };
        }
    }
    let this = format!("{color}{code}");
    // Conditional maps: "value()>=0" applies another style to positives.
    if depth < 4 {
        for m in e.elements().filter(|c| c.local() == "map") {
            let cond = attr(m, "condition").unwrap_or("");
            if let Some(target) = attr(m, "apply-style-name").and_then(|n| all.get(n)) {
                let other = data_style_code(target, all, depth + 1);
                if cond.contains(">=0") || cond.contains(">0") {
                    return format!("{other};{this}");
                }
            }
        }
    }
    if this.is_empty() {
        "General".into()
    } else {
        this
    }
}

/// Writes data styles for format codes, naming them `dhN1`, `dhN2`...
#[derive(Default)]
pub struct DataStyles {
    names: HashMap<String, String>,
    pub xml: String,
}

impl DataStyles {
    /// The data style name for a format code (None for General).
    pub fn name_for(&mut self, code: &str) -> Option<String> {
        let fmt = NumberFormat::parse(code);
        if fmt.is_general() {
            return None;
        }
        if let Some(n) = self.names.get(code) {
            return Some(n.clone());
        }
        let name = format!("dhN{}", self.names.len() + 1);
        let sections = crate::numfmt::parse_sections_public(code);
        let first = sections.first()?;
        let mut body = section_xml(first);
        let kind = section_kind(first);
        // A second section for negatives: map positives to the first.
        let mut out = String::new();
        if sections.len() >= 2 && kind != "date-style" && kind != "time-style" && kind != "text-style" {
            let pos_name = format!("{name}P0");
            out.push_str(&format!("<number:{kind} style:name=\"{pos_name}\">{body}</number:{kind}>"));
            body = section_xml(&sections[1]);
            let color = sections[1]
                .color
                .map(|c| format!("<style:text-properties fo:color=\"#{c:06x}\"/>"))
                .unwrap_or_default();
            // The negative section shows its own sign (if any) as literal text.
            let minus = if sections[1].tokens.iter().any(|t| matches!(t, Token::Literal(l) if l.contains('-') || l.contains('('))) {
                ""
            } else {
                "<number:text>-</number:text>"
            };
            out.push_str(&format!(
                "<number:{kind} style:name=\"{name}\">{color}{minus}{body}<style:map style:condition=\"value()&gt;=0\" style:apply-style-name=\"{pos_name}\"/></number:{kind}>"
            ));
        } else {
            let extra = if kind == "time-style" && first.tokens.iter().any(|t| matches!(t, Token::ElapsedHours(_))) {
                " number:truncate-on-overflow=\"false\""
            } else {
                ""
            };
            let color = first
                .color
                .map(|c| format!("<style:text-properties fo:color=\"#{c:06x}\"/>"))
                .unwrap_or_default();
            out.push_str(&format!("<number:{kind} style:name=\"{name}\"{extra}>{color}{body}</number:{kind}>"));
        }
        self.xml.push_str(&out);
        self.names.insert(code.to_owned(), name.clone());
        Some(name)
    }
}

fn section_kind(s: &Section) -> &'static str {
    if s.tokens.iter().any(|t| matches!(t, Token::At)) {
        return "text-style";
    }
    if s.is_date {
        let has_date = s.tokens.iter().any(|t| matches!(t, Token::Year(_) | Token::Month(_) | Token::Day(_)));
        return if has_date { "date-style" } else { "time-style" };
    }
    if s.percent > 0 {
        return "percentage-style";
    }
    let currency = s
        .tokens
        .iter()
        .any(|t| matches!(t, Token::Literal(l) if l.contains(['$', '₹', '€', '£', '¥'])));
    if currency {
        "currency-style"
    } else {
        "number-style"
    }
}

fn text_el(out: &mut String, t: &str) {
    if t.is_empty() {
        return;
    }
    out.push_str("<number:text>");
    let mut esc = String::new();
    doc_xml::escape_text(t, &mut esc);
    out.push_str(&esc);
    out.push_str("</number:text>");
}

fn section_xml(s: &Section) -> String {
    let mut out = String::new();
    if s.is_date {
        for t in &s.tokens {
            match t {
                Token::Year(n) => out.push_str(if *n > 2 { "<number:year number:style=\"long\"/>" } else { "<number:year/>" }),
                Token::Month(n) => out.push_str(match n {
                    1 => "<number:month/>",
                    2 => "<number:month number:style=\"long\"/>",
                    3 => "<number:month number:textual=\"true\"/>",
                    _ => "<number:month number:style=\"long\" number:textual=\"true\"/>",
                }),
                Token::Day(n) => out.push_str(match n {
                    1 => "<number:day/>",
                    2 => "<number:day number:style=\"long\"/>",
                    3 => "<number:day-of-week/>",
                    _ => "<number:day-of-week number:style=\"long\"/>",
                }),
                Token::Hour(n) | Token::ElapsedHours(n) => {
                    out.push_str(if *n > 1 { "<number:hours number:style=\"long\"/>" } else { "<number:hours/>" })
                }
                Token::Minute(n) | Token::ElapsedMinutes(n) => {
                    out.push_str(if *n > 1 { "<number:minutes number:style=\"long\"/>" } else { "<number:minutes/>" })
                }
                Token::Second(n) | Token::ElapsedSeconds(n) => {
                    out.push_str(if *n > 1 { "<number:seconds number:style=\"long\"/>" } else { "<number:seconds/>" })
                }
                Token::AmPm(_) => out.push_str("<number:am-pm/>"),
                Token::Literal(l) => text_el(&mut out, l),
                Token::Space(_) => text_el(&mut out, " "),
                _ => {}
            }
        }
        return out;
    }
    if s.tokens.iter().any(|t| matches!(t, Token::At)) {
        for t in &s.tokens {
            match t {
                Token::At => out.push_str("<number:text-content/>"),
                Token::Literal(l) => text_el(&mut out, l),
                _ => {}
            }
        }
        return out;
    }
    let (int, _frac, _) = s.split_parts();
    let min_int = int.iter().filter(|t| matches!(t, Token::Zero)).count();
    let dec = s.decimals();
    let mut number = if s.exponent.is_some() {
        format!("<number:scientific-number number:decimal-places=\"{dec}\" number:min-integer-digits=\"{min_int}\" number:min-exponent-digits=\"2\"/>")
    } else if s.fraction.is_some() {
        "<number:fraction number:min-integer-digits=\"0\" number:min-numerator-digits=\"1\" number:min-denominator-digits=\"1\"/>".to_owned()
    } else {
        format!(
            "<number:number number:decimal-places=\"{dec}\" number:min-decimal-places=\"{dec}\" number:min-integer-digits=\"{min_int}\"{}/>",
            if s.thousands { " number:grouping=\"true\"" } else { "" }
        )
    };
    // Literal text before the first digit, then the number, then the rest.
    let first_digit = s.tokens.iter().position(Token::is_digit).unwrap_or(s.tokens.len());
    let last_digit = s.tokens.iter().rposition(|t| t.is_digit() || matches!(t, Token::Point)).map_or(0, |i| i + 1);
    let lit = |ts: &[Token], out: &mut String| {
        let mut buf = String::new();
        for t in ts {
            match t {
                Token::Literal(l) => {
                    for ch in l.chars() {
                        if matches!(ch, '$' | '₹' | '€' | '£' | '¥') {
                            if !buf.is_empty() {
                                text_el(out, &buf);
                                buf.clear();
                            }
                            out.push_str(&format!("<number:currency-symbol>{ch}</number:currency-symbol>"));
                        } else {
                            buf.push(ch);
                        }
                    }
                }
                Token::Space(_) => buf.push(' '),
                _ => {}
            }
        }
        text_el(out, &buf);
    };
    lit(&s.tokens[..first_digit], &mut out);
    out.push_str(&std::mem::take(&mut number));
    if last_digit < s.tokens.len() {
        lit(&s.tokens[last_digit..], &mut out);
    }
    out
}

fn color_hex(c: &Option<ColorSpec>, theme: &crate::style::Theme) -> Option<String> {
    c.as_ref().and_then(|c| c.resolve(theme)).map(|v| format!("#{v:06x}"))
}

/// An automatic cell style (`style:style style:family="table-cell"`).
pub fn cell_style_xml(name: &str, s: &CellStyle, data_style: Option<&str>, theme: &crate::style::Theme) -> String {
    let mut out = format!("<style:style style:name=\"{name}\" style:family=\"table-cell\" style:parent-style-name=\"Default\"");
    if let Some(d) = data_style {
        out.push_str(&format!(" style:data-style-name=\"{d}\""));
    }
    out.push('>');
    // Cell properties.
    let mut cell = String::new();
    if let Some(c) = s.fill.paint(theme) {
        cell.push_str(&format!(" fo:background-color=\"#{c:06x}\""));
    }
    let b = &s.border;
    for (attr_name, edge) in [("fo:border-left", &b.left), ("fo:border-right", &b.right), ("fo:border-top", &b.top), ("fo:border-bottom", &b.bottom)] {
        if let Some(e) = edge {
            let resolved = Edge {
                style: e.style.clone(),
                color: e.color.as_ref().and_then(|c| c.resolve(theme)).map(ColorSpec::Rgb),
            };
            cell.push_str(&format!(" {attr_name}=\"{}\"", border_text(&resolved)));
        }
    }
    if s.align.wrap {
        cell.push_str(" fo:wrap-option=\"wrap\"");
    }
    match s.align.vertical.as_str() {
        "top" => cell.push_str(" style:vertical-align=\"top\""),
        "center" => cell.push_str(" style:vertical-align=\"middle\""),
        _ => {}
    }
    if s.align.rotation != 0 {
        let deg = if s.align.rotation > 90 { 360 - (s.align.rotation - 90) } else { s.align.rotation };
        cell.push_str(&format!(" style:rotation-angle=\"{deg}\""));
    }
    if s.align.shrink {
        cell.push_str(" style:shrink-to-fit=\"true\"");
    }
    if !s.locked {
        cell.push_str(" style:cell-protect=\"none\"");
    }
    if !s.align.horizontal.is_empty() {
        cell.push_str(" style:text-align-source=\"fix\"");
    }
    if !cell.is_empty() {
        out.push_str(&format!("<style:table-cell-properties{cell}/>"));
    }
    let h = match s.align.horizontal.as_str() {
        "center" | "centerContinuous" => Some("center"),
        "right" => Some("end"),
        "left" => Some("start"),
        "justify" | "distributed" => Some("justify"),
        _ => None,
    };
    if h.is_some() || s.align.indent > 0 {
        out.push_str("<style:paragraph-properties");
        if let Some(h) = h {
            out.push_str(&format!(" fo:text-align=\"{h}\""));
        }
        if s.align.indent > 0 {
            out.push_str(&format!(" fo:margin-left=\"{}pt\"", s.align.indent as u32 * 9));
        }
        out.push_str("/>");
    }
    // Text properties.
    let f = &s.font;
    let base = crate::style::Font::default();
    let mut text = String::new();
    if f.name != base.name {
        let mut v = String::new();
        escape_attr(&f.name, &mut v);
        text.push_str(&format!(" style:font-name=\"{v}\""));
    }
    if f.size != base.size {
        text.push_str(&format!(" fo:font-size=\"{}pt\"", crate::value::number_to_text(f.size as f64 / 100.0)));
    }
    if f.bold {
        text.push_str(" fo:font-weight=\"bold\" style:font-weight-asian=\"bold\" style:font-weight-complex=\"bold\"");
    }
    if f.italic {
        text.push_str(" fo:font-style=\"italic\" style:font-style-asian=\"italic\" style:font-style-complex=\"italic\"");
    }
    match f.underline {
        Underline::None => {}
        Underline::Double | Underline::DoubleAccounting => {
            text.push_str(" style:text-underline-style=\"solid\" style:text-underline-type=\"double\" style:text-underline-width=\"auto\" style:text-underline-color=\"font-color\"")
        }
        _ => text.push_str(" style:text-underline-style=\"solid\" style:text-underline-width=\"auto\" style:text-underline-color=\"font-color\""),
    }
    if f.strike {
        text.push_str(" style:text-line-through-style=\"solid\" style:text-line-through-type=\"single\"");
    }
    if let Some(c) = color_hex(&f.color, theme) {
        if c != "#000000" {
            text.push_str(&format!(" fo:color=\"{c}\""));
        }
    }
    match f.vert_align.as_deref() {
        Some("superscript") => text.push_str(" style:text-position=\"super 58%\""),
        Some("subscript") => text.push_str(" style:text-position=\"sub 58%\""),
        _ => {}
    }
    if !text.is_empty() {
        out.push_str(&format!("<style:text-properties{text}/>"));
    }
    out.push_str("</style:style>");
    out
}

/// Fonts used by styles, declared once (`style:font-face`).
pub fn font_face_xml(name: &str) -> String {
    let mut v = String::new();
    escape_attr(name, &mut v);
    let mut fam = String::new();
    escape_attr(&format!("'{name}'"), &mut fam);
    format!("<style:font-face style:name=\"{v}\" svg:font-family=\"{fam}\"/>")
}
