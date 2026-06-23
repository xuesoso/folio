//! Markdown → PDF conversion — the first non-PDF input format.
//!
//! Parses CommonMark with `pulldown-cmark`, lowers it to a small block tree, then
//! lays it out into a PDF with `pdf-writer`. Latin / WinAnsi text uses the
//! standard-14 PDF fonts (Helvetica + Courier), which hayro renders from its
//! bundled Foxit substitutes — so normal text embeds no font data. Characters
//! WinAnsi can't represent (Greek, math operators, arrows, …) fall back to an
//! embedded **DejaVu Sans** subset font referenced as a Type0/Identity-H font, so
//! symbols render as real glyphs. Only that one fallback font is ever embedded,
//! and only when a document actually uses such a character.
//!
//! Scope (v1): headings, paragraphs, bold/italic, inline code, code blocks,
//! ordered/unordered lists (nested), block quotes, and thematic breaks. Tables,
//! images, and LaTeX math typesetting are out of scope (LaTeX shows as text).

use std::collections::BTreeMap;
use std::fmt::Write as _;

use pdf_writer::types::{CidFontType, FontFlags, SystemInfo};
use pdf_writer::{Name, Pdf, Rect, Ref, Str};
use pulldown_cmark::{Event, HeadingLevel, Options, Parser, Tag, TagEnd};
use ttf_parser::Face;

use crate::Result;

/// Unicode fallback font, embedded only when needed (SIL/Bitstream — see
/// assets/fonts/DejaVuSans-LICENSE).
const DEJAVU: &[u8] = include_bytes!("../../assets/fonts/DejaVuSans.ttf");

// --- Page geometry (A4) and styling, in PDF points (1/72") -----------------
const PAGE_W: f64 = 595.0;
const PAGE_H: f64 = 842.0;
const MARGIN: f64 = 54.0;
const BODY_SIZE: f64 = 11.0;
const CODE_SIZE: f64 = 9.5;
const LIST_INDENT: f64 = 20.0;
const QUOTE_INDENT: f64 = 12.0;

type Rgb = [u8; 3];
const BLACK: Rgb = [0, 0, 0];
const INLINE_CODE_COLOR: Rgb = [0x80, 0x30, 0x30];
const CODE_BLOCK_COLOR: Rgb = [0x20, 0x20, 0x20];
const QUOTE_COLOR: Rgb = [0x55, 0x55, 0x55];

// Adobe/URW Helvetica advance widths (per 1000 em) for codes 32..=126. Oblique
// shares the upright widths; bold-oblique shares the bold widths. Courier is
// monospaced at 600. Used for line wrapping so layout matches hayro's rendering.
#[rustfmt::skip]
const HELV_W: [u16; 95] = [
    278,278,355,556,556,889,667,222,333,333,389,584,278,333,278,278,
    556,556,556,556,556,556,556,556,556,556,278,278,584,584,584,556,
    1015,667,667,722,722,667,611,778,722,278,500,667,556,833,722,778,
    667,778,722,667,611,722,667,944,667,667,611,278,278,278,469,556,
    222,556,556,500,556,556,278,556,556,222,222,500,222,833,556,556,
    556,556,333,500,278,556,500,722,500,500,500,334,260,334,584,
];
#[rustfmt::skip]
const HELVB_W: [u16; 95] = [
    278,333,474,556,556,889,722,278,333,333,389,584,278,333,278,278,
    556,556,556,556,556,556,556,556,556,556,333,333,584,584,584,611,
    975,722,722,722,722,667,611,778,722,278,556,722,611,833,722,778,
    667,778,722,667,611,722,667,944,667,667,611,333,278,333,584,556,
    278,556,611,556,611,556,333,611,611,278,278,556,278,889,611,611,
    611,611,389,556,333,611,556,778,556,556,500,389,280,389,584,
];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum FontId {
    Helv,
    HelvBold,
    HelvOblique,
    HelvBoldOblique,
    Courier,
}

impl FontId {
    /// Index into the page's base-14 font resources (`/F0`..`/F4`).
    fn index(self) -> u8 {
        self as u8
    }

