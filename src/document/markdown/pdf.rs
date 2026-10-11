//! PDF assembly: pages, base-14 font resources, and the embedded fonts.

use pdf_writer::types::{CidFontType, FontFlags, SystemInfo};
use pdf_writer::{Name, Pdf, Rect, Ref, Str};

use super::fonts::{BASE_FONTS, DEJAVU, EmbeddedFont, FALLBACK_NAME, Fonts, LM_MATH, MATH_NAME};
use super::{PAGE_H, PAGE_W};

/// Object numbers reserved for one embedded Type0 font.
struct FontRefs {
    type0: Ref,
    cid: Ref,
    descriptor: Ref,
    file: Ref,
}

pub fn assemble_pdf(pages: &[String], title: Option<&str>, fonts: &Fonts) -> Vec<u8> {
    let mut pdf = Pdf::new();
    let uses_fallback = !fonts.fallback.used.is_empty();
    let uses_math = !fonts.math.used.is_empty();

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

    // Always reserve the embedded-font object numbers (unwritten numbers just
    // become free xref entries); they're only written/referenced when used.
    let mut reserve = || FontRefs {
        type0: alloc(),
        cid: alloc(),
        descriptor: alloc(),
        file: alloc(),
    };
    let fallback_refs = reserve();
    let math_refs = reserve();

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
        write_embedded_font(&mut pdf, &fonts.fallback, &fallback_refs, b"DejaVuSans", DEJAVU, false);
    }
    if uses_math {
        write_embedded_font(&mut pdf, &fonts.math, &math_refs, b"LatinModernMath-Regular", LM_MATH, true);
    }

    for (i, content) in pages.iter().enumerate() {
        {
            let mut page = pdf.page(page_ids[i]);
            page.parent(tree)
                .media_box(Rect::new(0.0, 0.0, PAGE_W as f32, PAGE_H as f32))
                .contents(content_ids[i]);
            let mut resources = page.resources();
            let mut fonts_dict = resources.fonts();
            for (j, fref) in font_refs.iter().enumerate() {
                let name = format!("F{j}");
                fonts_dict.pair(Name(name.as_bytes()), *fref);
            }
            if uses_fallback {
                fonts_dict.pair(Name(FALLBACK_NAME.as_bytes()), fallback_refs.type0);
            }
            if uses_math {
                fonts_dict.pair(Name(MATH_NAME.as_bytes()), math_refs.type0);
            }
        }
        pdf.stream(content_ids[i], content.as_bytes());
    }

    pdf.finish()
}

/// Write a Type0/Identity-H font whose CIDs are glyph ids. TrueType outlines
/// go in `/FontFile2`; an OpenType/CFF font (`cff`) goes in `/FontFile3` with
/// subtype `/OpenType`, embedded byte-for-byte as shipped.
fn write_embedded_font(pdf: &mut Pdf, font: &EmbeddedFont, refs: &FontRefs, name: &[u8], data: &[u8], cff: bool) {
    let scale = 1000.0 / font.upem;
    let s = |v: i16| (v as f64 * scale) as f32;
    let face = &font.face;

    pdf.type0_font(refs.type0)
        .base_font(Name(name))
        .encoding_predefined(Name(b"Identity-H"))
        .descendant_font(refs.cid);

    {
        let mut cid_font = pdf.cid_font(refs.cid);
        cid_font
            .subtype(if cff { CidFontType::Type0 } else { CidFontType::Type2 })
            .base_font(Name(name))
            .system_info(SystemInfo {
                registry: Str(b"Adobe"),
                ordering: Str(b"Identity"),
                supplement: 0,
            })
            .font_descriptor(refs.descriptor);
        if !cff {
            cid_font.cid_to_gid_map_predefined(Name(b"Identity"));
        }

        let mut widths = cid_font.widths();
        let mut entries = font.used.iter().peekable();
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
    let mut descriptor = pdf.font_descriptor(refs.descriptor);
    descriptor
        .name(Name(name))
        .flags(FontFlags::SYMBOLIC)
        .bbox(Rect::new(s(bbox.x_min), s(bbox.y_min), s(bbox.x_max), s(bbox.y_max)))
        .italic_angle(0.0)
        .ascent(s(face.ascender()))
        .descent(s(face.descender()))
        .cap_height(s(face.capital_height().unwrap_or_else(|| face.ascender())))
        .stem_v(80.0);
    if cff {
        descriptor.font_file3(refs.file);
    } else {
        descriptor.font_file2(refs.file);
    }
    drop(descriptor);

    let mut stream = pdf.stream(refs.file, data);
    if cff {
        stream.pair(Name(b"Subtype"), Name(b"OpenType"));
    } else {
        stream.pair(Name(b"Length1"), data.len() as i32);
    }
}
