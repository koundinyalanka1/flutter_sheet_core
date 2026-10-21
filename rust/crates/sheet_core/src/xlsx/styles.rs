//! The styles part (`xl/styles.xml`) and the theme's colours.

use doc_xml::{ns, Document, Element};

use super::{child_or_insert, qname, Package, STYLES_ORDER};
use crate::numfmt::{builtin_format, FormatOptions};
use crate::style::{Align, Border, CellStyle, ColorSpec, Edge, Fill, Font, Theme, Underline};

fn bool_attr(e: &Element, name: &str) -> Option<bool> {
    e.attr(ns::NONE, name).map(|v| matches!(v, "1" | "true" | "on"))
}

/// An element's `val` (absent means true for toggles such as `<b/>`).
fn toggle(e: Option<&Element>) -> bool {
    match e {
        None => false,
        Some(e) => e.attr(ns::NONE, "val").is_none_or(|v| !matches!(v, "0" | "false" | "off")),
    }
}

pub(super) fn read_color(e: &Element) -> Option<ColorSpec> {
    if let Some(rgb) = e.attr(ns::NONE, "rgb") {
        return ColorSpec::parse_hex(rgb);
    }
    if let Some(t) = e.attr(ns::NONE, "theme") {
        let i: u8 = t.parse().ok()?;
        return Some(ColorSpec::Theme(i, e.attr(ns::NONE, "tint").map(str::to_owned)));
    }
    if let Some(i) = e.attr(ns::NONE, "indexed") {
        return Some(ColorSpec::Indexed(i.parse().ok()?));
    }
    if bool_attr(e, "auto") == Some(true) {
        return Some(ColorSpec::Auto);
    }
    None
}

fn read_font(e: &Element) -> Font {
    let child = |name: &str| e.elements().find(|c| c.local() == name);
    let val = |name: &str| child(name).and_then(|c| c.attr(ns::NONE, "val")).map(str::to_owned);
    Font {
        name: val("name").or_else(|| val("rFont")).unwrap_or_else(|| "Calibri".into()),
        size: val("sz")
            .and_then(|s| s.parse::<f64>().ok())
            .map(|s| (s * 100.0).round() as u32)
            .unwrap_or(1100),
        bold: toggle(child("b")),
        italic: toggle(child("i")),
        underline: child("u").map_or(Underline::None, |u| Underline::parse(u.attr(ns::NONE, "val").unwrap_or("single"))),
        strike: toggle(child("strike")),
        color: child("color").and_then(read_color),
        vert_align: val("vertAlign").filter(|v| v != "baseline"),
        family: val("family"),
        scheme: val("scheme"),
    }
}

fn read_fill(e: &Element) -> Fill {
    if let Some(p) = e.elements().find(|c| c.local() == "patternFill") {
        let color = |name: &str| p.elements().find(|c| c.local() == name).and_then(read_color);
        return Fill {
            pattern: p.attr(ns::NONE, "patternType").unwrap_or("none").to_owned(),
            fg: color("fgColor"),
            bg: color("bgColor"),
        };
    }
    if let Some(g) = e.elements().find(|c| c.local() == "gradientFill") {
        // Gradients are shown as their first stop's colour.
        let first = g.descendants().find(|c| c.local() == "color").and_then(read_color);
        return Fill {
            pattern: "gradient".into(),
            fg: first,
            bg: None,
        };
    }
    Fill::none()
}

fn read_edge(e: Option<&Element>) -> Option<Edge> {
    let e = e?;
    let style = e.attr(ns::NONE, "style")?;
    if style == "none" {
        return None;
    }
    Some(Edge {
        style: style.to_owned(),
        color: e.elements().find(|c| c.local() == "color").and_then(read_color),
    })
}

fn read_border(e: &Element) -> Border {
    let side = |names: &[&str]| e.elements().find(|c| names.contains(&c.local()));
    Border {
        left: read_edge(side(&["left", "start"])),
        right: read_edge(side(&["right", "end"])),
        top: read_edge(side(&["top"])),
        bottom: read_edge(side(&["bottom"])),
        diagonal: read_edge(side(&["diagonal"])),
        diagonal_up: bool_attr(e, "diagonalUp").unwrap_or(false),
        diagonal_down: bool_attr(e, "diagonalDown").unwrap_or(false),
    }
}

fn list<'d>(root: &'d Element, container: &str, item: &'d str) -> Vec<&'d Element> {
    root.elements()
        .find(|e| e.local() == container)
        .map(|c| c.elements().filter(|e| e.local() == item).collect())
        .unwrap_or_default()
}

