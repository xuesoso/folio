//! Markdown → PDF conversion — the first non-PDF input format.
//!
//! Parses CommonMark (plus GFM tables, task lists, strikethrough, footnotes, and
//! `$`/`$$` math) with `pulldown-cmark`, lowers it to a small block tree, then
//! lays it out into a PDF with `pdf-writer`. Latin / WinAnsi text uses the
//! standard-14 PDF fonts (Helvetica + Courier), which hayro renders from its
//! bundled Foxit substitutes — so normal text embeds no font data. Characters
//! WinAnsi can't represent (Greek, math operators, arrows, …) fall back to an
//! embedded **DejaVu Sans** font referenced as a Type0/Identity-H font, so
//! symbols render as real glyphs. Math is typeset by a small TeX-style engine
//! (`math`) using the embedded **Latin Modern Math** font. Each embedded font is
//! only included when a document actually uses it.
//!
//! Scope: headings, paragraphs, bold/italic/strikethrough, links (styled, not
//! clickable), inline code, code blocks, ordered/unordered/task lists (nested),
//! block quotes, thematic breaks, tables, footnotes (collected at the end), and
//! inline/display math. Images and raw HTML are out of scope.

mod fonts;
mod layout;
mod math;
mod parse;
mod pdf;
mod table;

use crate::Result;

// --- Page geometry (A4) and styling, in PDF points (1/72") -----------------
const PAGE_W: f64 = 595.0;
const PAGE_H: f64 = 842.0;
const MARGIN: f64 = 54.0;
const BODY_SIZE: f64 = 11.0;
const CODE_SIZE: f64 = 9.5;
const LIST_INDENT: f64 = 20.0;
const QUOTE_INDENT: f64 = 12.0;
/// Math is set slightly larger than the text: Latin Modern's x-height is
/// smaller than Helvetica's.
const MATH_SCALE: f64 = 1.1;

/// Convert Markdown source to PDF bytes. `title` (if any) becomes the PDF title.
pub fn markdown_to_pdf(markdown: &str, title: Option<&str>) -> Result<Vec<u8>> {
    let mut fonts = fonts::Fonts::new()?;
    let doc = parse::parse(markdown);
    let mut w = layout::Writer::new();
    layout::render_document(&mut w, &doc, &mut fonts);
    let pages = w.into_pages();
    Ok(pdf::assemble_pdf(&pages, title, &fonts))
}

