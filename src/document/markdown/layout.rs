//! Block tree → laid-out PDF content streams (text layout + pagination).

use std::fmt::Write as _;
use std::rc::Rc;

use super::fonts::{EmbeddedFont, FALLBACK_NAME, FontId, Fonts, MATH_NAME, font_for, winansi_byte};
use super::math::{self, MathBox, MathOp};
use super::parse::{Block, Doc, ListItem, Span, SpanKind, SpanStyle};
use super::table;
use super::{BODY_SIZE, CODE_SIZE, LIST_INDENT, MARGIN, MATH_SCALE, PAGE_H, PAGE_W, QUOTE_INDENT};

pub type Rgb = [u8; 3];
pub const BLACK: Rgb = [0, 0, 0];
const INLINE_CODE_COLOR: Rgb = [0x80, 0x30, 0x30];
const CODE_BLOCK_COLOR: Rgb = [0x20, 0x20, 0x20];
const QUOTE_COLOR: Rgb = [0x55, 0x55, 0x55];
const LINK_COLOR: Rgb = [0x1F, 0x5F, 0xAF];

/// Text decorations, as bit flags on a cell.
const DECO_STRIKE: u8 = 1;
const DECO_UNDERLINE: u8 = 2;

/// Share of a line's height above / below the baseline. Inline content taller
/// than this (math) grows the line.
const LINE_ASCENT: f64 = 0.78;
const LINE_DESCENT: f64 = 0.22;

/// How a single character is drawn.
#[derive(Clone, Copy)]
pub enum Draw {
    /// A WinAnsi byte in a base-14 font.
    Win { byte: u8, font: FontId },
    /// A glyph id in the embedded fallback font.
    Uni { gid: u16 },
}

/// One laid-out character (or space).
#[derive(Clone, Copy)]
pub struct Cell {
    pub is_space: bool,
    pub width: f64,
    pub size: f64,
    pub color: Rgb,
    pub draw: Draw,
    /// Baseline shift (superscripts), in points.
    pub rise: f64,
    pub deco: u8,
}

/// One unit on a line: a character, or an unbreakable inline formula.
#[derive(Clone)]
pub enum Item {
    Cell(Cell),
    Math(Rc<MathBox>, Rgb),
}

impl Item {
    pub fn width(&self) -> f64 {
        match self {
            Item::Cell(c) => c.width,
            Item::Math(b, _) => b.width,
        }
    }

    fn is_space(&self) -> bool {
        matches!(self, Item::Cell(c) if c.is_space)
    }
}

/// Style shared by the cells of one text run.
#[derive(Clone, Copy)]
pub struct CellStyle {
    pub font: FontId,
    pub size: f64,
    pub color: Rgb,
    pub rise: f64,
    pub deco: u8,
}

impl CellStyle {
    pub fn plain(font: FontId, size: f64, color: Rgb) -> Self {
        CellStyle {
            font,
            size,
            color,
            rise: 0.0,
            deco: 0,
        }
    }
}

/// Where and how a run of blocks is drawn.
#[derive(Clone, Copy)]
pub struct Ctx {
    pub x: f64,
    /// Colour override (block quotes), else per-span colours.
    pub color: Option<Rgb>,
    /// Size multiplier (footnotes are set smaller).
    pub scale: f64,
}

/// Accumulates per-page content streams (all ASCII) while tracking the baseline.
pub struct Writer {
    pages: Vec<String>,
    pub cur: String,
    pub y: f64,
    /// A list marker to draw on the next line, at the given x.
    pending: Option<(f64, Vec<Item>)>,
    /// Depth of the previous line beyond its normal share (tall inline math),
    /// to clear before the next line.
    carry: f64,
}

impl Writer {
    pub fn new() -> Self {
        Writer {
            pages: Vec::new(),
            cur: String::new(),
            y: PAGE_H - MARGIN,
            pending: None,
            carry: 0.0,
        }
    }

    pub fn new_page(&mut self) {
        self.pages.push(std::mem::take(&mut self.cur));
        self.y = PAGE_H - MARGIN;
        self.carry = 0.0;
    }