/// Read the styles part into cell styles (one per `cellXfs` entry).
pub(super) fn read_styles(pkg: &mut Package, theme: &Theme, options: &FormatOptions) -> Vec<CellStyle> {
    let Some(doc) = &pkg.styles else {
        return vec![CellStyle::base()];
    };
    let root = &doc.root;
    pkg.num_fmts = list(root, "numFmts", "numFmt")
        .iter()
        .filter_map(|e| Some((e.attr(ns::NONE, "numFmtId")?.parse().ok()?, e.attr(ns::NONE, "formatCode")?.to_owned())))
        .collect();
    pkg.fonts = list(root, "fonts", "font").into_iter().map(read_font).collect();
    pkg.fills = list(root, "fills", "fill").into_iter().map(read_fill).collect();
    pkg.borders = list(root, "borders", "border").into_iter().map(read_border).collect();
    let _ = theme;
    let xfs = list(root, "cellXfs", "xf");
    pkg.xf_count = xfs.len();
    let mut out = Vec::with_capacity(xfs.len());
    for xf in xfs {
        let idx = |name: &str| xf.attr(ns::NONE, name).and_then(|v| v.parse::<usize>().ok()).unwrap_or(0);
        let num_id = idx("numFmtId") as u32;
        let num_fmt = pkg
            .num_fmts
            .iter()
            .find(|(id, _)| *id == num_id)
            .map(|(_, code)| code.clone())
            .or_else(|| builtin_format(num_id, options))
            .unwrap_or_else(|| "General".into());
        let mut style = CellStyle {
            num_fmt,
            font: pkg.fonts.get(idx("fontId")).cloned().unwrap_or_default(),
            fill: pkg.fills.get(idx("fillId")).cloned().unwrap_or_else(Fill::none),
            border: pkg.borders.get(idx("borderId")).cloned().unwrap_or_default(),
            align: Align::default(),
            locked: true,
            hidden: false,
        };
        if let Some(a) = xf.elements().find(|e| e.local() == "alignment") {
            style.align = Align {
                horizontal: a.attr(ns::NONE, "horizontal").filter(|h| *h != "general").unwrap_or("").to_owned(),
                vertical: a.attr(ns::NONE, "vertical").filter(|v| *v != "bottom").unwrap_or("").to_owned(),
                wrap: bool_attr(a, "wrapText").unwrap_or(false),
                indent: a.attr(ns::NONE, "indent").and_then(|v| v.parse().ok()).unwrap_or(0),
                rotation: a.attr(ns::NONE, "textRotation").and_then(|v| v.parse().ok()).unwrap_or(0),
                shrink: bool_attr(a, "shrinkToFit").unwrap_or(false),
            };
        }
        if let Some(p) = xf.elements().find(|e| e.local() == "protection") {
            style.locked = bool_attr(p, "locked").unwrap_or(true);
            style.hidden = bool_attr(p, "hidden").unwrap_or(false);
        }
        out.push(style);
    }
    if out.is_empty() {
        out.push(CellStyle::base());
    }
    out
}

/// The theme's colours and fonts.
pub(super) fn read_theme(doc: &Document) -> Theme {
    let mut theme = Theme::default();
    let Some(scheme) = doc.root.descendants().find(|e| e.local() == "clrScheme") else {
        return theme;
    };
    // SpreadsheetML theme indexes: lt1, dk1, lt2, dk2, accents, links.
    let order = ["lt1", "dk1", "lt2", "dk2", "accent1", "accent2", "accent3", "accent4", "accent5", "accent6", "hlink", "folHlink"];
    for (i, name) in order.iter().enumerate() {
        let Some(slot) = scheme.elements().find(|e| e.local() == *name) else { continue };
        let Some(c) = slot.elements().next() else { continue };
        let hex = match c.local() {
            "srgbClr" => c.attr(ns::NONE, "val"),
            "sysClr" => c.attr(ns::NONE, "lastClr"),
            _ => None,
        };
        if let Some(v) = hex.and_then(|h| u32::from_str_radix(h, 16).ok()) {
            theme.colors[i] = v & 0xFF_FFFF;
        }
    }
    let font = |which: &str| {
        doc.root
            .descendants()
            .find(|e| e.local() == which)
            .and_then(|f| f.elements().find(|e| e.local() == "latin"))
            .and_then(|l| l.attr(ns::NONE, "typeface"))
            .map(str::to_owned)
    };
    if let Some(f) = font("minorFont") {
        theme.minor_font = f;
    }
    if let Some(f) = font("majorFont") {
        theme.major_font = f;
    }
    theme
}

