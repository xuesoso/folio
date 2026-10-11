//! A small TeX math typesetter: parses LaTeX math source and lays it out with
//! the glyphs and MATH-table parameters of the embedded Latin Modern Math font.
//!
//! The output is a box of positioned glyphs (by glyph id) and filled rules,
//! which the Markdown layout places on a line (inline math) or centres as its
//! own block (display math). Anything the parser doesn't understand is an
//! error, so the caller can fall back to showing the TeX source.

mod layout;
mod parse;
mod symbols;

use super::fonts::EmbeddedFont;

/// One drawing operation, positioned relative to the box origin (left end of
/// the baseline, y up), in points.
#[derive(Debug, Clone, Copy)]
pub enum MathOp {
    /// A glyph of the math font at `size` points, its origin at (x, y).
    Glyph { x: f64, y: f64, gid: u16, size: f64 },
    /// A filled rectangle with its lower-left corner at (x, y).
    Rule { x: f64, y: f64, w: f64, h: f64 },
}

/// A typeset formula.
#[derive(Debug, Clone)]
pub struct MathBox {
    pub width: f64,
    /// Extent above the baseline.
    pub height: f64,
    /// Extent below the baseline.
    pub depth: f64,
    pub ops: Vec<MathOp>,
}

/// Typeset `tex` at `size` points, in display style if `display`.
pub fn typeset(tex: &str, size: f64, display: bool, font: &mut EmbeddedFont) -> Result<MathBox, String> {
    let nodes = parse::parse(tex, display)?;
    layout::Layout::new(font)?.typeset(&nodes, size, display)
}

#[cfg(test)]
mod tests {
    use super::symbols::{self, Variant};
    use super::*;
    use crate::document::markdown::fonts::{EmbeddedFont, LM_MATH};
    use ttf_parser::GlyphId;

    fn font() -> EmbeddedFont {
        EmbeddedFont::new(LM_MATH).expect("math font")
    }

    #[test]
    fn fraction_is_taller_than_its_numerator() {
        let mut f = font();
        let x = typeset("x", 10.0, false, &mut f).unwrap();
        let frac = typeset(r"\frac{x}{y}", 10.0, false, &mut f).unwrap();
        assert!(frac.height > x.height);
        assert!(frac.depth > x.depth);
        assert!(frac.ops.iter().any(|op| matches!(op, MathOp::Rule { .. })));
    }

    #[test]
    fn superscript_is_raised() {
        let mut f = font();
        let b = typeset("x^2", 10.0, false, &mut f).unwrap();
        let ys: Vec<f64> = b
            .ops
            .iter()
            .filter_map(|op| match op {
                MathOp::Glyph { y, .. } => Some(*y),
                _ => None,
            })
            .collect();
        assert_eq!(ys.len(), 2);
        assert_eq!(ys[0], 0.0);
        assert!(ys[1] > 2.0, "superscript should sit above the baseline");
    }

    #[test]
    fn display_operators_are_larger() {
        let mut f = font();
        let t = typeset(r"\sum", 10.0, false, &mut f).unwrap();
        let d = typeset(r"\sum", 10.0, true, &mut f).unwrap();
        assert!(d.height + d.depth > (t.height + t.depth) * 1.2);
    }

    #[test]
    fn delimiters_grow_with_content() {
        let mut f = font();
        let small = typeset(r"\left( x \right)", 10.0, true, &mut f).unwrap();
        let big = typeset(r"\left( \frac{\frac{a}{b}}{\frac{c}{d}} \right)", 10.0, true, &mut f).unwrap();
        assert!(big.height + big.depth > 2.0 * (small.height + small.depth));
    }

    #[test]
    fn every_alphabet_typesets_every_latin_letter() {
        let mut f = font();
        let letters: String = ('A'..='Z').chain('a'..='z').collect();
        for cmd in [
            "mathbf", "boldsymbol", "mathit", "mathrm", "mathbb", "mathcal", "mathscr", "mathfrak", "mathsf",
            "mathtt",
        ] {
            let tex = format!(r"\{cmd}{{{letters}}}");
            typeset(&tex, 10.0, false, &mut f).unwrap_or_else(|e| panic!("{tex}: {e}"));
        }
    }

    /// (x, y, size) of each glyph, in drawing order.
    fn glyphs(b: &MathBox) -> Vec<(f64, f64, f64)> {
        b.ops
            .iter()
            .filter_map(|op| match *op {
                MathOp::Glyph { x, y, size, .. } => Some((x, y, size)),
                _ => None,
            })
            .collect()
    }