    pub fn into_pages(mut self) -> Vec<String> {
        if !self.cur.is_empty() || self.pages.is_empty() {
            self.pages.push(self.cur);
        }
        self.pages
    }

    fn gap(&mut self, amount: f64) {
        self.y -= amount;
    }

    /// Consume any pending depth from the previous line.
    pub fn settle(&mut self) {
        self.y -= self.carry;
        self.carry = 0.0;
    }

    /// Advance the baseline by `line_h` (plus whatever tall items on this line
    /// need, breaking pages as needed) and draw the pending marker plus `items`.
    fn place(&mut self, x: f64, line_h: f64, items: &[Item]) {
        let (above, below) = extents(items, line_h);
        self.y -= line_h + above + self.carry;
        if self.y - below < MARGIN {
            self.new_page();
            self.y = PAGE_H - MARGIN - line_h - above;
        }
        self.carry = below;
        let y = self.y;
        if let Some((mx, marker)) = self.pending.take() {
            emit_line(&mut self.cur, mx, y, &marker);
        }
        if !items.is_empty() {
            emit_line(&mut self.cur, x, y, items);
        }
    }

    /// Reserve a box `height` above and `depth` below a new baseline, returning
    /// that baseline.
    fn place_box(&mut self, height: f64, depth: f64) -> f64 {
        self.y -= height + self.carry;
        if self.y - depth < MARGIN {
            self.new_page();
            self.y = PAGE_H - MARGIN - height;
        }
        self.carry = depth;
        self.y
    }

    /// A horizontal line, `width` thick, in grey level `gray` (0 = black).
    pub fn hline(&mut self, x1: f64, x2: f64, y: f64, width: f64, gray: f64) {
        let _ = writeln!(
            &mut self.cur,
            "{gray:.3} {gray:.3} {gray:.3} RG {width:.2} w {x1:.2} {y:.2} m {x2:.2} {y:.2} l S"
        );
    }

    /// A filled rectangle (lower-left corner at x, y) in grey level `gray`.
    pub fn fill_rect(&mut self, x: f64, y: f64, w: f64, h: f64, gray: f64) {
        let _ = writeln!(
            &mut self.cur,
            "{gray:.3} {gray:.3} {gray:.3} rg {x:.2} {y:.2} {w:.2} {h:.2} re f"
        );
    }
}

/// Extra room a line needs above / below its normal share of `line_h` for
/// items taller than text (inline math).
pub fn extents(items: &[Item], line_h: f64) -> (f64, f64) {
    let mut above: f64 = 0.0;
    let mut below: f64 = 0.0;
    for item in items {
        if let Item::Math(b, _) = item {
            above = above.max(b.height + 1.0 - LINE_ASCENT * line_h);
            below = below.max(b.depth + 1.0 - LINE_DESCENT * line_h);
        }
    }
    (above, below)
}

/// Draw one laid-out line with its baseline origin at (x, y).
pub fn emit_line(buf: &mut String, x: f64, y: f64, items: &[Item]) {
    let mut cx = x;
    let mut i = 0;
    while i < items.len() {
        match &items[i] {
            Item::Math(b, color) => {
                emit_math(buf, cx, y, b, *color);
                cx += b.width;
                i += 1;
            }
            Item::Cell(_) => {
                let mut cells = Vec::new();
                while let Some(Item::Cell(c)) = items.get(i) {
                    cells.push(*c);
                    i += 1;
                }
                emit_cells(buf, cx, y, &cells);
                cx += cells.iter().map(|c| c.width).sum::<f64>();
            }
        }
    }
    emit_decorations(buf, x, y, items);
}

