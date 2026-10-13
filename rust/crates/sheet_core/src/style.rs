//! Cell formatting: fonts, fills, borders, alignment and number formats,
//! independent of the file format they came from.

use std::collections::HashMap;

/// A colour as a file states it. Theme colours and tints are kept as
/// written, so a style copied from the file and saved again is unchanged.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum ColorSpec {
    /// `0xRRGGBB`.
    Rgb(u32),
    /// A theme colour (SpreadsheetML numbering: 0 = Background 1 (lt1),
    /// 1 = Text 1 (dk1), 2 = lt2, 3 = dk2, 4-9 = accents, 10-11 = links),
    /// with the tint text as written.
    Theme(u8, Option<String>),
    /// The legacy 64-colour palette.
    Indexed(u8),
    /// "Automatic" (black text, no fill).
    Auto,
}

/// The legacy palette: indexes 0-7 then the 56 colours of `[ColorN]`.
pub fn indexed_color(i: u8) -> Option<u32> {
    const FIRST: [u32; 8] = [0x000000, 0xFFFFFF, 0xFF0000, 0x00FF00, 0x0000FF, 0xFFFF00, 0xFF00FF, 0x00FFFF];
    match i {
        0..=7 => Some(FIRST[i as usize]),
        8..=63 => Some(crate::numfmt::PALETTE[(i - 8) as usize]),
        64 => Some(0x000000),
        65 => Some(0xFFFFFF),
        _ => None,
    }
}

/// The colours of a workbook's theme, in SpreadsheetML order (see
/// [`ColorSpec::Theme`]).
#[derive(Clone, Debug, PartialEq)]
pub struct Theme {
    pub colors: [u32; 12],
    /// The minor (body) and major (heading) Latin fonts.
    pub minor_font: String,
    pub major_font: String,
}

impl Default for Theme {
    /// The Office 2013-2022 theme.
    fn default() -> Self {
        Theme {
            colors: [
                0xFFFFFF, 0x000000, 0xE7E6E6, 0x44546A, 0x4472C4, 0xED7D31, 0xA5A5A5, 0xFFC000, 0x5B9BD5, 0x70AD47,
                0x0563C1, 0x954F72,
            ],
            minor_font: "Calibri".into(),
            major_font: "Calibri Light".into(),
        }
    }
}

impl ColorSpec {
    /// The colour to draw, if it is not automatic.
    pub fn resolve(&self, theme: &Theme) -> Option<u32> {
        match self {
            ColorSpec::Rgb(c) => Some(*c),
            ColorSpec::Indexed(i) => indexed_color(*i),
            ColorSpec::Auto => None,
            ColorSpec::Theme(i, tint) => {
                let base = *theme.colors.get(*i as usize)?;
                let t: f64 = tint.as_deref().and_then(|t| t.parse().ok()).unwrap_or(0.0);
                Some(apply_tint(base, t))
            }
        }
    }

    /// Parse `RRGGBB` or `AARRGGBB` (alpha is ignored), with or without `#`.
    pub fn parse_hex(text: &str) -> Option<ColorSpec> {
        let t = text.trim_start_matches('#');
        let v = u32::from_str_radix(t, 16).ok()?;
        match t.len() {
            6 => Some(ColorSpec::Rgb(v)),
            8 => Some(ColorSpec::Rgb(v & 0xFF_FFFF)),
            _ => None,
        }
    }
}

/// Lighten (tint > 0) or darken (tint < 0) a colour in HSL space, as
/// Office does.
pub fn apply_tint(rgb: u32, tint: f64) -> u32 {
    if tint == 0.0 {
        return rgb;
    }
    let (h, s, l) = rgb_to_hsl(rgb);
    let l = if tint < 0.0 { l * (1.0 + tint) } else { l * (1.0 - tint) + tint };
    hsl_to_rgb(h, s, l.clamp(0.0, 1.0))
}

fn rgb_to_hsl(rgb: u32) -> (f64, f64, f64) {
    let r = ((rgb >> 16) & 0xFF) as f64 / 255.0;
    let g = ((rgb >> 8) & 0xFF) as f64 / 255.0;
    let b = (rgb & 0xFF) as f64 / 255.0;
    let max = r.max(g).max(b);
    let min = r.min(g).min(b);
    let l = (max + min) / 2.0;
    if max == min {
        return (0.0, 0.0, l);
    }
    let d = max - min;
    let s = if l > 0.5 { d / (2.0 - max - min) } else { d / (max + min) };
    let h = if max == r {
        ((g - b) / d + if g < b { 6.0 } else { 0.0 }) / 6.0
    } else if max == g {
        ((b - r) / d + 2.0) / 6.0
    } else {
        ((r - g) / d + 4.0) / 6.0
    };
    (h, s, l)
}

