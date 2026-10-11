//! Fonts used by the Markdown renderer: the base-14 PDF fonts (with their
//! metrics, for wrapping), the embedded DejaVu Sans Unicode fallback, and the
//! embedded Latin Modern Math font used by the math typesetter.

use std::collections::BTreeMap;

use ttf_parser::{Face, GlyphId};

use super::parse::SpanStyle;

/// Unicode fallback font, embedded only when needed (SIL/Bitstream — see
/// assets/fonts/DejaVuSans-LICENSE).
pub const DEJAVU: &[u8] = include_bytes!("../../../assets/fonts/DejaVuSans.ttf");

/// OpenType math font (CFF outlines + MATH table), embedded unmodified only when
/// a document contains math (GUST Font License — see
/// assets/fonts/LatinModernMath-LICENSE).
pub const LM_MATH: &[u8] = include_bytes!("../../../assets/fonts/LatinModernMath.otf");

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
pub enum FontId {
    Helv,
    HelvBold,
    HelvOblique,
    HelvBoldOblique,
    Courier,
}

impl FontId {
    /// Index into the page's base-14 font resources (`/F0`..`/F4`).
    pub fn index(self) -> u8 {
        self as u8
    }

    pub fn glyph_width(self, b: u8) -> u16 {
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

pub const BASE_FONTS: [&[u8]; 5] = [
    b"Helvetica",
    b"Helvetica-Bold",
    b"Helvetica-Oblique",
    b"Helvetica-BoldOblique",
    b"Courier",
];
/// Resource name of the embedded Unicode fallback font.
pub const FALLBACK_NAME: &str = "FU";
/// Resource name of the embedded math font.
pub const MATH_NAME: &str = "FM";

pub fn font_for(style: SpanStyle) -> FontId {
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

/// An embedded TrueType/OpenType font addressed by glyph id. Tracks which glyphs
/// a document uses so only those get `/W` entries.
pub struct EmbeddedFont {
    pub face: Face<'static>,
    pub upem: f64,
    /// glyph id → horizontal advance (font units).
    pub used: BTreeMap<u16, u16>,
}

impl EmbeddedFont {
    pub fn new(data: &'static [u8]) -> crate::Result<Self> {
        let face = Face::parse(data, 0).map_err(|e| anyhow::anyhow!("invalid embedded font: {e}"))?;
        let upem = face.units_per_em() as f64;
        Ok(EmbeddedFont {
            face,
            upem: if upem > 0.0 { upem } else { 1000.0 },
            used: BTreeMap::new(),
        })
    }

    /// Route a char to a glyph, returning (gid, advance in font units) and
    /// recording it for the `/W` array. `None` if unsupported.
    pub fn route(&mut self, c: char) -> Option<(u16, f64)> {
        let gid = self.face.glyph_index(c)?;
        Some((gid.0, self.mark(gid.0)))
    }

    /// Record a glyph id as used and return its advance in font units.
    pub fn mark(&mut self, gid: u16) -> f64 {
        let adv = self.face.glyph_hor_advance(GlyphId(gid)).unwrap_or(0);
        self.used.insert(gid, adv);
        adv as f64
    }
}

/// All fonts a document may embed.
pub struct Fonts {
    pub fallback: EmbeddedFont,
    pub math: EmbeddedFont,
}

impl Fonts {
    pub fn new() -> crate::Result<Self> {
        Ok(Fonts {
            fallback: EmbeddedFont::new(DEJAVU)?,
            math: EmbeddedFont::new(LM_MATH)?,
        })
    }
}

/// Map a `char` to a WinAnsi byte if representable there (so it can use the
/// base-14 fonts), else `None` (route to the fallback font).
pub fn winansi_byte(c: char) -> Option<u8> {
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