/// Emit a run of cells as a single text object, merging adjacent cells that
/// share a font/size/colour/rise into one `Tj`.
fn emit_cells(buf: &mut String, x: f64, y: f64, cells: &[Cell]) {
    buf.push_str("BT\n");
    let _ = writeln!(buf, "1 0 0 1 {x:.2} {y:.2} Tm");

    let same = |a: &Cell, b: &Cell| a.size == b.size && a.color == b.color && a.rise == b.rise;
    let mut i = 0;
    while i < cells.len() {
        let c0 = cells[i];
        if c0.rise != 0.0 {
            let _ = writeln!(buf, "{:.2} Ts", c0.rise);
        }
        match c0.draw {
            Draw::Win { font, .. } => {
                let mut bytes: Vec<u8> = Vec::new();
                while i < cells.len() {
                    if let Draw::Win { byte, font: f } = cells[i].draw
                        && f == font
                        && same(&cells[i], &c0)
                    {
                        bytes.push(byte);
                        i += 1;
                    } else {
                        break;
                    }
                }
                set_font_color(buf, &format!("F{}", font.index()), c0.size, c0.color);
                buf.push('(');
                escape_into(buf, &bytes);
                buf.push_str(") Tj\n");
            }
            Draw::Uni { .. } => {
                let mut hex = String::new();
                while i < cells.len() {
                    if let Draw::Uni { gid } = cells[i].draw
                        && same(&cells[i], &c0)
                    {
                        let _ = write!(hex, "{gid:04X}");
                        i += 1;
                    } else {
                        break;
                    }
                }
                set_font_color(buf, FALLBACK_NAME, c0.size, c0.color);
                let _ = writeln!(buf, "<{hex}> Tj");
            }
        }
        if c0.rise != 0.0 {
            buf.push_str("0 Ts\n");
        }
    }
    buf.push_str("ET\n");
}

/// Strike-through and underline: one stroked line per contiguous run.
fn emit_decorations(buf: &mut String, x: f64, y: f64, items: &[Item]) {
    for (bit, offset) in [(DECO_STRIKE, 0.28), (DECO_UNDERLINE, -0.12)] {
        let mut cx = x;
        let mut run: Option<(f64, f64, Cell)> = None; // (x start, x end, style)
        let flush = |buf: &mut String, run: &mut Option<(f64, f64, Cell)>| {
            if let Some((x1, x2, c)) = run.take() {
                let ly = y + c.rise + offset * c.size;
                let t = (0.06 * c.size).max(0.4);
                let [r, g, b] = c.color.map(|v| v as f64 / 255.0);
                let _ = writeln!(buf, "{r:.3} {g:.3} {b:.3} RG {t:.2} w {x1:.2} {ly:.2} m {x2:.2} {ly:.2} l S");
            }
        };
        for item in items {
            let w = item.width();
            match item {
                Item::Cell(c) if c.deco & bit != 0 => match &mut run {
                    Some((_, end, rc)) if rc.size == c.size && rc.color == c.color && rc.rise == c.rise => {
                        *end = cx + w;
                    }
                    _ => {
                        flush(buf, &mut run);
                        run = Some((cx, cx + w, *c));
                    }
                },
                _ => flush(buf, &mut run),
            }
            cx += w;
        }
        flush(buf, &mut run);
    }
}

/// Draw a typeset formula with its baseline origin at (x, y).
pub fn emit_math(buf: &mut String, x: f64, y: f64, b: &MathBox, color: Rgb) {
    let [r, g, bl] = color.map(|v| v as f64 / 255.0);
    let mut in_text = false;
    let mut cur_size = -1.0;
    for op in &b.ops {
        match *op {
            MathOp::Glyph { x: gx, y: gy, gid, size } => {
                if !in_text {
                    let _ = writeln!(buf, "BT\n{r:.3} {g:.3} {bl:.3} rg");
                    in_text = true;
                    cur_size = -1.0;
                }
                if size != cur_size {
                    let _ = writeln!(buf, "/{MATH_NAME} {size:.3} Tf");
                    cur_size = size;
                }
                let _ = writeln!(buf, "1 0 0 1 {:.3} {:.3} Tm <{gid:04X}> Tj", x + gx, y + gy);
            }
            MathOp::Rule { x: rx, y: ry, w, h } => {
                if in_text {
                    buf.push_str("ET\n");
                    in_text = false;
                }
                let _ = writeln!(buf, "{r:.3} {g:.3} {bl:.3} rg {:.3} {:.3} {w:.3} {h:.3} re f", x + rx, y + ry);
            }
        }
    }
    if in_text {
        buf.push_str("ET\n");
    }
}