    fn glyph_width(self, b: u8) -> u16 {
        match self {
            FontId::Courier => 600,
            _ => {
                if (32..=126).contains(&b) {
                    let table = if matches!(self, FontId::HelvBold | FontId::HelvBoldOblique) {
                        &HELVB_W
                    } else {
                        &HELV_W
                    };
                    table[(b - 32) as usize]
                } else if matches!(self, FontId::HelvBold | FontId::HelvBoldOblique) {
                    611
                } else {
                    556
                }
            }
        }
    }
}

const BASE_FONTS: [&[u8]; 5] = [
    b"Helvetica",
    b"Helvetica-Bold",
    b"Helvetica-Oblique",
    b"Helvetica-BoldOblique",
    b"Courier",
];
/// Resource name of the embedded Unicode fallback font.
const FALLBACK_NAME: &[u8] = b"FU";

fn font_for(style: SpanStyle) -> FontId {
    if style.code {
        FontId::Courier
    } else {
        match (style.bold, style.italic) {
            (true, true) => FontId::HelvBoldOblique,
            (true, false) => FontId::HelvBold,
            (false, true) => FontId::HelvOblique,
            (false, false) => FontId::Helv,
        }
    }
}

/// Convert Markdown source to PDF bytes. `title` (if any) becomes the PDF title.
pub fn markdown_to_pdf(markdown: &str, title: Option<&str>) -> Result<Vec<u8>> {
    let face = Face::parse(DEJAVU, 0).map_err(|e| anyhow::anyhow!("invalid fallback font: {e}"))?;
    let mut fb = Fallback::new(face);

    let blocks = parse(markdown);
    let mut w = Writer::new();
    render_blocks(&mut w, &blocks, MARGIN, None, &mut fb);
    let pages = w.into_pages();

    Ok(assemble_pdf(&pages, title, &fb))
}

// ---------------------------------------------------------------------------
// Block tree
// ---------------------------------------------------------------------------

#[derive(Debug, Default, Clone, Copy)]
struct SpanStyle {
    bold: bool,
    italic: bool,
    code: bool,
}

#[derive(Debug)]
struct Span {
    text: String,
    style: SpanStyle,
}

#[derive(Debug)]
enum Block {
    Heading { level: u8, spans: Vec<Span> },
    Paragraph(Vec<Span>),
    Code(String),
    List { ordered: bool, items: Vec<Vec<Block>> },
    Quote(Vec<Block>),
    Rule,
}

fn parse(markdown: &str) -> Vec<Block> {
    let options = Options::ENABLE_STRIKETHROUGH | Options::ENABLE_TASKLISTS;
    let mut parser = Parser::new_ext(markdown, options);
    parse_blocks(&mut parser)
}