    fn gids(b: &MathBox) -> std::collections::BTreeSet<u16> {
        b.ops
            .iter()
            .filter_map(|op| match *op {
                MathOp::Glyph { gid, .. } => Some(gid),
                _ => None,
            })
            .collect()
    }

    fn width(tex: &str, display: bool) -> f64 {
        typeset(tex, 10.0, display, &mut font()).unwrap_or_else(|e| panic!("{tex}: {e}")).width
    }

    /// The highest script glyph (smaller than the base size) in `b`.
    fn sup_x(b: &MathBox) -> f64 {
        glyphs(b)
            .into_iter()
            .filter(|g| g.2 < 10.0)
            .max_by(|a, b| a.1.total_cmp(&b.1))
            .expect("a script glyph")
            .0
    }

    #[test]
    fn every_command_has_a_glyph() {
        let f = font();
        let has = |c: char| f.face.glyph_index(c).is_some();
        let stretchy = |c: char| {
            let gid = f.face.glyph_index(c).expect("glyph");
            f.face
                .tables()
                .math
                .and_then(|m| m.variants)
                .and_then(|v| v.vertical_constructions.get(GlyphId(gid.0)))
                .is_some()
        };
        let mut missing = Vec::new();
        for &(name, ch, _) in symbols::SYMBOLS {
            let greek = ('\u{0391}'..='\u{03F5}').contains(&ch);
            let variants: &[Variant] = if greek {
                &[Variant::Normal, Variant::Upright, Variant::Bold, Variant::BoldItalic]
            } else {
                &[Variant::Upright]
            };
            for &v in variants {
                let c = symbols::map_char(ch, v);
                if !has(c) {
                    missing.push(format!("\\{name} ({v:?}) {c:?}"));
                }
            }
        }
        for &(name, ch, _) in symbols::BIG_OPS {
            if !has(ch) || !stretchy(ch) {
                missing.push(format!("\\{name} (needs a display size)"));
            }
        }
        for &(name, ch, _) in symbols::ACCENTS {
            if !has(ch) {
                missing.push(format!("\\{name}"));
            }
        }
        for &(name, ch) in symbols::DELIMITERS {
            if !has(ch) {
                missing.push(format!("delimiter \\{name}"));
            }
        }
        for c in "()[]|/".chars().filter_map(symbols::delimiter_char) {
            if !has(c) || !stretchy(c) {
                missing.push(format!("delimiter {c} (needs to stretch)"));
            }
        }
        for &(name, text, _) in symbols::NAMED_OPS {
            if !text.chars().all(|c| c == ' ' || has(c)) {
                missing.push(format!("\\{name}"));
            }
        }
        for c in (' '..='~').filter(|c| !"\\{}^_&'~%".contains(*c)) {
            let (mapped, _) = symbols::char_class(c);
            if !has(symbols::map_char(mapped, Variant::Normal)) {
                missing.push(format!("source char {c:?}"));
            }
        }
        for v in [Variant::Bold, Variant::DoubleStruck, Variant::SansSerif, Variant::Monospace] {
            for d in '0'..='9' {
                if !has(symbols::map_char(d, v)) {
                    missing.push(format!("digit {d} ({v:?})"));
                }
            }
        }
        for c in ['\u{2032}', '\u{2033}', '\u{2034}', '\u{221A}'] {
            if !has(c) {
                missing.push(format!("{c:?}"));
            }
        }
        assert!(missing.is_empty(), "no glyph for: {missing:#?}");
    }

    #[test]
    fn negations_typeset() {
        let mut f = font();
        for rel in ["=", "<", ">", r"\in", r"\equiv", r"\subset", r"\supset", r"\subseteq", r"\supseteq", r"\sim", r"\approx", r"\le", r"\ge", r"\mid", r"\parallel"] {
            let tex = format!(r"a \not{rel} b");
            typeset(&tex, 10.0, false, &mut f).unwrap_or_else(|e| panic!("{tex}: {e}"));
        }
    }

