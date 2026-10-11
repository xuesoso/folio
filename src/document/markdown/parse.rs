//! Markdown → block tree. Parses CommonMark + GFM extensions (tables, task
//! lists, strikethrough, footnotes) and `$`/`$$` math with `pulldown-cmark`.

use std::collections::HashMap;

pub use pulldown_cmark::Alignment;
use pulldown_cmark::{Event, HeadingLevel, Options, Parser, Tag, TagEnd};

#[derive(Debug, Default, Clone, Copy, PartialEq)]
pub struct SpanStyle {
    pub bold: bool,
    pub italic: bool,
    pub code: bool,
    pub strike: bool,
    pub link: bool,
}

#[derive(Debug, Clone, PartialEq)]
pub enum SpanKind {
    Text,
    /// `text` is TeX source, to be typeset as inline math.
    Math,
    /// A footnote reference; `text` is its number. The label is kept so that
    /// references to undefined notes can be turned back into literal text.
    Note(String),
}

#[derive(Debug, Clone)]
pub struct Span {
    pub text: String,
    pub style: SpanStyle,
    pub kind: SpanKind,
}

impl Span {
    fn text(text: impl Into<String>, style: SpanStyle) -> Self {
        Span {
            text: text.into(),
            style,
            kind: SpanKind::Text,
        }
    }
}

#[derive(Debug)]
pub struct ListItem {
    /// `Some(done)` for a GFM task-list item.
    pub task: Option<bool>,
    pub blocks: Vec<Block>,
}

#[derive(Debug)]
pub enum Block {
    Heading { level: u8, spans: Vec<Span> },
    Paragraph(Vec<Span>),
    Code(String),
    List { start: Option<u64>, items: Vec<ListItem> },
    Quote(Vec<Block>),
    Rule,
    Table {
        aligns: Vec<Alignment>,
        head: Vec<Vec<Span>>,
        rows: Vec<Vec<Vec<Span>>>,
    },
    /// Display math (TeX source).
    Math(String),
}

/// A parsed document: the main flow plus its footnotes, numbered in order of
/// first reference (unreferenced notes follow, in definition order).
#[derive(Debug)]
pub struct Doc {
    pub blocks: Vec<Block>,
    pub notes: Vec<(usize, Vec<Block>)>,
}

pub fn parse(markdown: &str) -> Doc {
    let options = Options::ENABLE_STRIKETHROUGH
        | Options::ENABLE_TASKLISTS
        | Options::ENABLE_TABLES
        | Options::ENABLE_FOOTNOTES
        | Options::ENABLE_MATH;
    let mut p = P {
        events: Parser::new_ext(markdown, options),
        tasks: Vec::new(),
        refs: Vec::new(),
        defs: HashMap::new(),
        def_order: Vec::new(),
    };
    let mut blocks = p.parse_blocks();

    // Final note order: referenced notes by first reference, then any
    // unreferenced ones in definition order. Undefined references go back to
    // their literal source text.
    let order: Vec<String> = p
        .refs
        .iter()
        .filter(|l| p.defs.contains_key(*l))
        .chain(p.def_order.iter().filter(|l| !p.refs.contains(l)))
        .cloned()
        .collect();
    let numbers: HashMap<&str, usize> = order.iter().enumerate().map(|(i, l)| (l.as_str(), i + 1)).collect();
    let mut notes: Vec<(usize, Vec<Block>)> = order
        .iter()
        .filter_map(|l| p.defs.remove(l).map(|b| (numbers[l.as_str()], b)))
        .collect();
    number_notes(&mut blocks, &numbers);
    for (_, b) in notes.iter_mut() {
        number_notes(b, &numbers);
    }

    Doc { blocks, notes }
}

struct P<'a> {
    events: Parser<'a>,
    /// One slot per open list item; a task marker fills the innermost one.
    /// (The marker is the first event in its item, so a nested list can't
    /// claim it.)
    tasks: Vec<Option<bool>>,
    /// Footnote labels in order of first reference.
    refs: Vec<String>,
    defs: HashMap<String, Vec<Block>>,
    def_order: Vec<String>,
}

/// Inline state while collecting spans.
#[derive(Default)]
struct Inline {
    spans: Vec<Span>,
    styles: Vec<SpanStyle>,
}

impl Inline {
    fn current(&self) -> SpanStyle {
        self.styles.last().copied().unwrap_or_default()
    }