/// Read events until the current container closes (an `End` for List/Item/
/// BlockQuote) or the stream ends. Inline content is accumulated into spans and
/// flushed into Paragraph/Heading blocks at the right boundaries; this also
/// captures the un-wrapped inline content of tight list items.
fn parse_blocks(parser: &mut Parser) -> Vec<Block> {
    let mut blocks: Vec<Block> = Vec::new();
    let mut inline: Vec<Span> = Vec::new();
    let mut styles: Vec<SpanStyle> = Vec::new();
    let mut heading: Option<u8> = None;

    let current = |styles: &[SpanStyle]| styles.last().copied().unwrap_or_default();

    macro_rules! flush_paragraph {
        () => {
            if !inline.is_empty() {
                blocks.push(Block::Paragraph(std::mem::take(&mut inline)));
            }
        };
    }

    while let Some(event) = parser.next() {
        match event {
            Event::Start(tag) => match tag {
                Tag::Paragraph => flush_paragraph!(),
                Tag::Heading { level, .. } => {
                    flush_paragraph!();
                    heading = Some(heading_level(level));
                }
                Tag::CodeBlock(_) => {
                    flush_paragraph!();
                    blocks.push(parse_code_block(parser));
                }
                Tag::List(start) => {
                    flush_paragraph!();
                    blocks.push(parse_list(parser, start.is_some()));
                }
                Tag::BlockQuote(_) => {
                    flush_paragraph!();
                    blocks.push(Block::Quote(parse_blocks(parser)));
                }
                Tag::Emphasis => {
                    let mut s = current(&styles);
                    s.italic = true;
                    styles.push(s);
                }
                Tag::Strong => {
                    let mut s = current(&styles);
                    s.bold = true;
                    styles.push(s);
                }
                _ => styles.push(current(&styles)),
            },
            Event::End(end) => match end {
                TagEnd::Paragraph => flush_paragraph!(),
                TagEnd::Heading(_) => {
                    let spans = std::mem::take(&mut inline);
                    blocks.push(Block::Heading {
                        level: heading.take().unwrap_or(1),
                        spans,
                    });
                }
                TagEnd::List(_) | TagEnd::Item | TagEnd::BlockQuote(_) => {
                    flush_paragraph!();
                    return blocks;
                }
                _ => {
                    styles.pop();
                }
            },
            Event::Text(t) => inline.push(Span {
                text: t.to_string(),
                style: current(&styles),
            }),
            Event::Code(t) => {
                let mut s = current(&styles);
                s.code = true;
                inline.push(Span {
                    text: t.to_string(),
                    style: s,
                });
            }
            Event::SoftBreak | Event::HardBreak => inline.push(Span {
                text: " ".to_string(),
                style: current(&styles),
            }),
            Event::TaskListMarker(done) => inline.push(Span {
                text: if done { "[x] " } else { "[ ] " }.to_string(),
                style: current(&styles),
            }),
            Event::Rule => {
                flush_paragraph!();
                blocks.push(Block::Rule);
            }
            _ => {}
        }
    }

    flush_paragraph!();
    blocks
}

fn parse_code_block(parser: &mut Parser) -> Block {
    let mut text = String::new();
    for event in parser.by_ref() {
        match event {
            Event::Text(t) => text.push_str(&t),
            Event::End(TagEnd::CodeBlock) => break,
            _ => {}
        }
    }
    if text.ends_with('\n') {
        text.pop();
    }
    Block::Code(text)
}

fn parse_list(parser: &mut Parser, ordered: bool) -> Block {
    let mut items: Vec<Vec<Block>> = Vec::new();
    while let Some(event) = parser.next() {
        match event {
            Event::Start(Tag::Item) => items.push(parse_blocks(parser)),
            Event::End(TagEnd::List(_)) => break,
            _ => {}
        }
    }
    Block::List { ordered, items }
}

