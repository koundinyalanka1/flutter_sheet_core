//! Sheets to PDF: each sheet's used range printed on pages, as Excel
//! prints them (down, then across), with cell fills, borders, fonts,
//! alignment, wrapping, merged cells and text running into empty
//! neighbours. A sheet a little wider than a page is scaled to fit.

use doc_font::FontBook;
use doc_layout::{Align, LineMetric, LineSpacing, Paragraph, Span, Style};
use doc_pdf::{Cap, Pdf, Stroke};

use crate::display::shown;
use crate::model::{SheetState, Workbook};
use crate::refs::{CellRef, Range};
use crate::style::{CellStyle, Edge};
use crate::value::Value;

#[derive(Clone, Debug)]
pub struct PdfOptions {
    /// Sheets to print (indexes); empty for every visible sheet.
    pub sheets: Vec<usize>,
    /// Page size in points (portrait); A4 by default.
    pub page: (f32, f32),
    /// Light grid lines between cells, as the screen shows them.
    pub gridlines: bool,
    pub title: Option<String>,
}

impl Default for PdfOptions {
    fn default() -> Self {
        PdfOptions {
            sheets: Vec::new(),
            page: (595.0, 842.0),
            gridlines: true,
            title: None,
        }
    }
}

/// Excel's margins: 0.7in left and right, 0.75in top and bottom.
const MARGIN_X: f32 = 50.4;
const MARGIN_Y: f32 = 54.0;
const MAX_PAGES: usize = 1000;

/// A column width in characters as points (Calibri 11's 7-pixel digit).
fn col_points(chars: f64) -> f32 {
    if chars <= 0.0 {
        return 0.0;
    }
    ((chars * 7.0 + 5.0) * 0.75) as f32
}

fn edge_stroke(e: &Edge, theme: &crate::style::Theme) -> Option<Stroke> {
    let color = 0xFF00_0000 | e.color.as_ref().and_then(|c| c.resolve(theme)).unwrap_or(0);
    let (width, dash): (f32, Vec<f32>) = match e.style.as_str() {
        "" | "none" => return None,
        "hair" => (0.25, vec![]),
        "thin" => (0.5, vec![]),
        "medium" => (1.0, vec![]),
        "thick" => (1.5, vec![]),
        "double" => (1.5, vec![]),
        "dotted" => (0.5, vec![0.5, 1.5]),
        "dashed" => (0.5, vec![3.0, 1.5]),
        "mediumDashed" => (1.0, vec![4.0, 2.0]),
        "dashDot" | "mediumDashDot" | "slantDashDot" => (if e.style.starts_with("medium") { 1.0 } else { 0.5 }, vec![3.0, 1.5, 1.0, 1.5]),
        "dashDotDot" | "mediumDashDotDot" => (if e.style.starts_with("medium") { 1.0 } else { 0.5 }, vec![3.0, 1.5, 1.0, 1.5, 1.0, 1.5]),
        _ => (0.5, vec![]),
    };
    Some(Stroke { width, color, dash, cap: Cap::Butt, round_join: false })
}

fn text_style(wb: &Workbook, st: &CellStyle, color: Option<u32>) -> Style {
    let f = &st.font;
    let theme = &wb.styles.theme;
    let base = color.or_else(|| f.color.as_ref().and_then(|c| c.resolve(theme))).unwrap_or(0);
    Style {
        family: f.name.clone(),
        size: f.size as f32 / 100.0,
        bold: f.bold,
        italic: f.italic,
        color: 0xFF00_0000 | base,
        underline: f.underline != crate::style::Underline::None,
        strike: f.strike,
        baseline: match f.vert_align.as_deref() {
            Some("superscript") => 33,
            Some("subscript") => -15,
            _ => 0,
        },
        ..Style::default()
    }
}

/// The whole workbook (or some sheets) as a PDF.
pub fn export(wb: &Workbook, book: &mut FontBook, options: &PdfOptions) -> Vec<u8> {
    let mut pdf = Pdf::new();
    if let Some(t) = &options.title {
        pdf.set_title(t);
    }
    let sheets: Vec<usize> = if options.sheets.is_empty() {
        (0..wb.sheets.len()).filter(|i| wb.sheets[*i].state == SheetState::Visible).collect()
    } else {
        options.sheets.iter().copied().filter(|i| *i < wb.sheets.len()).collect()
    };
    for si in sheets {
        if pdf.page_count() >= MAX_PAGES {
            break;
        }
        export_sheet(wb, si, book, &mut pdf, options);
    }
    if pdf.page_count() == 0 {
        let p = pdf.page(options.page.0, options.page.1);
        pdf.add_page(p);
    }
    pdf.finish()
}