fn color_element(prefix: &str, name: &str, color: &ColorSpec) -> Element {
    let e = Element::new(qname(prefix, name), ns::S);
    match color {
        ColorSpec::Rgb(c) => e.with_attr("rgb", ns::NONE, format!("FF{c:06X}")),
        ColorSpec::Theme(i, tint) => {
            let e = e.with_attr("theme", ns::NONE, i.to_string());
            match tint {
                Some(t) => e.with_attr("tint", ns::NONE, t.clone()),
                None => e,
            }
        }
        ColorSpec::Indexed(i) => e.with_attr("indexed", ns::NONE, i.to_string()),
        ColorSpec::Auto => e.with_attr("auto", ns::NONE, "1"),
    }
}

fn font_element(prefix: &str, f: &Font) -> Element {
    let mut e = Element::new(qname(prefix, "font"), ns::S);
    let val = |name: &str, v: &str| Element::new(qname(prefix, name), ns::S).with_attr("val", ns::NONE, v);
    if f.bold {
        e.push(Element::new(qname(prefix, "b"), ns::S));
    }
    if f.italic {
        e.push(Element::new(qname(prefix, "i"), ns::S));
    }
    if f.strike {
        e.push(Element::new(qname(prefix, "strike"), ns::S));
    }
    match f.underline {
        Underline::None => {}
        Underline::Single => e.push(Element::new(qname(prefix, "u"), ns::S)),
        other => e.push(val("u", other.name())),
    }
    if let Some(v) = &f.vert_align {
        e.push(val("vertAlign", v));
    }
    let size = f.size as f64 / 100.0;
    e.push(val("sz", &crate::value::number_to_text(size)));
    if let Some(c) = &f.color {
        e.push(color_element(prefix, "color", c));
    }
    e.push(val("name", &f.name));
    if let Some(fam) = &f.family {
        e.push(val("family", fam));
    }
    if let Some(s) = &f.scheme {
        e.push(val("scheme", s));
    }
    e
}

fn fill_element(prefix: &str, f: &Fill) -> Element {
    let mut p = Element::new(qname(prefix, "patternFill"), ns::S).with_attr(
        "patternType",
        ns::NONE,
        if f.pattern == "gradient" { "solid" } else { f.pattern.as_str() },
    );
    if let Some(c) = &f.fg {
        p.push(color_element(prefix, "fgColor", c));
    }
    if let Some(c) = &f.bg {
        p.push(color_element(prefix, "bgColor", c));
    }
    let mut e = Element::new(qname(prefix, "fill"), ns::S);
    e.push(p);
    e
}

fn border_element(prefix: &str, b: &Border) -> Element {
    let mut e = Element::new(qname(prefix, "border"), ns::S);
    if b.diagonal_up {
        e.set_attr("diagonalUp", ns::NONE, "1");
    }
    if b.diagonal_down {
        e.set_attr("diagonalDown", ns::NONE, "1");
    }
    for (name, edge) in [("left", &b.left), ("right", &b.right), ("top", &b.top), ("bottom", &b.bottom), ("diagonal", &b.diagonal)] {
        let mut side = Element::new(qname(prefix, name), ns::S);
        if let Some(edge) = edge {
            side.set_attr("style", ns::NONE, edge.style.clone());
            if let Some(c) = &edge.color {
                side.push(color_element(prefix, "color", c));
            }
        }
        e.push(side);
    }
    e
}

fn set_count(e: &mut Element) {
    let n = e.elements().count();
    e.set_attr("count", ns::NONE, n.to_string());
}