    fn push_style(&mut self, f: impl FnOnce(&mut SpanStyle)) {
        let mut s = self.current();
        f(&mut s);
        self.styles.push(s);
    }
}

impl P<'_> {
    /// Handle an inline event, returning `true` if it was consumed.
    fn inline_event(&mut self, event: &Event, inl: &mut Inline) -> bool {
        match event {
            Event::Start(Tag::Emphasis) => inl.push_style(|s| s.italic = true),
            Event::Start(Tag::Strong) => inl.push_style(|s| s.bold = true),
            Event::Start(Tag::Strikethrough) => inl.push_style(|s| s.strike = true),
            Event::Start(Tag::Link { .. }) => inl.push_style(|s| s.link = true),
            Event::Start(Tag::Image { .. }) => inl.push_style(|_| {}),
            Event::End(TagEnd::Emphasis | TagEnd::Strong | TagEnd::Strikethrough | TagEnd::Link | TagEnd::Image) => {
                inl.styles.pop();
            }
            Event::Text(t) => {
                let s = inl.current();
                inl.spans.push(Span::text(t.to_string(), s));
            }
            Event::Code(t) => {
                let mut s = inl.current();
                s.code = true;
                inl.spans.push(Span::text(t.to_string(), s));
            }
            Event::InlineMath(t) => inl.spans.push(Span {
                text: t.to_string(),
                style: inl.current(),
                kind: SpanKind::Math,
            }),
            Event::FootnoteReference(label) => {
                let label = label.to_string();
                let n = match self.refs.iter().position(|l| *l == label) {
                    Some(i) => i + 1,
                    None => {
                        self.refs.push(label.clone());
                        self.refs.len()
                    }
                };
                inl.spans.push(Span {
                    text: n.to_string(),
                    style: inl.current(),
                    kind: SpanKind::Note(label),
                });
            }
            Event::SoftBreak | Event::HardBreak => {
                let s = inl.current();
                inl.spans.push(Span::text(" ", s));
            }
            Event::TaskListMarker(done) => {
                if let Some(slot) = self.tasks.last_mut() {
                    *slot = Some(*done);
                }
            }
            _ => return false,
        }
        true
    }

    /// Read events until the current container closes (an `End` for List/Item/
    /// BlockQuote/FootnoteDefinition) or the stream ends. Inline content is
    /// accumulated into spans and flushed into Paragraph/Heading blocks at the
    /// right boundaries; this also captures the un-wrapped inline content of
    /// tight list items.
    fn parse_blocks(&mut self) -> Vec<Block> {
        let mut blocks: Vec<Block> = Vec::new();
        let mut inl = Inline::default();
        let mut heading: Option<u8> = None;

        macro_rules! flush_paragraph {
            () => {
                if !inl.spans.is_empty() {
                    blocks.push(Block::Paragraph(std::mem::take(&mut inl.spans)));
                }
            };
        }

        while let Some(event) = self.events.next() {
            if self.inline_event(&event, &mut inl) {
                continue;
            }
            match event {
                Event::Start(tag) => match tag {
                    Tag::Paragraph => flush_paragraph!(),
                    Tag::Heading { level, .. } => {
                        flush_paragraph!();
                        heading = Some(heading_level(level));
                    }
                    Tag::CodeBlock(_) => {
                        flush_paragraph!();
                        blocks.push(self.parse_code_block());
                    }
                    Tag::List(start) => {
                        flush_paragraph!();
                        blocks.push(self.parse_list(start));
                    }
                    Tag::BlockQuote(_) => {
                        flush_paragraph!();
                        blocks.push(Block::Quote(self.parse_blocks()));
                    }
                    Tag::Table(aligns) => {
                        flush_paragraph!();
                        blocks.push(self.parse_table(aligns));
                    }
                    Tag::FootnoteDefinition(label) => {
                        flush_paragraph!();
                        let label = label.to_string();
                        let body = self.parse_blocks();
                        if !self.defs.contains_key(&label) {
                            self.def_order.push(label.clone());
                            self.defs.insert(label, body);
                        }
                    }
                    _ => inl.push_style(|_| {}),
                },
                Event::End(end) => match end {
                    TagEnd::Paragraph => flush_paragraph!(),
                    TagEnd::Heading(_) => {
                        let spans = std::mem::take(&mut inl.spans);
                        blocks.push(Block::Heading {
                            level: heading.take().unwrap_or(1),
                            spans,
                        });
                    }
                    TagEnd::List(_) | TagEnd::Item | TagEnd::BlockQuote(_) | TagEnd::FootnoteDefinition => {
                        flush_paragraph!();
                        return blocks;
                    }
                    _ => {
                        inl.styles.pop();
                    }
                },
                Event::DisplayMath(t) => {
                    if heading.is_some() {
                        // Keep headings on one line: typeset inline instead.
                        inl.spans.push(Span {
                            text: t.to_string(),
                            style: inl.current(),
                            kind: SpanKind::Math,
                        });
                    } else {
                        flush_paragraph!();
                        blocks.push(Block::Math(t.to_string()));
                    }
                }
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

    fn parse_code_block(&mut self) -> Block {
        let mut text = String::new();
        for event in self.events.by_ref() {
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

    fn parse_list(&mut self, start: Option<u64>) -> Block {
        let mut items = Vec::new();
        while let Some(event) = self.events.next() {
            match event {
                Event::Start(Tag::Item) => {
                    self.tasks.push(None);
                    let blocks = self.parse_blocks();
                    let task = self.tasks.pop().flatten();
                    items.push(ListItem { task, blocks });
                }
                Event::End(TagEnd::List(_)) => break,
                _ => {}
            }
        }
        Block::List { start, items }
    }

    fn parse_table(&mut self, aligns: Vec<Alignment>) -> Block {
        let mut head = Vec::new();
        let mut rows: Vec<Vec<Vec<Span>>> = Vec::new();
        let mut in_head = false;
        let mut row: Vec<Vec<Span>> = Vec::new();
        while let Some(event) = self.events.next() {
            match event {
                Event::Start(Tag::TableHead) => in_head = true,
                Event::End(TagEnd::TableHead) => {
                    head = std::mem::take(&mut row);
                    in_head = false;
                }
                Event::Start(Tag::TableRow) => row.clear(),
                Event::End(TagEnd::TableRow) => {
                    if !in_head {
                        rows.push(std::mem::take(&mut row));
                    }
                }
                Event::Start(Tag::TableCell) => row.push(self.parse_cell()),
                Event::End(TagEnd::Table) => break,
                _ => {}
            }
        }
        let cols = aligns.len().max(head.len());
        head.resize_with(cols, Vec::new);
        for r in rows.iter_mut() {
            r.resize_with(cols, Vec::new);
        }
        let mut aligns = aligns;
        aligns.resize(cols, Alignment::None);
        Block::Table { aligns, head, rows }
    }

    fn parse_cell(&mut self) -> Vec<Span> {
        let mut inl = Inline::default();
        while let Some(event) = self.events.next() {
            if self.inline_event(&event, &mut inl) {
                continue;
            }
            match event {
                Event::End(TagEnd::TableCell) => break,
                Event::DisplayMath(t) => inl.spans.push(Span {
                    text: t.to_string(),
                    style: inl.current(),
                    kind: SpanKind::Math,
                }),
                Event::Start(_) => inl.push_style(|_| {}),
                Event::End(_) => {
                    inl.styles.pop();
                }
                _ => {}
            }
        }
        inl.spans
    }
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

fn for_each_span(blocks: &mut [Block], f: &mut dyn FnMut(&mut Span)) {
    for b in blocks {
        match b {
            Block::Heading { spans, .. } | Block::Paragraph(spans) => spans.iter_mut().for_each(&mut *f),
            Block::List { items, .. } => {
                for item in items {
                    for_each_span(&mut item.blocks, f);
                }
            }
            Block::Quote(inner) => for_each_span(inner, f),
            Block::Table { head, rows, .. } => {
                for cell in head.iter_mut().chain(rows.iter_mut().flatten()) {
                    cell.iter_mut().for_each(&mut *f);
                }
            }
            Block::Code(_) | Block::Rule | Block::Math(_) => {}
        }
    }
}

/// Give each footnote reference its final number, or turn it back into
/// literal `[^label]` text if the note is undefined.
fn number_notes(blocks: &mut [Block], numbers: &HashMap<&str, usize>) {
    for_each_span(blocks, &mut |span| {
        if let SpanKind::Note(label) = &span.kind {
            match numbers.get(label.as_str()) {
                Some(n) => span.text = n.to_string(),
                None => {
                    span.text = format!("[^{label}]");
                    span.kind = SpanKind::Text;
                }
            }
        }
    });
}