fn export_sheet(wb: &Workbook, si: usize, book: &mut FontBook, pdf: &mut Pdf, options: &PdfOptions) {
    let sh = &wb.sheets[si];
    let Some(used) = sh.used_range() else { return };
    // Merged ranges reaching past the used range widen it.
    let mut last_row = used.end.row;
    let mut last_col = used.end.col;
    for m in &sh.merges {
        last_row = last_row.max(m.end.row);
        last_col = last_col.max(m.end.col);
    }
    let (first_row, first_col) = (0u32, 0u32);
    let cols: Vec<u32> = (first_col..=last_col).filter(|c| !sh.col_hidden(*c)).collect();
    let rows: Vec<u32> = (first_row..=last_row).filter(|r| !sh.row_hidden(*r)).collect();
    if cols.is_empty() || rows.is_empty() {
        return;
    }
    let widths: Vec<f32> = cols.iter().map(|c| col_points(sh.col_width(*c))).collect();
    let heights: Vec<f32> = rows.iter().map(|r| sh.row_height(*r) as f32).collect();
    let total_w: f32 = widths.iter().sum();
    // Wide sheets turn the page; a little too wide, they shrink to fit.
    let (pw, ph) = options.page;
    let portrait_w = pw - 2.0 * MARGIN_X;
    let (page_w, page_h) = if total_w > portrait_w { (ph, pw) } else { (pw, ph) };
    let area_w = page_w - 2.0 * MARGIN_X;
    let area_h = page_h - 2.0 * MARGIN_Y - 14.0;
    let scale = if total_w > area_w && total_w <= area_w * 2.0 { area_w / total_w } else { 1.0 };
    // Column and row breaks.
    let breaks = |sizes: &[f32], room: f32| -> Vec<(usize, usize)> {
        let mut out = Vec::new();
        let mut start = 0;
        let mut acc = 0.0;
        for (i, s) in sizes.iter().enumerate() {
            let s = s * scale;
            if acc + s > room && i > start {
                out.push((start, i));
                start = i;
                acc = 0.0;
            }
            acc += s;
        }
        out.push((start, sizes.len()));
        out
    };
    let col_pages = breaks(&widths, area_w);
    let row_pages = breaks(&heights, area_h);
    let theme = &wb.styles.theme;
    let mut page_no = 0;
    let total_pages = col_pages.len() * row_pages.len();
    for &(c0, c1) in &col_pages {
        for &(r0, r1) in &row_pages {
            if pdf.page_count() >= MAX_PAGES {
                return;
            }
            page_no += 1;
            let mut page = pdf.page(page_w, page_h);
            // Column and row edges on this page, in sheet points.
            let mut xs = vec![0.0f32];
            for w in &widths[c0..c1] {
                xs.push(xs.last().unwrap() + w);
            }
            let mut ys = vec![0.0f32];
            for h in &heights[r0..r1] {
                ys.push(ys.last().unwrap() + h);
            }
            page.save();
            page.translate(MARGIN_X, MARGIN_Y);
            page.scale(scale, scale);
            // Where a merged range's first cell is drawn from.
            let merge_box = |r: u32, c: u32| -> Option<(Range, f32, f32, f32, f32)> {
                let m = sh.merge_at(CellRef { row: r, col: c })?;
                let x0 = cols.iter().position(|x| *x >= m.start.col).map(|i| i as i64 - c0 as i64)?;
                let x1 = cols.iter().position(|x| *x > m.end.col).unwrap_or(cols.len()) as i64 - c0 as i64;
                let y0 = rows.iter().position(|x| *x >= m.start.row).map(|i| i as i64 - r0 as i64)?;
                let y1 = rows.iter().position(|x| *x > m.end.row).unwrap_or(rows.len()) as i64 - r0 as i64;
                // An edge `k` columns (rows) from the page's first, where
                // `v` holds the page's edges and `all` every size.
                let edge = |v: &[f32], all: &[f32], base: usize, k: i64| -> f32 {
                    if k < 0 {
                        let from = (base as i64 + k).max(0) as usize;
                        -all[from.min(base)..base.min(all.len())].iter().sum::<f32>()
                    } else if (k as usize) < v.len() {
                        v[k as usize]
                    } else {
                        let from = (base + v.len() - 1).min(all.len());
                        let to = (base + k as usize).min(all.len()).max(from);
                        *v.last().unwrap() + all[from..to].iter().sum::<f32>()
                    }
                };
                Some((m, edge(&xs, &widths, c0, x0), edge(&ys, &heights, r0, y0), edge(&xs, &widths, c0, x1), edge(&ys, &heights, r0, y1)))
            };
            // Fills first.
            for (ri, &r) in rows[r0..r1].iter().enumerate() {
                for (ci, &c) in cols[c0..c1].iter().enumerate() {
                    let at = CellRef { row: r, col: c };
                    let Some(cell) = sh.cell(at) else { continue };
                    let st = wb.styles.get(cell.style);
                    if let Some(fill) = st.fill.paint(theme) {
                        let (x0, y0, x1, y1) = match merge_box(r, c) {
                            Some((m, a, b, cc, d)) if m.start == at => (a, b, cc, d),
                            Some(_) => continue,
                            None => (xs[ci], ys[ri], xs[ci + 1], ys[ri + 1]),
                        };
                        page.rect(x0, y0, x1 - x0, y1 - y0);
                        page.fill(0xFF00_0000 | fill, false);
                    }
                }
            }
            // Grid lines, except where text runs over a boundary or inside
            // merged cells.
            if options.gridlines {
                let mut hidden_v: std::collections::HashSet<(usize, usize)> = std::collections::HashSet::new();
                let mut hidden_h: std::collections::HashSet<(usize, usize)> = std::collections::HashSet::new();
                for (ri, &r) in rows[r0..r1].iter().enumerate() {
                    for (ci, &c) in cols[c0..c1].iter().enumerate() {
                        let at = CellRef { row: r, col: c };
                        if let Some(m) = sh.merge_at(at) {
                            if at.col < m.end.col {
                                hidden_v.insert((ri, ci + 1));
                            }
                            if at.row < m.end.row {
                                hidden_h.insert((ri + 1, ci));
                            }
                            continue;
                        }
                        let Some(cell) = sh.cell(at) else { continue };
                        let st = wb.styles.get(cell.style);
                        if cell.value.is_empty() || st.align.wrap || !matches!(cell.value, Value::Text(_)) {
                            continue;
                        }
                        let text = shown(wb, cell).text;
                        let style = text_style(wb, st, None);
                        let w = text.chars().count() as f32 * style.size * 0.5;
                        let right = matches!(st.align.horizontal.as_str(), "right");
                        let mut k = ci;
                        let mut room = widths[c0 + ci];
                        if !right {
                            while room < w && k + 1 < c1 - c0 && sh.cell(CellRef { row: r, col: cols[c0 + k + 1] }).is_none_or(|n| n.value.is_empty()) {
                                k += 1;
                                hidden_v.insert((ri, k));
                                room += widths[c0 + k];
                            }
                        }
                    }
                }
                let grid = Stroke { width: 0.25, color: 0xFFD0_D0D0, dash: Vec::new(), cap: Cap::Butt, round_join: false };
                for (k, x) in xs.iter().enumerate() {
                    for ri in 0..r1 - r0 {
                        if !hidden_v.contains(&(ri, k)) {
                            page.move_to(*x, ys[ri]);
                            page.line_to(*x, ys[ri + 1]);
                        }
                    }
                }
                for (k, y) in ys.iter().enumerate() {
                    for ci in 0..c1 - c0 {
                        if !hidden_h.contains(&(k, ci)) {
                            page.move_to(xs[ci], *y);
                            page.line_to(xs[ci + 1], *y);
                        }
                    }
                }
                page.stroke(&grid);
            }
            // Text.
            for (ri, &r) in rows[r0..r1].iter().enumerate() {
                for (ci, &c) in cols[c0..c1].iter().enumerate() {
                    let at = CellRef { row: r, col: c };
                    let Some(cell) = sh.cell(at) else { continue };
                    if cell.value.is_empty() {
                        continue;
                    }
                    let (mut x0, mut y0, mut x1, mut y1) = (xs[ci], ys[ri], xs[ci + 1], ys[ri + 1]);
                    let merged = merge_box(r, c);
                    if let Some((m, a, b, cc, d)) = merged {
                        if m.start != at {
                            continue;
                        }
                        (x0, y0, x1, y1) = (a, b, cc, d);
                    }
                    let st = wb.styles.get(cell.style);
                    let s = shown(wb, cell);
                    if s.text.is_empty() {
                        continue;
                    }
                    let style = text_style(wb, st, s.color);
                    let number = matches!(cell.value, Value::Number(_));
                    let h = match st.align.horizontal.as_str() {
                        "center" | "centerContinuous" => Align::Center,
                        "right" => Align::Right,
                        "justify" | "distributed" => Align::Justify,
                        "left" | "fill" => Align::Left,
                        _ => {
                            if number {
                                Align::Right
                            } else if matches!(cell.value, Value::Bool(_) | Value::Error(_)) {
                                Align::Center
                            } else {
                                Align::Left
                            }
                        }
                    };
                    let pad = 2.0 + st.align.indent as f32 * 9.0;
                    let mut text = s.text.clone();
                    let cell_w = x1 - x0;
                    // Text runs on into empty cells to its right (or left).
                    let mut clip_l = x0;
                    let mut clip_r = x1;
                    if !st.align.wrap && !number && merged.is_none() {
                        let mut k = ci + 1;
                        while k < c1 - c0 && sh.cell(CellRef { row: r, col: cols[c0 + k] }).is_none_or(|n| n.value.is_empty()) && h == Align::Left {
                            clip_r = xs[k + 1];
                            k += 1;
                        }
                        let mut k = ci;
                        while k > 0 && sh.cell(CellRef { row: r, col: cols[c0 + k - 1] }).is_none_or(|n| n.value.is_empty()) && h == Align::Right {
                            clip_l = xs[k - 1];
                            k -= 1;
                        }
                    }
                    let para = Paragraph {
                        spans: vec![Span { text: text.clone(), style: style.clone() }],
                        align: h,
                        left: pad,
                        right: pad,
                        spacing: LineSpacing::Multiple(1.0),
                        metric: LineMetric::Font,
                        mark: style.clone(),
                        ..Paragraph::default()
                    };
                    let mut laid = if st.align.wrap {
                        doc_layout::layout(book, &para, cell_w)
                    } else {
                        doc_layout::layout(book, &para, 100_000.0)
                    };
                    if !st.align.wrap {
                        let used = doc_layout::max_line_width(&laid) + 2.0 * pad;
                        if number && used > cell_w + 0.5 {
                            // Numbers that do not fit show #'s, as in Excel.
                            let n = ((cell_w - 2.0 * pad) / (style.size * 0.55)).floor().max(1.0) as usize;
                            text = "#".repeat(n);
                            laid = doc_layout::layout(book, &Paragraph { spans: vec![Span { text: text.clone(), style: style.clone() }], ..para.clone() }, 100_000.0);
                        }
                        // Align within the cell (or the merged box).
                        let used = doc_layout::max_line_width(&laid);
                        for line in &mut laid.lines {
                            let shift = match h {
                                Align::Center => (cell_w - used) / 2.0 - line.left,
                                Align::Right => cell_w - pad - used - line.left,
                                _ => 0.0,
                            };
                            for run in &mut line.runs {
                                run.x += shift;
                            }
                        }
                    }
                    let top = match st.align.vertical.as_str() {
                        "top" => y0 + 1.0,
                        "center" | "justify" | "distributed" => y0 + (y1 - y0 - laid.height) / 2.0,
                        _ => y1 - laid.height - 1.0,
                    };
                    page.save();
                    page.rect(clip_l, y0, clip_r - clip_l, y1 - y0);
                    page.clip(false);
                    doc_layout::draw(&mut page, pdf, &laid, x0, top);
                    page.restore();
                }
            }
            // Borders last, over everything.
            for (ri, &r) in rows[r0..r1].iter().enumerate() {
                for (ci, &c) in cols[c0..c1].iter().enumerate() {
                    let at = CellRef { row: r, col: c };
                    let Some(cell) = sh.cell(at) else { continue };
                    let b = &wb.styles.get(cell.style).border;
                    let (x0, y0, x1, y1) = (xs[ci], ys[ri], xs[ci + 1], ys[ri + 1]);
                    for (edge, (a, bb)) in [
                        (&b.left, ((x0, y0), (x0, y1))),
                        (&b.top, ((x0, y0), (x1, y0))),
                        (&b.right, ((x1, y0), (x1, y1))),
                        (&b.bottom, ((x0, y1), (x1, y1))),
                    ] {
                        if let Some(s) = edge.as_ref().and_then(|e| edge_stroke(e, theme)) {
                            page.move_to(a.0, a.1);
                            page.line_to(bb.0, bb.1);
                            page.stroke(&s);
                        }
                    }
                }
            }
            page.restore();
            // A small footer: the sheet and page.
            let footer = if total_pages > 1 { format!("{} \u{2014} page {page_no} of {total_pages}", sh.name) } else { sh.name.clone() };
            let fs = Style { family: "Calibri".into(), size: 8.0, color: 0xFF80_8080, ..Style::default() };
            let laid = doc_layout::layout(book, &Paragraph { spans: vec![Span { text: footer, style: fs.clone() }], align: Align::Center, mark: fs, ..Paragraph::default() }, page_w);
            doc_layout::draw(&mut page, pdf, &laid, 0.0, page_h - MARGIN_Y / 2.0 - 6.0);
            pdf.add_page(page);
        }
    }
}