fn hsl_to_rgb(h: f64, s: f64, l: f64) -> u32 {
    let to = |v: f64| (v * 255.0).round().clamp(0.0, 255.0) as u32;
    if s == 0.0 {
        let v = to(l);
        return (v << 16) | (v << 8) | v;
    }
    let q = if l < 0.5 { l * (1.0 + s) } else { l + s - l * s };
    let p = 2.0 * l - q;
    let hue = |mut t: f64| {
        if t < 0.0 {
            t += 1.0;
        }
        if t > 1.0 {
            t -= 1.0;
        }
        if t < 1.0 / 6.0 {
            p + (q - p) * 6.0 * t
        } else if t < 0.5 {
            q
        } else if t < 2.0 / 3.0 {
            p + (q - p) * (2.0 / 3.0 - t) * 6.0
        } else {
            p
        }
    };
    (to(hue(h + 1.0 / 3.0)) << 16) | (to(hue(h)) << 8) | to(hue(h - 1.0 / 3.0))
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Default)]
pub enum Underline {
    #[default]
    None,
    Single,
    Double,
    SingleAccounting,
    DoubleAccounting,
}

impl Underline {
    pub fn name(self) -> &'static str {
        match self {
            Underline::None => "none",
            Underline::Single => "single",
            Underline::Double => "double",
            Underline::SingleAccounting => "singleAccounting",
            Underline::DoubleAccounting => "doubleAccounting",
        }
    }

    pub fn parse(s: &str) -> Underline {
        match s {
            "none" => Underline::None,
            "double" => Underline::Double,
            "singleAccounting" => Underline::SingleAccounting,
            "doubleAccounting" => Underline::DoubleAccounting,
            _ => Underline::Single,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct Font {
    pub name: String,
    /// Size in hundredths of a point.
    pub size: u32,
    pub bold: bool,
    pub italic: bool,
    pub underline: Underline,
    pub strike: bool,
    pub color: Option<ColorSpec>,
    /// `superscript` / `subscript`.
    pub vert_align: Option<String>,
    /// The font family number and scheme (`minor`/`major`), kept for saving.
    pub family: Option<String>,
    pub scheme: Option<String>,
}

impl Default for Font {
    fn default() -> Self {
        Font {
            name: "Calibri".into(),
            size: 1100,
            bold: false,
            italic: false,
            underline: Underline::None,
            strike: false,
            color: None,
            vert_align: None,
            family: Some("2".into()),
            scheme: Some("minor".into()),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Hash, Default)]
pub struct Fill {
    /// `none`, `solid`, `gray125`, ...
    pub pattern: String,
    pub fg: Option<ColorSpec>,
    pub bg: Option<ColorSpec>,
}

impl Fill {
    pub fn none() -> Self {
        Fill {
            pattern: "none".into(),
            fg: None,
            bg: None,
        }
    }

    pub fn solid(color: ColorSpec) -> Self {
        Fill {
            pattern: "solid".into(),
            fg: Some(color),
            bg: Some(ColorSpec::Indexed(64)),
        }
    }

    /// The colour to paint the cell, if any.
    pub fn paint(&self, theme: &Theme) -> Option<u32> {
        match self.pattern.as_str() {
            "" | "none" => None,
            "solid" => self.fg.as_ref().and_then(|c| c.resolve(theme)),
            // Patterns are drawn as their foreground colour blended lightly.
            _ => self.fg.as_ref().and_then(|c| c.resolve(theme)).or(Some(0xC0C0C0)),
        }
    }
}

/// One side of a cell's border.
#[derive(Clone, Debug, PartialEq, Eq, Hash, Default)]
pub struct Edge {
    /// `thin`, `medium`, `thick`, `dashed`, `dotted`, `double`, `hair`, ...
    pub style: String,
    pub color: Option<ColorSpec>,
}

#[derive(Clone, Debug, PartialEq, Eq, Hash, Default)]
pub struct Border {
    pub left: Option<Edge>,
    pub right: Option<Edge>,
    pub top: Option<Edge>,
    pub bottom: Option<Edge>,
    pub diagonal: Option<Edge>,
    pub diagonal_up: bool,
    pub diagonal_down: bool,
}

#[derive(Clone, Debug, PartialEq, Eq, Hash, Default)]
pub struct Align {
    /// `general`, `left`, `center`, `right`, `fill`, `justify`,
    /// `centerContinuous`, `distributed`; empty for general.
    pub horizontal: String,
    /// `top`, `center`, `bottom`, `justify`, `distributed`; empty for bottom.
    pub vertical: String,
    pub wrap: bool,
    pub indent: u8,
    pub rotation: i16,
    pub shrink: bool,
}

/// Everything about how a cell looks.
#[derive(Clone, Debug, PartialEq, Eq, Hash, Default)]
pub struct CellStyle {
    pub num_fmt: String,
    pub font: Font,
    pub fill: Fill,
    pub border: Border,
    pub align: Align,
    pub locked: bool,
    pub hidden: bool,
}

impl CellStyle {
    pub fn base() -> Self {
        CellStyle {
            num_fmt: "General".into(),
            font: Font::default(),
            fill: Fill::none(),
            border: Border::default(),
            align: Align::default(),
            locked: true,
            hidden: false,
        }
    }
}

/// The workbook's styles. Index 0 is the default style. Styles are only
/// added, never removed or renumbered, so cells in parts that are saved
/// untouched keep pointing at the right style.
#[derive(Clone, Debug)]
pub struct StyleTable {
    styles: Vec<CellStyle>,
    lookup: HashMap<CellStyle, u32>,
    /// How many styles came from the file (the rest were added).
    pub original: usize,
    pub theme: Theme,
}

impl Default for StyleTable {
    fn default() -> Self {
        StyleTable::new(vec![CellStyle::base()], Theme::default())
    }
}

impl StyleTable {
    pub fn new(mut styles: Vec<CellStyle>, theme: Theme) -> Self {
        if styles.is_empty() {
            styles.push(CellStyle::base());
        }
        let mut lookup = HashMap::new();
        for (i, s) in styles.iter().enumerate() {
            lookup.entry(s.clone()).or_insert(i as u32);
        }
        let original = styles.len();
        StyleTable {
            styles,
            lookup,
            original,
            theme,
        }
    }

    pub fn len(&self) -> usize {
        self.styles.len()
    }

    pub fn is_empty(&self) -> bool {
        self.styles.is_empty()
    }

    /// A style by index (the default style for unknown indexes).
    pub fn get(&self, id: u32) -> &CellStyle {
        self.styles.get(id as usize).unwrap_or(&self.styles[0])
    }

    /// The index of a style, adding it if it is new.
    pub fn intern(&mut self, style: CellStyle) -> u32 {
        if let Some(&id) = self.lookup.get(&style) {
            return id;
        }
        let id = self.styles.len() as u32;
        self.lookup.insert(style.clone(), id);
        self.styles.push(style);
        id
    }

    /// Styles added since the file was read.
    pub fn added(&self) -> &[CellStyle] {
        &self.styles[self.original..]
    }

    pub fn all(&self) -> &[CellStyle] {
        &self.styles
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tints_and_themes() {
        assert_eq!(apply_tint(0x4472C4, 0.0), 0x4472C4);
        let near = |a: u32, b: u32| (0..3).all(|i| (((a >> (8 * i)) & 0xFF) as i32 - ((b >> (8 * i)) & 0xFF) as i32).abs() <= 3);
        // "Blue, Accent 1, Lighter 80%" in Excel is D9E1F2.
        assert!(near(apply_tint(0x4472C4, 0.7999816888943144), 0xD9E1F2));
        // "White, Background 1, Darker 15%" is D9D9D9.
        assert!(near(apply_tint(0xFFFFFF, -0.1499984740745262), 0xD9D9D9));
        let theme = Theme::default();
        assert_eq!(ColorSpec::Theme(1, None).resolve(&theme), Some(0));
        assert_eq!(ColorSpec::parse_hex("FFFF0000"), Some(ColorSpec::Rgb(0xFF0000)));
        assert_eq!(indexed_color(10), Some(0xFF0000));
    }

    #[test]
    fn styles_are_interned() {
        let mut t = StyleTable::default();
        let mut bold = CellStyle::base();
        bold.font.bold = true;
        let a = t.intern(bold.clone());
        let b = t.intern(bold);
        assert_eq!(a, b);
        assert_eq!(a, 1);
        assert_eq!(t.intern(CellStyle::base()), 0);
        assert_eq!(t.added().len(), 1);
    }
}