/// Append the workbook's new styles (those past `pkg.xf_count`) to the
/// styles part, reusing fonts, fills and borders already listed.
pub(super) fn append_styles(pkg: &mut Package, styles: &[CellStyle], options: &FormatOptions) {
    if styles.len() <= pkg.xf_count {
        return;
    }
    let Some(doc) = &mut pkg.styles else { return };
    let prefix = doc.prefix_for(ns::S).unwrap_or_default();
    let new = &styles[pkg.xf_count..];
    for style in new {
        // Number format id: built-in, already listed, or a new custom one.
        // Ids 14 and 22 show the reader's own short date, so a written-out
        // date format is never mapped to them.
        let _ = options;
        let num_id = (0..=49u32)
            .filter(|id| !matches!(id, 14 | 22))
            .find(|id| builtin_format(*id, &FormatOptions::default()).as_deref() == Some(style.num_fmt.as_str()))
            .or_else(|| pkg.num_fmts.iter().find(|(_, c)| *c == style.num_fmt).map(|(id, _)| *id))
            .unwrap_or_else(|| {
                let id = pkg.num_fmts.iter().map(|(id, _)| *id).max().unwrap_or(163).max(163) + 1;
                pkg.num_fmts.push((id, style.num_fmt.clone()));
                let fmts = child_or_insert(&mut doc.root, "numFmts", STYLES_ORDER, ns::S, &prefix);
                fmts.push(
                    doc_xml::Element::new(qname(&prefix, "numFmt"), ns::S)
                        .with_attr("numFmtId", ns::NONE, id.to_string())
                        .with_attr("formatCode", ns::NONE, style.num_fmt.clone()),
                );
                set_count(fmts);
                id
            });
        let font_id = match pkg.fonts.iter().position(|f| *f == style.font) {
            Some(i) => i,
            None => {
                let fonts = child_or_insert(&mut doc.root, "fonts", STYLES_ORDER, ns::S, &prefix);
                fonts.push(font_element(&prefix, &style.font));
                set_count(fonts);
                pkg.fonts.push(style.font.clone());
                pkg.fonts.len() - 1
            }
        };
        let fill_id = match pkg.fills.iter().position(|f| *f == style.fill) {
            Some(i) => i,
            None => {
                let fills = child_or_insert(&mut doc.root, "fills", STYLES_ORDER, ns::S, &prefix);
                fills.push(fill_element(&prefix, &style.fill));
                set_count(fills);
                pkg.fills.push(style.fill.clone());
                pkg.fills.len() - 1
            }
        };
        let border_id = match pkg.borders.iter().position(|b| *b == style.border) {
            Some(i) => i,
            None => {
                let borders = child_or_insert(&mut doc.root, "borders", STYLES_ORDER, ns::S, &prefix);
                borders.push(border_element(&prefix, &style.border));
                set_count(borders);
                pkg.borders.push(style.border.clone());
                pkg.borders.len() - 1
            }
        };
        let mut xf = Element::new(qname(&prefix, "xf"), ns::S)
            .with_attr("numFmtId", ns::NONE, num_id.to_string())
            .with_attr("fontId", ns::NONE, font_id.to_string())
            .with_attr("fillId", ns::NONE, fill_id.to_string())
            .with_attr("borderId", ns::NONE, border_id.to_string())
            .with_attr("xfId", ns::NONE, "0");
        if num_id != 0 {
            xf.set_attr("applyNumberFormat", ns::NONE, "1");
        }
        if font_id != 0 {
            xf.set_attr("applyFont", ns::NONE, "1");
        }
        if fill_id != 0 {
            xf.set_attr("applyFill", ns::NONE, "1");
        }
        if border_id != 0 {
            xf.set_attr("applyBorder", ns::NONE, "1");
        }
        let a = &style.align;
        if *a != Align::default() {
            xf.set_attr("applyAlignment", ns::NONE, "1");
            let mut al = Element::new(qname(&prefix, "alignment"), ns::S);
            if !a.horizontal.is_empty() {
                al.set_attr("horizontal", ns::NONE, a.horizontal.clone());
            }
            if !a.vertical.is_empty() {
                al.set_attr("vertical", ns::NONE, a.vertical.clone());
            }
            if a.rotation != 0 {
                al.set_attr("textRotation", ns::NONE, a.rotation.to_string());
            }
            if a.wrap {
                al.set_attr("wrapText", ns::NONE, "1");
            }
            if a.indent != 0 {
                al.set_attr("indent", ns::NONE, a.indent.to_string());
            }
            if a.shrink {
                al.set_attr("shrinkToFit", ns::NONE, "1");
            }
            xf.push(al);
        }
        if !style.locked || style.hidden {
            xf.set_attr("applyProtection", ns::NONE, "1");
            let mut p = Element::new(qname(&prefix, "protection"), ns::S);
            if !style.locked {
                p.set_attr("locked", ns::NONE, "0");
            }
            if style.hidden {
                p.set_attr("hidden", ns::NONE, "1");
            }
            xf.push(p);
        }
        let xfs = child_or_insert(&mut doc.root, "cellXfs", STYLES_ORDER, ns::S, &prefix);
        xfs.push(xf);
        set_count(xfs);
    }
    pkg.xf_count = styles.len();
}
