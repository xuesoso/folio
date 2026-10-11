//! GFM tables: auto column widths, wrapped and aligned cells, booktabs-style
//! rules, and row-wise pagination that repeats the header row.

use super::fonts::Fonts;
use super::layout::{Ctx, Item, Writer, emit_line, extents, min_max_width, spans_to_items, wrap};
use super::parse::{Alignment, Span, SpanStyle};
use super::{BODY_SIZE, MARGIN, PAGE_H, PAGE_W};

const PAD_X: f64 = 4.0;
const PAD_Y: f64 = 2.5;
/// Smallest font size a too-wide table shrinks to before breaking words.
const MIN_SIZE: f64 = 8.0;
const HEADER_FILL: f64 = 0.93;
const RULE_DARK: f64 = 0.25;
const RULE_LIGHT: f64 = 0.82;
/// Tables up to this share of a page's height are never split across pages.
const KEEP_TOGETHER: f64 = 0.3;

/// One cell wrapped into lines, with each line's ascent/descent.
struct CellLayout {
    lines: Vec<(Vec<Item>, f64, f64)>,
    height: f64,
}

/// Column widths for content with the given minimum (widest word) and maximum
/// (unwrapped) widths, padding included: natural widths if they fit, else each
/// column's minimum plus a share of the remaining space proportional to how
/// much it would like to grow; if even the minimums don't fit, the minimums
/// scaled down (words then break inside cells).
pub fn column_widths(min: &[f64], max: &[f64], avail: f64) -> Vec<f64> {
    let sum_max: f64 = max.iter().sum();
    if sum_max <= avail {
        return max.to_vec();
    }
    let sum_min: f64 = min.iter().sum();
    if sum_min >= avail {
        return min.iter().map(|m| m * avail / sum_min).collect();
    }
    let extra = avail - sum_min;
    let grow = sum_max - sum_min;
    min.iter()
        .zip(max)
        .map(|(lo, hi)| lo + extra * (hi - lo) / grow)
        .collect()
}

/// Estimated height a table with `rows` body rows must keep together at the
/// top of its page (assuming one-line cells): all of it if short, else the
/// header plus one row. Mirrors the rule in `render_table`.
pub fn keep_height(rows: usize, scale: f64) -> f64 {
    let row = BODY_SIZE * scale * 1.3 + 2.0 * PAD_Y;
    let total = row * (rows + 1) as f64;
    if total <= KEEP_TOGETHER * (PAGE_H - 2.0 * MARGIN) {
        total
    } else {
        2.0 * row
    }
}