fn heading_level(level: HeadingLevel) -> u8 {
    match level {
        HeadingLevel::H1 => 1,
        HeadingLevel::H2 => 2,
        HeadingLevel::H3 => 3,
        HeadingLevel::H4 => 4,
        HeadingLevel::H5 => 5,
        HeadingLevel::H6 => 6,
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

// ---------------------------------------------------------------------------
// Unicode fallback font
// ---------------------------------------------------------------------------

/// Tracks which fallback glyphs a document uses so only those get `/W` entries.
struct Fallback {
    face: Face<'static>,
    upem: f64,
    /// glyph id → horizontal advance (font units).
    used: BTreeMap<u16, u16>,
}

impl Fallback {
    fn new(face: Face<'static>) -> Self {
        let upem = face.units_per_em() as f64;
        Fallback {
            face,
            upem: if upem > 0.0 { upem } else { 1000.0 },
            used: BTreeMap::new(),
        }
    }

    /// Route a non-WinAnsi char to a fallback glyph, returning (gid, advance in
    /// font units) and recording it for the `/W` array. `None` if unsupported.
    fn route(&mut self, c: char) -> Option<(u16, f64)> {
        let gid = self.face.glyph_index(c)?;
        let adv = self.face.glyph_hor_advance(gid).unwrap_or(0);
        self.used.insert(gid.0, adv);
        Some((gid.0, adv as f64))
    }
}

/// Map a `char` to a WinAnsi byte if representable there (so it can use the
/// base-14 fonts), else `None` (route to the fallback font).
fn winansi_byte(c: char) -> Option<u8> {
    match c {
        '\u{00A0}' => Some(0x20),
        c if ('\u{20}'..='\u{7e}').contains(&c) => Some(c as u8),
        '\u{20AC}' => Some(0x80),
        '\u{201A}' => Some(0x82),
        '\u{0192}' => Some(0x83),
        '\u{201E}' => Some(0x84),
        '\u{2026}' => Some(0x85),
        '\u{2020}' => Some(0x86),
        '\u{2021}' => Some(0x87),
        '\u{02C6}' => Some(0x88),
        '\u{2030}' => Some(0x89),
        '\u{0160}' => Some(0x8A),
        '\u{2039}' => Some(0x8B),
        '\u{0152}' => Some(0x8C),
        '\u{017D}' => Some(0x8E),
        '\u{2018}' => Some(0x91),
        '\u{2019}' => Some(0x92),
        '\u{201C}' => Some(0x93),
        '\u{201D}' => Some(0x94),
        '\u{2022}' => Some(0x95),
        '\u{2013}' => Some(0x96),
        '\u{2014}' => Some(0x97),
        '\u{02DC}' => Some(0x98),
        '\u{2122}' => Some(0x99),
        '\u{0161}' => Some(0x9A),
        '\u{203A}' => Some(0x9B),
        '\u{0153}' => Some(0x9C),
        '\u{017E}' => Some(0x9E),
        '\u{0178}' => Some(0x9F),
        c if ('\u{A0}'..='\u{FF}').contains(&c) => Some(c as u8),
        _ => None,
    }
}

// ---------------------------------------------------------------------------
// PDF content writer (text layout + pagination)
// ---------------------------------------------------------------------------

/// How a single character is drawn.
#[derive(Clone, Copy)]
enum Draw {
    /// A WinAnsi byte in a base-14 font.
    Win { byte: u8, font: FontId },
    /// A glyph id in the embedded fallback font.
    Uni { gid: u16 },
}

/// One laid-out character (or space).
#[derive(Clone, Copy)]
struct Cell {
    is_space: bool,
    width: f64,
    size: f64,
    color: Rgb,
    draw: Draw,
}

/// Accumulates per-page content streams (all ASCII) while tracking the baseline.
struct Writer {
    pages: Vec<String>,
    cur: String,
    y: f64,
    /// A list marker (cells) to draw on the next line, at the given x.
    pending: Option<(f64, Vec<Cell>)>,
}

impl Writer {
    fn new() -> Self {
        Writer {
            pages: Vec::new(),
            cur: String::new(),
            y: PAGE_H - MARGIN,
            pending: None,
        }
    }

    fn new_page(&mut self) {
        self.pages.push(std::mem::take(&mut self.cur));
        self.y = PAGE_H - MARGIN;
    }

    fn into_pages(mut self) -> Vec<String> {
        if !self.cur.is_empty() || self.pages.is_empty() {
            self.pages.push(self.cur);
        }
        self.pages
    }

    fn gap(&mut self, amount: f64) {
        self.y -= amount;
    }

    /// Advance the baseline by `line_h` (breaking pages as needed) and draw the
    /// pending marker plus `cells`.
    fn place(&mut self, x: f64, line_h: f64, cells: &[Cell]) {
        self.y -= line_h;
        if self.y < MARGIN {
            self.new_page();
            self.y = PAGE_H - MARGIN - line_h;
        }
        let y = self.y;
        if let Some((mx, mcells)) = self.pending.take() {
            emit_cells(&mut self.cur, mx, y, &mcells);
        }
        if !cells.is_empty() {
            emit_cells(&mut self.cur, x, y, cells);
        }
    }
}

/// Emit one line's cells as a single text object, merging adjacent cells that
/// share a font/size/colour into one `Tj`.
fn emit_cells(buf: &mut String, x: f64, y: f64, cells: &[Cell]) {
    buf.push_str("BT\n");
    let _ = writeln!(buf, "1 0 0 1 {x:.2} {y:.2} Tm");

    let mut i = 0;
    while i < cells.len() {
        let c0 = cells[i];
        match c0.draw {
            Draw::Win { font, .. } => {
                let mut bytes: Vec<u8> = Vec::new();
                while i < cells.len() {
                    if let Draw::Win { byte, font: f } = cells[i].draw
                        && f == font
                        && cells[i].size == c0.size
                        && cells[i].color == c0.color
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
                        && cells[i].size == c0.size
                        && cells[i].color == c0.color
                    {
                        let _ = write!(hex, "{gid:04X}");
                        i += 1;
                    } else {
                        break;
                    }
                }
                set_font_color(buf, std::str::from_utf8(FALLBACK_NAME).unwrap(), c0.size, c0.color);
                let _ = writeln!(buf, "<{hex}> Tj");
            }
        }
    }
    buf.push_str("ET\n");
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

fn render_blocks(w: &mut Writer, blocks: &[Block], x: f64, color: Option<Rgb>, fb: &mut Fallback) {
    for (i, block) in blocks.iter().enumerate() {
        if i > 0 {
            w.gap(gap_before(block));
        }
        match block {
            Block::Heading { level, spans } => {
                let bolded: Vec<Span> = spans
                    .iter()
                    .map(|s| Span {
                        text: s.text.clone(),
                        style: SpanStyle {
                            bold: true,
                            italic: s.style.italic,
                            code: s.style.code,
                        },
                    })
                    .collect();
                render_paragraph(w, &bolded, heading_size(*level), x, color, fb);
            }
            Block::Paragraph(spans) => render_paragraph(w, spans, BODY_SIZE, x, color, fb),
            Block::Code(text) => render_code(w, text, x + 6.0, fb),
            Block::List { ordered, items } => render_list(w, *ordered, items, x, color, fb),
            Block::Quote(inner) => {
                let quote_color = color.unwrap_or(QUOTE_COLOR);
                render_blocks(w, inner, x + QUOTE_INDENT, Some(quote_color), fb);
            }
            Block::Rule => render_rule(w, x),
        }
    }
}

fn gap_before(block: &Block) -> f64 {
    match block {
        Block::Heading { .. } => BODY_SIZE * 0.8,
        Block::Rule => BODY_SIZE * 0.6,
        Block::Paragraph(_) => BODY_SIZE * 0.5,
        Block::Code(_) | Block::Quote(_) => BODY_SIZE * 0.45,
        Block::List { .. } => BODY_SIZE * 0.3,
    }
}

fn render_paragraph(w: &mut Writer, spans: &[Span], size: f64, x: f64, color: Option<Rgb>, fb: &mut Fallback) {
    let avail = (PAGE_W - MARGIN) - x;
    let line_h = size * 1.35;
    let mut cells: Vec<Cell> = Vec::new();
    for span in spans {
        let win_font = font_for(span.style);
        let col = color.unwrap_or(if span.style.code { INLINE_CODE_COLOR } else { BLACK });
        text_to_cells(&mut cells, &span.text, win_font, size, col, fb);
    }
    for line in wrap(&cells, avail) {
        w.place(x, line_h, &line);
    }
}

fn render_code(w: &mut Writer, text: &str, x: f64, fb: &mut Fallback) {
    let line_h = CODE_SIZE * 1.32;
    for raw in text.split('\n') {
        let mut cells = Vec::new();
        text_to_cells(&mut cells, raw, FontId::Courier, CODE_SIZE, CODE_BLOCK_COLOR, fb);
        w.place(x, line_h, &cells);
    }
}

fn render_list(w: &mut Writer, ordered: bool, items: &[Vec<Block>], x: f64, color: Option<Rgb>, fb: &mut Fallback) {
    let marker_color = color.unwrap_or(BLACK);
    for (i, item) in items.iter().enumerate() {
        if i > 0 {
            w.gap(BODY_SIZE * 0.2);
        }
        let marker = if ordered {
            format!("{}.", i + 1)
        } else {
            "\u{2022}".to_string()
        };
        let mut mcells = Vec::new();
        text_to_cells(&mut mcells, &marker, FontId::Helv, BODY_SIZE, marker_color, fb);
        w.pending = Some((x, mcells));
        render_blocks(w, item, x + LIST_INDENT, color, fb);
        w.pending = None; // discard if the item drew nothing
    }
}

fn render_rule(w: &mut Writer, x: f64) {
    let line_h = BODY_SIZE * 0.5;
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
fn text_to_cells(out: &mut Vec<Cell>, text: &str, win_font: FontId, size: f64, color: Rgb, fb: &mut Fallback) {
    let space_cell = |w: f64| Cell {
        is_space: true,
        width: w,
        size,
        color,
        draw: Draw::Win { byte: b' ', font: win_font },
    };
    let space_w = win_font.glyph_width(b' ') as f64 * size / 1000.0;

    for ch in text.chars() {
        match ch {
            '\t' => {
                for _ in 0..4 {
                    out.push(space_cell(space_w));
                }
            }
            '\n' | '\r' => out.push(space_cell(space_w)),
            c => {
                if let Some(byte) = winansi_byte(c) {
                    let w = win_font.glyph_width(byte) as f64 * size / 1000.0;
                    out.push(Cell {
                        is_space: byte == b' ',
                        width: w,
                        size,
                        color,
                        draw: Draw::Win { byte, font: win_font },
                    });
                } else if let Some((gid, adv_units)) = fb.route(c) {
                    let w = adv_units * size / fb.upem;
                    out.push(Cell {
                        is_space: false,
                        width: w,
                        size,
                        color,
                        draw: Draw::Uni { gid },
                    });
                } else {
                    let w = win_font.glyph_width(b'?') as f64 * size / 1000.0;
                    out.push(Cell {
                        is_space: false,
                        width: w,
                        size,
                        color,
                        draw: Draw::Win { byte: b'?', font: win_font },
                    });
                }
            }
        }
    }
}

/// Greedy word wrap. Returns lines (with trailing spaces trimmed).
fn wrap(cells: &[Cell], avail: f64) -> Vec<Vec<Cell>> {
    let mut lines: Vec<Vec<Cell>> = Vec::new();
    let mut line: Vec<Cell> = Vec::new();
    let mut line_w = 0.0;
    let mut i = 0;
    while i < cells.len() {
        let start = i;
        let is_space = cells[i].is_space;
        while i < cells.len() && cells[i].is_space == is_space {
            i += 1;
        }
        let chunk = &cells[start..i];
        let chunk_w: f64 = chunk.iter().map(|c| c.width).sum();

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
            line.extend_from_slice(chunk);
            line_w += chunk_w;
        }
    }
    trim_trailing_spaces(&mut line);
    if !line.is_empty() {
        lines.push(line);
    }
    lines
}

fn trim_trailing_spaces(line: &mut Vec<Cell>) {
    while line.last().is_some_and(|c| c.is_space) {
        line.pop();
    }
}

// ---------------------------------------------------------------------------
// PDF assembly
// ---------------------------------------------------------------------------

fn assemble_pdf(pages: &[String], title: Option<&str>, fb: &Fallback) -> Vec<u8> {
    let mut pdf = Pdf::new();
    let uses_fallback = !fb.used.is_empty();

    let catalog = Ref::new(1);
    let tree = Ref::new(2);
    let info = Ref::new(3);
    let font_refs: [Ref; 5] = std::array::from_fn(|i| Ref::new(4 + i as i32));
    let mut next = 9;
    let mut alloc = || {
        let r = Ref::new(next);
        next += 1;
        r
    };

    // Always reserve the fallback-font object numbers (unwritten numbers just
    // become free xref entries); they're only written/referenced when used.
    let type0 = alloc();
    let cid = alloc();
    let descriptor = alloc();
    let fontfile = alloc();

    let mut page_ids = Vec::with_capacity(pages.len());
    let mut content_ids = Vec::with_capacity(pages.len());
    for _ in pages {
        page_ids.push(alloc());
        content_ids.push(alloc());
    }

    pdf.catalog(catalog).pages(tree);
    pdf.pages(tree)
        .kids(page_ids.iter().copied())
        .count(page_ids.len() as i32);

    if let Some(t) = title {
        pdf.document_info(info).title(pdf_writer::TextStr(t));
    }

    for (i, base) in BASE_FONTS.iter().enumerate() {
        pdf.type1_font(font_refs[i])
            .base_font(Name(base))
            .encoding_predefined(Name(b"WinAnsiEncoding"));
    }

    if uses_fallback {
        write_fallback_font(&mut pdf, fb, type0, cid, descriptor, fontfile);
    }

    for (i, content) in pages.iter().enumerate() {
        {
            let mut page = pdf.page(page_ids[i]);
            page.parent(tree)
                .media_box(Rect::new(0.0, 0.0, PAGE_W as f32, PAGE_H as f32))
                .contents(content_ids[i]);
            let mut resources = page.resources();
            let mut fonts = resources.fonts();
            for (j, fref) in font_refs.iter().enumerate() {
                let name = format!("F{j}");
                fonts.pair(Name(name.as_bytes()), *fref);
            }
            if uses_fallback {
                fonts.pair(Name(FALLBACK_NAME), type0);
            }
        }
        pdf.stream(content_ids[i], content.as_bytes());
    }

    pdf.finish()
}

fn write_fallback_font(pdf: &mut Pdf, fb: &Fallback, type0: Ref, cid: Ref, descriptor: Ref, fontfile: Ref) {
    let scale = 1000.0 / fb.upem;
    let s = |v: i16| (v as f64 * scale) as f32;
    let face = &fb.face;

    pdf.type0_font(type0)
        .base_font(Name(b"DejaVuSans"))
        .encoding_predefined(Name(b"Identity-H"))
        .descendant_font(cid);

    {
        let mut cid_font = pdf.cid_font(cid);
        cid_font
            .subtype(CidFontType::Type2)
            .base_font(Name(b"DejaVuSans"))
            .system_info(SystemInfo {
                registry: Str(b"Adobe"),
                ordering: Str(b"Identity"),
                supplement: 0,
            })
            .font_descriptor(descriptor)
            .cid_to_gid_map_predefined(Name(b"Identity"));

        let mut widths = cid_font.widths();
        let mut entries = fb.used.iter().peekable();
        while let Some((&gid, &adv)) = entries.next() {
            let mut run = vec![(adv as f64 * scale) as f32];
            while let Some(&(&ng, &nadv)) = entries.peek() {
                if ng == gid + run.len() as u16 {
                    run.push((nadv as f64 * scale) as f32);
                    entries.next();
                } else {
                    break;
                }
            }
            widths.consecutive(gid, run);
        }
    }

    let bbox = face.global_bounding_box();
    pdf.font_descriptor(descriptor)
        .name(Name(b"DejaVuSans"))
        .flags(FontFlags::SYMBOLIC)
        .bbox(Rect::new(s(bbox.x_min), s(bbox.y_min), s(bbox.x_max), s(bbox.y_max)))
        .italic_angle(0.0)
        .ascent(s(face.ascender()))
        .descent(s(face.descender()))
        .cap_height(s(face.capital_height().unwrap_or_else(|| face.ascender())))
        .stem_v(80.0)
        .font_file2(fontfile);

    pdf.stream(fontfile, DEJAVU)
        .pair(Name(b"Length1"), DEJAVU.len() as i32);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::document::Document;
    use crate::document::hayro::HayroDocument;

    fn contains(haystack: &[u8], needle: &[u8]) -> bool {
        haystack.windows(needle.len()).any(|w| w == needle)
    }

    #[test]
    fn converts_markdown_to_openable_pdf() {
        let md = "# Title\n\nSome **bold**, *italic*, and `code`.\n\n- one\n- two\n  - nested\n\n1. first\n2. second\n\n```\nfn main() {}\n```\n\n> a quote\n\n---\n\nDone.\n";
        let bytes = markdown_to_pdf(md, Some("test")).expect("convert");
        assert!(bytes.starts_with(b"%PDF"), "output should be a PDF");
        // Pure-ASCII input embeds no fallback font.
        assert!(!contains(&bytes, b"FontFile2"), "no font should be embedded for ASCII");

        let doc = HayroDocument::from_bytes(bytes, "test.md").expect("hayro opens converted PDF");
        assert!(doc.page_count() >= 1);
        let info = doc.page_info(0).expect("page info");
        assert!(info.width_pt > 0.0 && info.height_pt > 0.0);
    }

    #[test]
    fn empty_markdown_still_produces_an_openable_pdf() {
        let bytes = markdown_to_pdf("", None).expect("convert empty");
        let doc = HayroDocument::from_bytes(bytes, "empty.md").expect("hayro opens");
        assert!(doc.page_count() >= 1);
    }

    #[test]
    fn wraps_long_paragraph_onto_multiple_pages() {
        let para = "word ".repeat(4000);
        let bytes = markdown_to_pdf(&para, None).expect("convert");
        let doc = HayroDocument::from_bytes(bytes, "long.md").expect("open");
        assert!(doc.page_count() > 1, "long input should span multiple pages");
    }

    #[test]
    fn symbols_embed_fallback_font_and_open() {
        // Greek + math + arrow are not WinAnsi and must route to the embedded font.
        let md = "Greek \u{03C0}, sum \u{2211}, root \u{221A}2, approx \u{2248}, arrow \u{2192}.";
        let bytes = markdown_to_pdf(md, None).expect("convert");
        assert!(contains(&bytes, b"FontFile2"), "symbols should embed a fallback font");
        assert!(contains(&bytes, b"Identity-H"), "fallback should be a Type0 Identity font");
        let doc = HayroDocument::from_bytes(bytes, "sym.md").expect("hayro opens");
        assert!(doc.page_count() >= 1);
    }

    #[test]
    fn winansi_byte_routing() {
        assert_eq!(winansi_byte('a'), Some(b'a'));
        assert_eq!(winansi_byte('\u{2014}'), Some(0x97)); // em dash is WinAnsi
        assert_eq!(winansi_byte('\u{00E9}'), Some(0xE9)); // é is Latin-1
        assert_eq!(winansi_byte('\u{2192}'), None); // arrow -> fallback
        assert_eq!(winansi_byte('\u{03C0}'), None); // pi -> fallback
    }

    #[test]
    fn parses_nested_structure() {
        let blocks = parse("# H\n\ntext\n\n- a\n- b\n");
        assert!(matches!(blocks[0], Block::Heading { level: 1, .. }));
        assert!(matches!(blocks[1], Block::Paragraph(_)));
        match &blocks[2] {
            Block::List { ordered, items } => {
                assert!(!ordered);
                assert_eq!(items.len(), 2);
            }
            other => panic!("expected list, got {other:?}"),
        }
    }

    /// Dev-only: convert the fixture and dump page 1 to a PNG for eyeballing.
    /// Run with `cargo test dump_markdown_render -- --ignored --nocapture`.
    #[test]
    #[ignore]
    fn dump_markdown_render() {
        use crate::document::RenderRequest;

        let md = std::fs::read_to_string(
            std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/sample.md"),
        )
        .expect("read fixture");
        let bytes = markdown_to_pdf(&md, Some("sample.md")).expect("convert");
        std::fs::write("/tmp/folio_sample.pdf", &bytes).expect("write pdf");

        let doc = HayroDocument::from_bytes(bytes, "sample.md").expect("open");
        let img = doc
            .render_page(
                0,
                &RenderRequest {
                    target_width_px: 1200,
                    max_height_px: None,
                },
            )
            .expect("render");

        let file = std::fs::File::create("/tmp/folio_sample.png").expect("create png");
        let w = std::io::BufWriter::new(file);
        let mut encoder = png::Encoder::new(w, img.width, img.height);
        encoder.set_color(png::ColorType::Rgba);
        encoder.set_depth(png::BitDepth::Eight);
        let mut writer = encoder.write_header().expect("png header");
        writer.write_image_data(&img.data).expect("png data");
        eprintln!(
            "wrote /tmp/folio_sample.pdf and /tmp/folio_sample.png ({}x{})",
            img.width, img.height
        );
    }
}