fn set_font_color(buf: &mut String, font_res: &str, size: f64, color: Rgb) {
    let [r, g, b] = color;
    let _ = writeln!(buf, "/{font_res} {size:.2} Tf");
    let _ = writeln!(
        buf,
        "{:.3} {:.3} {:.3} rg",
        r as f64 / 255.0,
        g as f64 / 255.0,
        b as f64 / 255.0
    );
}

fn escape_into(buf: &mut String, bytes: &[u8]) {
    for &b in bytes {
        match b {
            b'(' => buf.push_str("\\("),
            b')' => buf.push_str("\\)"),
            b'\\' => buf.push_str("\\\\"),
            0x20..=0x7e => buf.push(b as char),
            _ => {
                let _ = write!(buf, "\\{b:03o}");
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Block tree → laid-out PDF content
// ---------------------------------------------------------------------------

pub fn render_document(w: &mut Writer, doc: &Doc, fonts: &mut Fonts) {
    let ctx = Ctx {
        x: MARGIN,
        color: None,
        scale: 1.0,
    };
    render_blocks(w, &doc.blocks, ctx, fonts);
    if !doc.notes.is_empty() {
        render_notes(w, &doc.notes, fonts);
    }
}

/// Footnotes at the end of the document: a short rule, then each note as a
/// numbered item in a smaller size.
fn render_notes(w: &mut Writer, notes: &[(usize, Vec<Block>)], fonts: &mut Fonts) {
    const SCALE: f64 = 0.85;
    w.gap(BODY_SIZE * 1.2);
    w.settle();
    let line_h = BODY_SIZE * 0.5;
    w.y -= line_h;
    if w.y < MARGIN {
        w.new_page();
        w.y = PAGE_H - MARGIN - line_h;
    }
    let y = w.y + line_h * 0.5;
    w.hline(MARGIN, MARGIN + (PAGE_W - 2.0 * MARGIN) * 0.3, y, 0.6, 0.6);
    let size = BODY_SIZE * SCALE;
    for (i, (n, blocks)) in notes.iter().enumerate() {
        if i > 0 {
            w.gap(BODY_SIZE * 0.2);
        }
        let mut marker = Vec::new();
        text_to_cells(
            &mut marker,
            &format!("{n}."),
            CellStyle::plain(FontId::Helv, size, BLACK),
            &mut fonts.fallback,
        );
        w.pending = Some((MARGIN, marker));
        let ctx = Ctx {
            x: MARGIN + LIST_INDENT * SCALE,
            color: None,
            scale: SCALE,
        };
        render_blocks(w, blocks, ctx, fonts);
        w.pending = None;
    }
}

pub fn render_blocks(w: &mut Writer, blocks: &[Block], ctx: Ctx, fonts: &mut Fonts) {
    for (i, block) in blocks.iter().enumerate() {
        if i > 0 {
            w.gap(gap_before(block));
        }
        match block {
            Block::Heading { level, spans } => {
                // Keep a heading with the start of what follows it.
                let size = heading_size(*level) * ctx.scale;
                let need = size * 1.35 + w.carry + keep_with_next(blocks.get(i + 1), ctx.scale);
                if w.y - need < MARGIN && w.y < PAGE_H - MARGIN {
                    w.new_page();
                }
                let bolded: Vec<Span> = spans
                    .iter()
                    .map(|s| Span {
                        style: SpanStyle { bold: true, ..s.style },
                        ..s.clone()
                    })
                    .collect();
                render_paragraph(w, &bolded, size, ctx, fonts);
            }
            Block::Paragraph(spans) => render_paragraph(w, spans, BODY_SIZE * ctx.scale, ctx, fonts),
            Block::Code(text) => render_code(w, text, ctx.x + 6.0, ctx.scale, &mut fonts.fallback),
            Block::List { start, items } => render_list(w, *start, items, ctx, fonts),
            Block::Quote(inner) => {
                let quote_color = ctx.color.unwrap_or(QUOTE_COLOR);
                let inner_ctx = Ctx {
                    x: ctx.x + QUOTE_INDENT,
                    color: Some(quote_color),
                    ..ctx
                };
                render_blocks(w, inner, inner_ctx, fonts);
            }
            Block::Rule => render_rule(w, ctx.x),
            Block::Table { aligns, head, rows } => table::render_table(w, aligns, head, rows, ctx, fonts),
            Block::Math(tex) => render_display_math(w, tex, ctx, fonts),
        }
    }
}

/// Rough height of the part of `next` that must share a page with the heading
/// before it: a whole short table, else about two lines.
fn keep_with_next(next: Option<&Block>, scale: f64) -> f64 {
    let gap = next.map(gap_before).unwrap_or(0.0);
    gap + match next {
        Some(Block::Table { rows, .. }) => table::keep_height(rows.len(), scale),
        Some(Block::Math(_)) => BODY_SIZE * scale * 3.0,
        Some(_) => BODY_SIZE * scale * 1.35 * 2.0,
        None => 0.0,
    }
}

fn gap_before(block: &Block) -> f64 {
    match block {
        Block::Heading { .. } => BODY_SIZE * 0.8,
        Block::Rule => BODY_SIZE * 0.6,
        Block::Paragraph(_) => BODY_SIZE * 0.5,
        Block::Code(_) | Block::Quote(_) => BODY_SIZE * 0.45,
        Block::List { .. } => BODY_SIZE * 0.3,
        Block::Table { .. } => BODY_SIZE * 0.7,
        Block::Math(_) => BODY_SIZE * 0.6,
    }
}

fn heading_size(level: u8) -> f64 {
    match level {
        1 => 22.0,
        2 => 18.0,
        3 => 15.0,
        4 => 13.0,
        5 => 12.0,
        _ => 11.0,
    }
}

/// Typeset inline math, leaving the font's used-glyph set untouched on failure
/// (so a document whose formulas all fail doesn't embed the font).
fn typeset(tex: &str, size: f64, display: bool, font: &mut EmbeddedFont) -> Option<MathBox> {
    let saved = font.used.clone();
    match math::typeset(tex, size, display, font) {
        Ok(b) => Some(b),
        Err(_) => {
            font.used = saved;
            None
        }
    }
}

/// Lay out spans as line items at `size`.
pub fn spans_to_items(spans: &[Span], size: f64, color: Option<Rgb>, fonts: &mut Fonts) -> Vec<Item> {
    let mut items = Vec::new();
    for span in spans {
        match &span.kind {
            SpanKind::Math => match typeset(&span.text, size * MATH_SCALE, false, &mut fonts.math) {
                Some(b) => items.push(Item::Math(Rc::new(b), color.unwrap_or(BLACK))),
                None => {
                    let style = CellStyle::plain(FontId::Courier, size, color.unwrap_or(INLINE_CODE_COLOR));
                    text_to_cells(&mut items, &format!("${}$", span.text), style, &mut fonts.fallback);
                }
            },
            SpanKind::Note(_) => {
                let style = CellStyle {
                    font: font_for(span.style),
                    size: size * 0.7,
                    color: color.unwrap_or(BLACK),
                    rise: size * 0.38,
                    deco: 0,
                };
                text_to_cells(&mut items, &span.text, style, &mut fonts.fallback);
            }
            SpanKind::Text => {
                let st = span.style;
                let default_color = if st.code {
                    INLINE_CODE_COLOR
                } else if st.link {
                    LINK_COLOR
                } else {
                    BLACK
                };
                let mut deco = 0;
                if st.strike {
                    deco |= DECO_STRIKE;
                }
                if st.link {
                    deco |= DECO_UNDERLINE;
                }
                let style = CellStyle {
                    font: font_for(st),
                    size,
                    color: color.unwrap_or(default_color),
                    rise: 0.0,
                    deco,
                };
                text_to_cells(&mut items, &span.text, style, &mut fonts.fallback);
            }
        }
    }
    items
}

fn render_paragraph(w: &mut Writer, spans: &[Span], size: f64, ctx: Ctx, fonts: &mut Fonts) {
    let avail = (PAGE_W - MARGIN) - ctx.x;
    let line_h = size * 1.35;
    let items = spans_to_items(spans, size, ctx.color, fonts);
    for line in wrap(&items, avail) {
        w.place(ctx.x, line_h, &line);
    }
}

fn render_display_math(w: &mut Writer, tex: &str, ctx: Ctx, fonts: &mut Fonts) {
    let avail = (PAGE_W - MARGIN) - ctx.x;
    let size = BODY_SIZE * ctx.scale * MATH_SCALE;
    let mut b = typeset(tex, size, true, &mut fonts.math);
    // Too wide: set it smaller (down to 70%) rather than wrapping math.
    if let Some(wide) = &b
        && wide.width > avail
    {
        let s = (avail / wide.width).max(0.7);
        b = typeset(tex, size * s, true, &mut fonts.math);
    }
    match b {
        Some(b) => {
            let x = ctx.x + ((avail - b.width) / 2.0).max(0.0);
            let pad = BODY_SIZE * 0.25;
            let y = w.place_box(b.height + pad, b.depth + pad);
            emit_math(&mut w.cur, x, y, &b, ctx.color.unwrap_or(BLACK));
        }
        None => {
            // Unsupported TeX: show the source, as before math support.
            let span = Span {
                text: format!("$${tex}$$"),
                style: SpanStyle {
                    code: true,
                    ..SpanStyle::default()
                },
                kind: SpanKind::Text,
            };
            render_paragraph(w, &[span], BODY_SIZE * ctx.scale, ctx, fonts);
        }
    }
}

fn render_code(w: &mut Writer, text: &str, x: f64, scale: f64, fb: &mut EmbeddedFont) {
    let size = CODE_SIZE * scale;
    let line_h = size * 1.32;
    for raw in text.split('\n') {
        let mut items = Vec::new();
        text_to_cells(&mut items, raw, CellStyle::plain(FontId::Courier, size, CODE_BLOCK_COLOR), fb);
        w.place(x, line_h, &items);
    }
}

fn render_list(w: &mut Writer, start: Option<u64>, items: &[ListItem], ctx: Ctx, fonts: &mut Fonts) {
    let marker_color = ctx.color.unwrap_or(BLACK);
    let size = BODY_SIZE * ctx.scale;
    for (i, item) in items.iter().enumerate() {
        if i > 0 {
            w.gap(BODY_SIZE * 0.2);
        }
        let marker = match (item.task, start) {
            (Some(true), _) => "\u{2611}".to_string(),
            (Some(false), _) => "\u{2610}".to_string(),
            (None, Some(n)) => format!("{}.", n + i as u64),
            (None, None) => "\u{2022}".to_string(),
        };
        let mut mcells = Vec::new();
        text_to_cells(
            &mut mcells,
            &marker,
            CellStyle::plain(FontId::Helv, size, marker_color),
            &mut fonts.fallback,
        );
        w.pending = Some((ctx.x, mcells));
        let inner = Ctx {
            x: ctx.x + LIST_INDENT * ctx.scale,
            ..ctx
        };
        render_blocks(w, &item.blocks, inner, fonts);
        w.pending = None; // discard if the item drew nothing
    }
}

fn render_rule(w: &mut Writer, x: f64) {
    let line_h = BODY_SIZE * 0.5;
    w.settle();
    w.y -= line_h;
    if w.y < MARGIN {
        w.new_page();
        w.y = PAGE_H - MARGIN - line_h;
    }
    let y = w.y + line_h * 0.5;
    let x2 = PAGE_W - MARGIN;
    let _ = writeln!(
        &mut w.cur,
        "0.7 0.7 0.7 RG 0.6 w {x:.2} {y:.2} m {x2:.2} {y:.2} l S"
    );
}

/// Convert text to cells, routing each character to base-14 (WinAnsi) or the
/// embedded fallback font.
pub fn text_to_cells(out: &mut Vec<Item>, text: &str, st: CellStyle, fb: &mut EmbeddedFont) {
    let CellStyle {
        font: win_font,
        size,
        color,
        rise,
        deco,
    } = st;
    let cell = |is_space: bool, width: f64, draw: Draw| {
        Item::Cell(Cell {
            is_space,
            width,
            size,
            color,
            draw,
            rise,
            deco,
        })
    };
    let space_w = win_font.glyph_width(b' ') as f64 * size / 1000.0;
    let space = Draw::Win { byte: b' ', font: win_font };

    for ch in text.chars() {
        match ch {
            '\t' => {
                for _ in 0..4 {
                    out.push(cell(true, space_w, space));
                }
            }
            '\n' | '\r' => out.push(cell(true, space_w, space)),
            c => {
                if let Some(byte) = winansi_byte(c) {
                    let w = win_font.glyph_width(byte) as f64 * size / 1000.0;
                    out.push(cell(byte == b' ', w, Draw::Win { byte, font: win_font }));
                } else if let Some((gid, adv_units)) = fb.route(c) {
                    out.push(cell(false, adv_units * size / fb.upem, Draw::Uni { gid }));
                } else {
                    let w = win_font.glyph_width(b'?') as f64 * size / 1000.0;
                    out.push(cell(false, w, Draw::Win { byte: b'?', font: win_font }));
                }
            }
        }
    }
}

/// Greedy word wrap. Returns lines (with trailing spaces trimmed). A word wider
/// than a whole line is broken between characters; a formula never is.
pub fn wrap(items: &[Item], avail: f64) -> Vec<Vec<Item>> {
    let mut lines: Vec<Vec<Item>> = Vec::new();
    let mut line: Vec<Item> = Vec::new();
    let mut line_w = 0.0;
    let mut i = 0;
    while i < items.len() {
        let start = i;
        let is_space = items[i].is_space();
        while i < items.len() && items[i].is_space() == is_space {
            i += 1;
        }
        let chunk = &items[start..i];
        let chunk_w: f64 = chunk.iter().map(|c| c.width()).sum();

        if is_space {
            if line.is_empty() {
                continue; // drop leading whitespace
            }
            line.extend_from_slice(chunk);
            line_w += chunk_w;
        } else {
            if !line.is_empty() && line_w + chunk_w > avail {
                trim_trailing_spaces(&mut line);
                lines.push(std::mem::take(&mut line));
                line_w = 0.0;
            }
            let has_math = chunk.iter().any(|i| matches!(i, Item::Math(..)));
            if chunk_w > avail && chunk.len() > 1 && !has_math {
                // Hard-break an over-long word. (A word containing a formula
                // overflows whole, keeping punctuation attached to it.)
                for item in chunk {
                    let w = item.width();
                    if !line.is_empty() && line_w + w > avail {
                        lines.push(std::mem::take(&mut line));
                        line_w = 0.0;
                    }
                    line.push(item.clone());
                    line_w += w;
                }
            } else {
                line.extend_from_slice(chunk);
                line_w += chunk_w;
            }
        }
    }
    trim_trailing_spaces(&mut line);
    if !line.is_empty() {
        lines.push(line);
    }
    lines
}

fn trim_trailing_spaces(line: &mut Vec<Item>) {
    while line.last().is_some_and(|c| c.is_space()) {
        line.pop();
    }
}

/// Width of the widest unbreakable chunk, and of the whole run unwrapped.
pub fn min_max_width(items: &[Item]) -> (f64, f64) {
    let mut min: f64 = 0.0;
    let mut cur = 0.0;
    for item in items {
        if item.is_space() {
            cur = 0.0;
        } else {
            cur += item.width();
            min = min.max(cur);
        }
    }
    let mut end = items.len();
    while end > 0 && items[end - 1].is_space() {
        end -= 1;
    }
    let mut begin = 0;
    while begin < end && items[begin].is_space() {
        begin += 1;
    }
    let max = items[begin..end].iter().map(|i| i.width()).sum();
    (min, max)
}

#[cfg(test)]
mod tests {
    use super::*;

    const SIZE: f64 = 10.0;

    fn text(s: &str, fonts: &mut Fonts) -> Vec<Item> {
        let mut v = Vec::new();
        text_to_cells(&mut v, s, CellStyle::plain(FontId::Helv, SIZE, BLACK), &mut fonts.fallback);
        v
    }

    fn formula(width: f64, height: f64, depth: f64) -> Item {
        Item::Math(
            Rc::new(MathBox {
                width,
                height,
                depth,
                ops: Vec::new(),
            }),
            BLACK,
        )
    }

    /// The text of a line, with `#` for a formula.
    fn line_text(line: &[Item]) -> String {
        line.iter()
            .map(|i| match i {
                Item::Cell(Cell {
                    draw: Draw::Win { byte, .. },
                    ..
                }) => *byte as char,
                Item::Cell(_) => '?',
                Item::Math(..) => '#',
            })
            .collect()
    }

    fn width(items: &[Item]) -> f64 {
        items.iter().map(|i| i.width()).sum()
    }

    #[test]
    fn wrap_breaks_between_words_and_trims_spaces() {
        let mut f = Fonts::new().unwrap();
        let items = text("aaa bbb   ccc  ", &mut f);
        let avail = width(&text("aaa bbb", &mut f)) + 1.0;
        let lines: Vec<String> = wrap(&items, avail).iter().map(|l| line_text(l)).collect();
        assert_eq!(lines, ["aaa bbb", "ccc"]);
    }

    #[test]
    fn over_long_words_break_between_characters() {
        let mut f = Fonts::new().unwrap();
        let items = text("abcdefghijklmnop", &mut f);
        let avail = 20.0;
        let lines = wrap(&items, avail);
        assert!(lines.len() > 1);
        assert!(lines.iter().all(|l| width(l) <= avail + 1e-9), "every piece fits");
        let joined: String = lines.iter().map(|l| line_text(l)).collect();
        assert_eq!(joined, "abcdefghijklmnop", "no characters lost");
    }

    #[test]
    fn formulas_are_never_split() {
        let mut f = Fonts::new().unwrap();
        let mut items = text("x ", &mut f);
        items.push(formula(100.0, 5.0, 1.0));
        items.extend(text(", y", &mut f));
        let lines: Vec<String> = wrap(&items, 50.0).iter().map(|l| line_text(l)).collect();
        // The formula overflows on its own line, keeping the comma glued to it.
        assert_eq!(lines, ["x", "#,", "y"]);
    }

    #[test]
    fn min_max_width_ignores_outer_spaces() {
        let mut f = Fonts::new().unwrap();
        let items = text("  aa bbbb  ", &mut f);
        let (min, max) = min_max_width(&items);
        assert!((min - width(&text("bbbb", &mut f))).abs() < 1e-9);
        assert!((max - width(&text("aa bbbb", &mut f))).abs() < 1e-9);
        assert_eq!(min_max_width(&[]), (0.0, 0.0));
    }

    #[test]
    fn plain_lines_advance_by_the_line_height() {
        let mut f = Fonts::new().unwrap();
        let mut w = Writer::new();
        let line_h = SIZE * 1.35;
        w.place(MARGIN, line_h, &text("x", &mut f));
        assert_eq!(w.y, PAGE_H - MARGIN - line_h);
        w.place(MARGIN, line_h, &text("x", &mut f));
        assert_eq!(w.y, PAGE_H - MARGIN - 2.0 * line_h);
    }

    #[test]
    fn tall_inline_math_grows_its_line() {
        let mut f = Fonts::new().unwrap();
        let line_h = SIZE * 1.35;
        let mut plain = Writer::new();
        plain.place(MARGIN, line_h, &text("x", &mut f));
        plain.place(MARGIN, line_h, &text("x", &mut f));

        let mut tall = Writer::new();
        let mut items = text("x ", &mut f);
        items.push(formula(10.0, 20.0, 10.0));
        tall.place(MARGIN, line_h, &items);
        let first = tall.y;
        tall.place(MARGIN, line_h, &text("x", &mut f));

        assert!(first < PAGE_H - MARGIN - line_h, "room above the tall formula");
        let (above, below) = extents(&items, line_h);
        assert!(above > 0.0 && below > 0.0);
        assert!((plain.y - tall.y - (above + below)).abs() < 1e-9, "and below it, before the next line");
    }

    #[test]
    fn lines_that_do_not_fit_start_a_new_page() {
        let mut f = Fonts::new().unwrap();
        let mut w = Writer::new();
        w.y = MARGIN + 5.0;
        w.place(MARGIN, SIZE * 1.35, &text("x", &mut f));
        assert_eq!(w.y, PAGE_H - MARGIN - SIZE * 1.35);
        assert_eq!(w.into_pages().len(), 2);
    }
}