pub fn render_table(
    w: &mut Writer,
    aligns: &[Alignment],
    head: &[Vec<Span>],
    rows: &[Vec<Vec<Span>>],
    ctx: Ctx,
    fonts: &mut Fonts,
) {
    let cols = aligns.len();
    if cols == 0 {
        return;
    }
    let avail = (PAGE_W - MARGIN) - ctx.x;
    let bold_head: Vec<Vec<Span>> = head
        .iter()
        .map(|cell| {
            cell.iter()
                .map(|s| Span {
                    style: SpanStyle { bold: true, ..s.style },
                    ..s.clone()
                })
                .collect()
        })
        .collect();

    // Measure, shrinking the font until the widest words fit (or MIN_SIZE).
    let mut size = BODY_SIZE * ctx.scale;
    let (items_head, items_rows, widths) = loop {
        let ih: Vec<Vec<Item>> = bold_head.iter().map(|c| spans_to_items(c, size, ctx.color, fonts)).collect();
        let ir: Vec<Vec<Vec<Item>>> = rows
            .iter()
            .map(|r| r.iter().map(|c| spans_to_items(c, size, ctx.color, fonts)).collect())
            .collect();
        let mut min = vec![0.0f64; cols];
        let mut max = vec![0.0f64; cols];
        for row in std::iter::once(&ih).chain(ir.iter()) {
            for (j, cell) in row.iter().enumerate().take(cols) {
                let (lo, hi) = min_max_width(cell);
                min[j] = min[j].max(lo + 2.0 * PAD_X);
                max[j] = max[j].max(hi + 2.0 * PAD_X);
            }
        }
        if min.iter().sum::<f64>() <= avail || size <= MIN_SIZE {
            let widths = column_widths(&min, &max, avail);
            break (ih, ir, widths);
        }
        size = (size - 0.5).max(MIN_SIZE);
    };
    let table_w: f64 = widths.iter().sum();
    let line_h = size * 1.3;

    let layout_row = |cells: &[Vec<Item>]| -> Vec<CellLayout> {
        cells
            .iter()
            .zip(&widths)
            .map(|(items, cw)| {
                let lines: Vec<(Vec<Item>, f64, f64)> = wrap(items, cw - 2.0 * PAD_X)
                    .into_iter()
                    .map(|l| {
                        let (above, below) = extents(&l, line_h);
                        (l, 0.78 * line_h + above, 0.22 * line_h + below)
                    })
                    .collect();
                let height = lines.iter().map(|(_, a, d)| a + d).sum();
                CellLayout { lines, height }
            })
            .collect()
    };
    let row_height = |cells: &[CellLayout]| -> f64 {
        cells.iter().map(|c| c.height).fold(line_h, f64::max) + 2.0 * PAD_Y
    };

    let head_cells = layout_row(&items_head);
    let head_h = row_height(&head_cells);
    let body: Vec<(Vec<CellLayout>, f64)> = items_rows
        .iter()
        .map(|r| {
            let cells = layout_row(r);
            let h = row_height(&cells);
            (cells, h)
        })
        .collect();

    let x0 = ctx.x;
    let draw_row = |w: &mut Writer, cells: &[CellLayout], top: f64| {
        let mut x = x0;
        for (j, cell) in cells.iter().enumerate() {
            let cw = widths[j];
            let mut y = top - PAD_Y;
            for (line, above, below) in &cell.lines {
                let baseline = y - above;
                let lw: f64 = line.iter().map(|i| i.width()).sum();
                let inner = cw - 2.0 * PAD_X;
                let dx = match aligns[j] {
                    Alignment::Right => inner - lw,
                    Alignment::Center => (inner - lw) / 2.0,
                    Alignment::Left | Alignment::None => 0.0,
                };
                emit_line(&mut w.cur, x + PAD_X + dx.max(0.0), baseline, line);
                y = baseline - below;
            }
            x += cw;
        }
    };
    let draw_header = |w: &mut Writer| -> f64 {
        let top = w.y;
        w.fill_rect(x0, top - head_h, table_w, head_h, HEADER_FILL);
        w.hline(x0, x0 + table_w, top, 0.8, RULE_DARK);
        draw_row(w, &head_cells, top);
        w.hline(x0, x0 + table_w, top - head_h, 0.5, RULE_DARK);
        top - head_h
    };

    // A short table moves to the next page whole rather than splitting; a long
    // one needs at least its header plus the first row, so a header never sits
    // alone at the foot of a page.
    w.settle();
    let total: f64 = head_h + body.iter().map(|(_, h)| h).sum::<f64>();
    let first_h = body.first().map(|(_, h)| *h).unwrap_or(0.0);
    let keep = if total <= KEEP_TOGETHER * (PAGE_H - 2.0 * MARGIN) {
        total
    } else {
        head_h + first_h
    };
    if w.y - keep < MARGIN && w.y < PAGE_H - MARGIN {
        w.new_page();
    }
    let mut y = draw_header(w);
    for (i, (cells, h)) in body.iter().enumerate() {
        if y - h < MARGIN && y < PAGE_H - MARGIN - head_h {
            w.hline(x0, x0 + table_w, y, 0.8, RULE_DARK);
            w.new_page();
            y = draw_header(w);
        } else if i > 0 {
            w.hline(x0, x0 + table_w, y, 0.3, RULE_LIGHT);
        }
        draw_row(w, cells, y);
        y -= h;
    }
    w.hline(x0, x0 + table_w, y, 0.8, RULE_DARK);
    w.y = y - PAD_Y;
}

#[cfg(test)]
mod tests {
    use super::super::layout::render_blocks;
    use super::super::parse::parse;
    use super::*;