#[cfg(test)]
mod tests {
    use super::fonts::winansi_byte;
    use super::parse::{Block, parse};
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
        let blocks = parse("# H\n\ntext\n\n- a\n- b\n").blocks;
        assert!(matches!(blocks[0], Block::Heading { level: 1, .. }));
        assert!(matches!(blocks[1], Block::Paragraph(_)));
        match &blocks[2] {
            Block::List { start, items } => {
                assert!(start.is_none());
                assert_eq!(items.len(), 2);
            }
            other => panic!("expected list, got {other:?}"),
        }
    }

    fn math_font_embedded(bytes: &[u8]) -> bool {
        contains(bytes, b"LatinModernMath")
    }

    #[test]
    fn parses_tables_with_alignment() {
        let doc = parse("| a | b | c |\n|:--|:-:|--:|\n| 1 | 2 | 3 |\n| 4 | 5 |\n");
        match &doc.blocks[0] {
            Block::Table { aligns, head, rows } => {
                use super::parse::Alignment::*;
                assert_eq!(aligns, &[Left, Center, Right]);
                assert_eq!(head.len(), 3);
                assert_eq!(rows.len(), 2);
                assert!(rows.iter().all(|r| r.len() == 3), "short rows are padded");
            }
            other => panic!("expected table, got {other:?}"),
        }
    }

    #[test]
    fn footnotes_number_by_first_reference() {
        let doc = parse("A[^b] and B[^a] and again[^b] and missing[^x].\n\n[^a]: Note A.\n[^b]: Note B.\n[^c]: Unreferenced.\n");
        let numbers: Vec<usize> = doc.notes.iter().map(|(n, _)| *n).collect();
        assert_eq!(numbers, [1, 2, 3]);
        let Block::Paragraph(spans) = &doc.blocks[0] else {
            panic!("expected paragraph")
        };
        let refs: Vec<&str> = spans
            .iter()
            .filter(|s| matches!(s.kind, super::parse::SpanKind::Note(_)))
            .map(|s| s.text.as_str())
            .collect();
        assert_eq!(refs, ["1", "2", "1"], "[^b] is 1, [^a] is 2");
        let text: String = spans.iter().map(|s| s.text.as_str()).collect();
        assert!(text.contains("missing[^x]."), "undefined refs stay literal: {text}");
        // The first note printed is [^b]'s.
        let Block::Paragraph(first) = &doc.notes[0].1[0] else {
            panic!("expected paragraph")
        };
        assert_eq!(first[0].text, "Note B.");
    }

    /// Footnote reference numbers in a block list, in order.
    fn note_refs(blocks: &[Block]) -> Vec<String> {
        let mut out = Vec::new();
        for b in blocks {
            if let Block::Paragraph(spans) = b {
                for s in spans {
                    if matches!(s.kind, super::parse::SpanKind::Note(_)) {
                        out.push(s.text.clone());
                    }
                }
            }
        }
        out
    }

    #[test]
    fn footnotes_defined_before_use_and_nested_references() {
        let doc = parse("[^n]: Defined first.\n\nText[^n].\n");
        assert_eq!(doc.notes.len(), 1);
        assert_eq!(note_refs(&doc.blocks), ["1"]);

        let doc = parse("A[^a].\n\n[^a]: See also[^b].\n[^b]: The second note.\n");
        let numbers: Vec<usize> = doc.notes.iter().map(|(n, _)| *n).collect();
        assert_eq!(numbers, [1, 2]);
        assert_eq!(note_refs(&doc.notes[0].1), ["2"], "a note can cite a later note");
    }

    #[test]
    fn decorations_and_superscripts_reach_the_pdf() {
        let line = |md: &str| markdown_to_pdf(md, None).expect("convert");
        // Stroked lines (` l S`) appear only for decorations here.
        assert!(!contains(&line("plain words"), b" l S"));
        assert!(contains(&line("~~gone~~"), b" l S"), "strikethrough draws a line");
        let link = line("[here](https://example.com)");
        assert!(contains(&link, b" l S"), "links are underlined");
        assert!(contains(&link, b"0.122 0.373 0.686 rg"), "links are blue");
        let note = line("Claim[^1].\n\n[^1]: Source.\n");
        assert!(contains(&note, b" Ts"), "footnote numbers are raised");
        assert!(contains(&note, b"0 Ts"), "and the rise is reset after");
    }

    #[test]
    fn task_markers_attach_to_their_own_items() {
        let doc = parse("- [x] outer\n  - [ ] inner\n  - plain\n- [ ] second\n");
        let Block::List { items, .. } = &doc.blocks[0] else {
            panic!("expected list")
        };
        assert_eq!(items[0].task, Some(true));
        assert_eq!(items[1].task, Some(false));
        let Some(Block::List { items: inner, .. }) = items[0].blocks.get(1) else {
            panic!("expected nested list")
        };
        assert_eq!(inner[0].task, Some(false));
        assert_eq!(inner[1].task, None);
    }

    #[test]
    fn dollar_amounts_are_not_math() {
        let doc = parse("It costs $5 and $10 today.");
        let Block::Paragraph(spans) = &doc.blocks[0] else {
            panic!("expected paragraph")
        };
        assert!(spans.iter().all(|s| s.kind == super::parse::SpanKind::Text));
    }

    #[test]
    fn display_math_becomes_its_own_block() {
        let doc = parse("Before\n$$x^2$$\nafter");
        assert!(matches!(doc.blocks[0], Block::Paragraph(_)));
        assert!(matches!(&doc.blocks[1], Block::Math(t) if t == "x^2"));
        assert!(matches!(doc.blocks[2], Block::Paragraph(_)));
    }

    #[test]
    fn math_embeds_the_math_font_and_opens() {
        let bytes = markdown_to_pdf("Inline $\\frac{a}{b}$ and\n\n$$\\sum_{i=1}^n i^2$$\n", None).expect("convert");
        assert!(math_font_embedded(&bytes), "math should embed Latin Modern Math");
        assert!(contains(&bytes, b"FontFile3"), "the OTF goes in /FontFile3");
        let doc = HayroDocument::from_bytes(bytes, "math.md").expect("hayro opens");
        assert!(doc.page_count() >= 1);
    }

    #[test]
    fn unsupported_math_falls_back_without_embedding() {
        let bytes = markdown_to_pdf("Bad $\\foo{x}$ and\n\n$$\\frac{a}{$$\n", None).expect("convert");
        assert!(!math_font_embedded(&bytes), "failed formulas must not embed the font");
        assert!(contains(&bytes, b"\\\\foo"), "the TeX source is shown instead");
        HayroDocument::from_bytes(bytes, "bad.md").expect("hayro opens");
    }

    #[test]
    fn long_tables_span_pages_and_wide_tables_fit() {
        let mut md = String::from("| n | square |\n|--:|--:|\n");
        for i in 0..120 {
            md.push_str(&format!("| {i} | {} |\n", i * i));
        }
        let bytes = markdown_to_pdf(&md, None).expect("convert");
        let doc = HayroDocument::from_bytes(bytes, "long.md").expect("open");
        assert!(doc.page_count() > 1, "120 rows should need more than one page");

        let header: Vec<String> = (0..15).map(|i| format!("column{i}")).collect();
        let md = format!(
            "| {} |\n|{}|\n| {} |\n",
            header.join(" | "),
            "---|".repeat(15),
            vec!["averyveryverylongwordindeed"; 15].join(" | ")
        );
        let bytes = markdown_to_pdf(&md, None).expect("convert wide");
        HayroDocument::from_bytes(bytes, "wide.md").expect("open wide");
    }

    #[test]
    fn showcase_fixture_converts() {
        let md = std::fs::read_to_string(
            std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/showcase.md"),
        )
        .expect("read fixture");
        let bytes = markdown_to_pdf(&md, Some("showcase.md")).expect("convert");
        assert!(math_font_embedded(&bytes));
        let doc = HayroDocument::from_bytes(bytes, "showcase.md").expect("open");
        assert!(doc.page_count() >= 2);
    }

    /// Dev-only: convert the fixtures and dump every page to PNGs for
    /// eyeballing. Run with `cargo test dump_markdown_render -- --ignored --nocapture`.
    #[test]
    #[ignore]
    fn dump_markdown_render() {
        use crate::document::RenderRequest;

        for name in ["sample", "showcase"] {
            let md = std::fs::read_to_string(
                std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join(format!("tests/fixtures/{name}.md")),
            )
            .expect("read fixture");
            let bytes = markdown_to_pdf(&md, Some(name)).expect("convert");
            std::fs::write(format!("/tmp/folio_{name}.pdf"), &bytes).expect("write pdf");

            let doc = HayroDocument::from_bytes(bytes, name).expect("open");
            for page in 0..doc.page_count() {
                let img = doc
                    .render_page(
                        page,
                        &RenderRequest {
                            target_width_px: 1200,
                            max_height_px: None,
                        },
                    )
                    .expect("render");
                let path = format!("/tmp/folio_{name}_{page}.png");
                let file = std::fs::File::create(&path).expect("create png");
                let w = std::io::BufWriter::new(file);
                let mut encoder = png::Encoder::new(w, img.width, img.height);
                encoder.set_color(png::ColorType::Rgba);
                encoder.set_depth(png::BitDepth::Eight);
                let mut writer = encoder.write_header().expect("png header");
                writer.write_image_data(&img.data).expect("png data");
                eprintln!("wrote {path} ({}x{})", img.width, img.height);
            }
        }
    }
}