    #[test]
    fn limits_go_above_in_display_and_beside_inline() {
        let mut f = font();
        let op_inline = width(r"\sum", false);
        let op_display = width(r"\sum", true);
        let display = typeset(r"\sum_a^b", 10.0, true, &mut f).unwrap();
        assert!(sup_x(&display) < op_display, "display limits sit over the operator");
        let inline = typeset(r"\sum_a^b", 10.0, false, &mut f).unwrap();
        assert!(sup_x(&inline) >= op_inline - 1e-6, "inline scripts sit after it");

        let forced = typeset(r"\sum\limits_a^b", 10.0, false, &mut f).unwrap();
        assert!(sup_x(&forced) < op_inline, "\\limits forces limits inline");
        let side = typeset(r"\sum\nolimits_a^b", 10.0, true, &mut f).unwrap();
        assert!(sup_x(&side) >= op_display - 1e-6, "\\nolimits forces scripts in display");
        let int = typeset(r"\int_a^b", 10.0, true, &mut f).unwrap();
        assert!(sup_x(&int) >= width(r"\int", true) - 1e-6, "integrals keep side scripts");
    }

    #[test]
    fn binary_operators_get_medium_spaces_but_unary_minus_none() {
        let gap = width("a+b", false) - (width("a", false) + width("+", false) + width("b", false));
        // Two medium spaces: 2 × 4mu = 8/18 em.
        assert!((gap - 8.0 / 18.0 * 10.0).abs() < 1e-6, "gap {gap}");
        let unary = width("-x", false) - (width("-", false) + width("x", false));
        assert!(unary.abs() < 1e-9, "a leading minus is unary: {unary}");
        // Scripts drop the spacing entirely.
        let script = width("x^{a+b}", false);
        assert!(script < width("x^{a\\,+\\,b}", false));
    }

    #[test]
    fn primes_are_full_size() {
        let b = typeset("f'", 10.0, false, &mut font()).unwrap();
        let g = glyphs(&b);
        assert_eq!(g.len(), 2);
        assert!(g.iter().all(|&(_, y, size)| size == 10.0 && y == 0.0));
    }

    #[test]
    fn middle_delimiters_match_the_outer_size() {
        let mut f = font();
        let frac = r"\frac{\frac{a}{b}}{\frac{c}{d}}";
        let outer = typeset(&format!(r"\left| {frac} \right."), 10.0, true, &mut f).unwrap();
        let middle = typeset(&format!(r"\left. {frac} \middle| \right."), 10.0, true, &mut f).unwrap();
        assert_eq!(gids(&outer), gids(&middle), "same glyphs, so the same bar size");
        let bar = f.face.glyph_index('|').unwrap().0;
        assert!(!gids(&middle).contains(&bar), "the bar is stretched, not the text-size one");
    }

    #[test]
    fn tall_delimiters_are_assembled_from_parts() {
        let mut f = font();
        let rows: Vec<String> = (1..=10).map(|i| i.to_string()).collect();
        let m = format!(r"\begin{{matrix}} {} \end{{matrix}}", rows.join(r" \\ "));
        let bare = typeset(&m, 10.0, true, &mut f).unwrap();
        let wrapped = typeset(&format!(r"\left( {m} \right)"), 10.0, true, &mut f).unwrap();
        let (h, wh) = (bare.height + bare.depth, wrapped.height + wrapped.depth);
        assert!(wh >= 0.9 * h, "delimiters cover the content: {wh} vs {h}");
        assert!(glyphs(&wrapped).len() >= glyphs(&bare).len() + 4, "each paren is several parts");
    }

    #[test]
    fn showcase_formulas_typeset() {
        let mut f = font();
        for tex in [
            r"E = mc^2",
            r"x = \frac{-b \pm \sqrt{b^2 - 4ac}}{2a}",
            r"e^{i\pi} + 1 = 0",
            r"\int_{-\infty}^{\infty} e^{-x^2}\,dx = \sqrt{\pi}, \qquad \sum_{n=1}^{\infty} \frac{1}{n^2} = \frac{\pi^2}{6}",
            r"A = \begin{pmatrix} a & b \\ c & d \end{pmatrix}, \qquad f(x) = \begin{cases} x^2 & x \ge 0 \\ -x & x < 0 \end{cases}",
            r"\begin{aligned} \nabla \cdot \mathbf{E} &= \frac{\rho}{\varepsilon_0} \\ \nabla \times \mathbf{B} &= \mu_0 \mathbf{J} + \mu_0 \varepsilon_0 \frac{\partial \mathbf{E}}{\partial t} \end{aligned}",
            r"\sqrt[3]{x} + \hat{a} + \vec{v} + \overline{z} + \lim_{n \to \infty} a_n + \mathbb{R}^n",
        ] {
            let b = typeset(tex, 11.0, true, &mut f).unwrap_or_else(|e| panic!("{tex}: {e}"));
            assert!(b.width > 0.0 && !b.ops.is_empty(), "{tex}");
        }
    }
}