    /// Render Markdown starting at baseline `y`; returns the page contents.
    fn render(md: &str, y: f64) -> Vec<String> {
        let mut fonts = Fonts::new().unwrap();
        let doc = parse(md);
        let mut w = Writer::new();
        w.y = y;
        let ctx = Ctx {
            x: MARGIN,
            color: None,
            scale: 1.0,
        };
        render_blocks(&mut w, &doc.blocks, ctx, &mut fonts);
        w.into_pages()
    }

    fn shows(page: &str, text: &str) -> bool {
        page.contains(&format!("({text}) Tj"))
    }

    #[test]
    fn long_tables_repeat_the_header_on_every_page() {
        let mut md = String::from("| Index | Square |\n|--:|--:|\n");
        for i in 0..120 {
            md.push_str(&format!("| {i} | {} |\n", i * i));
        }
        let pages = render(&md, PAGE_H - MARGIN);
        assert!(pages.len() > 1);
        for (n, page) in pages.iter().enumerate() {
            assert!(shows(page, "Index") && shows(page, "Square"), "header missing on page {n}");
        }
        assert!(shows(pages.last().unwrap(), "119"), "every row is drawn");
    }

    #[test]
    fn short_tables_move_whole_with_their_heading() {
        let md = "## Constants\n\n| a | b |\n|---|---|\n| 1 | 2 |\n| 3 | 4 |\n";
        // Near the foot of a page: heading and table move to the next one.
        let pages = render(md, MARGIN + 60.0);
        assert_eq!(pages.len(), 2);
        assert!(!shows(&pages[0], "Constants") && !shows(&pages[0], "a"));
        assert!(shows(&pages[1], "Constants") && shows(&pages[1], "3"));
        // With room to spare, everything stays put.
        assert_eq!(render(md, PAGE_H - MARGIN).len(), 1);
    }

    #[test]
    fn long_tables_start_where_their_header_and_a_row_fit() {
        let mut md = String::from("| n |\n|---|\n");
        for i in 0..80 {
            md.push_str(&format!("| r{i} |\n"));
        }
        let pages = render(&md, PAGE_H / 2.0);
        assert!(shows(&pages[0], "n") && shows(&pages[0], "r0"), "a long table starts mid-page");
    }

    #[test]
    fn over_long_words_wrap_inside_their_cell() {
        let word = "m".repeat(200);
        let md = format!("| a | b |\n|---|---|\n| x | {word} |\n");
        let pages = render(&md, PAGE_H - MARGIN);
        let page = &pages[0];
        assert!(!shows(page, &word), "the word is broken up");
        let pieces = page.matches("(mmm").count();
        assert!(pieces > 1, "into several lines, got {pieces}");
        // Every line starts inside the text area.
        for line in page.lines().filter(|l| l.ends_with(" Tm")) {
            let x: f64 = line.split_whitespace().nth(4).unwrap().parse().unwrap();
            assert!(x < PAGE_W - MARGIN, "line starts past the margin at {x}");
        }
    }

    #[test]
    fn natural_widths_when_they_fit() {
        assert_eq!(column_widths(&[10.0, 10.0], &[30.0, 40.0], 100.0), vec![30.0, 40.0]);
    }

    #[test]
    fn extra_space_shared_by_desire_to_grow() {
        let w = column_widths(&[10.0, 10.0], &[30.0, 110.0], 100.0);
        assert!((w.iter().sum::<f64>() - 100.0).abs() < 1e-9);
        // 80 extra split 20:100 → 13.33 and 66.67.
        assert!((w[0] - (10.0 + 80.0 * 20.0 / 120.0)).abs() < 1e-9);
        assert!(w[1] > w[0]);
    }

    #[test]
    fn minimums_scale_down_when_nothing_fits() {
        let w = column_widths(&[60.0, 90.0], &[80.0, 120.0], 100.0);
        assert!((w.iter().sum::<f64>() - 100.0).abs() < 1e-9);
        assert!((w[0] / w[1] - 60.0 / 90.0).abs() < 1e-9);
    }
}
