//! `Node` tree → positioned glyphs and rules, following TeX's math layout rules
//! (TeXbook appendix G) with the parameters of an OpenType MATH table, as
//! specified by MathML Core.

use ttf_parser::GlyphId;
use ttf_parser::math::{Constants, GlyphAssembly, MathValue, Table};

use super::parse::{Align, ArrayKind, Node, Result, Style};
use super::symbols::Class;
use super::{MathBox, MathOp};
use crate::document::markdown::fonts::EmbeddedFont;

/// Math style plus TeX's "cramped" flag (which lowers superscripts).
#[derive(Debug, Clone, Copy)]
struct St {
    style: Style,
    cramped: bool,
}

impl St {
    fn script(self) -> Style {
        match self.style {
            Style::Display | Style::Text => Style::Script,
            _ => Style::ScriptScript,
        }
    }
    fn sup(self) -> St {
        St {
            style: self.script(),
            cramped: self.cramped,
        }
    }
    fn sub(self) -> St {
        St {
            style: self.script(),
            cramped: true,
        }
    }
    fn num(self) -> St {
        St {
            style: match self.style {
                Style::Display => Style::Text,
                Style::Text => Style::Script,
                _ => Style::ScriptScript,
            },
            cramped: self.cramped,
        }
    }
    fn den(self) -> St {
        St {
            cramped: true,
            ..self.num()
        }
    }
    fn display(self) -> bool {
        self.style == Style::Display
    }
}

/// A laid-out box. Coordinates are in points, y up, origin at the left end of
/// the baseline.
#[derive(Debug, Clone, Default)]
struct B {
    width: f64,
    height: f64,
    depth: f64,
    /// Italic correction of the last glyph (for script placement).
    italic: f64,
    /// Set when the box is exactly one glyph (accent attachment, TeX's
    /// single-character script rule).
    glyph: Option<u16>,
    ops: Vec<MathOp>,
}

impl B {
    fn space(width: f64) -> B {
        B { width, ..B::default() }
    }

    /// Draw `other` with its origin at (dx, dy) and grow to cover it.
    fn add(&mut self, other: &B, dx: f64, dy: f64) {
        for op in &other.ops {
            self.ops.push(match *op {
                MathOp::Glyph { x, y, gid, size } => MathOp::Glyph {
                    x: x + dx,
                    y: y + dy,
                    gid,
                    size,
                },
                MathOp::Rule { x, y, w, h } => MathOp::Rule {
                    x: x + dx,
                    y: y + dy,
                    w,
                    h,
                },
            });
        }
        self.width = self.width.max(dx + other.width);
        self.height = self.height.max(dy + other.height);
        self.depth = self.depth.max(other.depth - dy);
        self.glyph = None;
    }

    fn rule(&mut self, x: f64, y: f64, w: f64, h: f64) {
        self.ops.push(MathOp::Rule { x, y, w, h });
        self.width = self.width.max(x + w);
        self.height = self.height.max(y + h);
        self.depth = self.depth.max(-y);
        self.glyph = None;
    }

    /// Move the contents vertically by `dy` (positive = up).
    fn raised(mut self, dy: f64) -> B {
        for op in self.ops.iter_mut() {
            match op {
                MathOp::Glyph { y, .. } | MathOp::Rule { y, .. } => *y += dy,
            }
        }
        self.height += dy;
        self.depth -= dy;
        self
    }
}

pub struct Layout<'f> {
    font: &'f mut EmbeddedFont,
    table: Table<'static>,
    c: Constants<'static>,
    base: f64,
    /// Size for `\middle` delimiters while laying out a `\left…\right` body.
    middle_target: Option<f64>,
}

impl<'f> Layout<'f> {
    pub fn new(font: &'f mut EmbeddedFont) -> Result<Self> {
        let table = font.face.tables().math.ok_or("math font has no MATH table")?;
        let c = table.constants.ok_or("math font has no MATH constants")?;
        Ok(Layout {
            font,
            table,
            c,
            base: 10.0,
            middle_target: None,
        })
    }

    pub fn typeset(&mut self, nodes: &[Node], size: f64, display: bool) -> Result<MathBox> {
        self.base = size;
        let st = St {
            style: if display { Style::Display } else { Style::Text },
            cramped: false,
        };
        let b = self.row(nodes, st)?;
        Ok(MathBox {
            width: b.width,
            height: b.height,
            depth: b.depth,
            ops: b.ops,
        })
    }

    // --- font access -------------------------------------------------------

    fn size(&self, style: Style) -> f64 {
        let pct = match style {
            Style::Display | Style::Text => 100,
            Style::Script => self.c.script_percent_scale_down(),
            Style::ScriptScript => self.c.script_script_percent_scale_down(),
        };
        self.base * pct as f64 / 100.0
    }

    /// Font units → points at `size`.
    fn pt(&self, units: f64, size: f64) -> f64 {
        units * size / self.font.upem
    }

    fn k(&self, v: MathValue, size: f64) -> f64 {
        self.pt(v.value as f64, size)
    }

    fn gid(&self, ch: char) -> Result<u16> {
        self.font
            .face
            .glyph_index(ch)
            .map(|g| g.0)
            .ok_or_else(|| format!("math font has no glyph for {ch:?}"))
    }

    fn glyph_box(&mut self, gid: u16, size: f64) -> B {
        let adv = self.font.mark(gid);
        let (h, d) = match self.font.face.glyph_bounding_box(GlyphId(gid)) {
            Some(bb) => (bb.y_max as f64, -(bb.y_min as f64)),
            None => (0.0, 0.0),
        };
        let italic = self
            .table
            .glyph_info
            .and_then(|gi| gi.italic_corrections)
            .and_then(|ic| ic.get(GlyphId(gid)))
            .map(|v| v.value as f64)
            .unwrap_or(0.0);
        B {
            width: self.pt(adv, size),
            height: self.pt(h, size),
            depth: self.pt(d, size),
            italic: self.pt(italic, size),
            glyph: Some(gid),
            ops: vec![MathOp::Glyph {
                x: 0.0,
                y: 0.0,
                gid,
                size,
            }],
        }
    }

    fn char_box(&mut self, ch: char, size: f64) -> Result<B> {
        let gid = self.gid(ch)?;
        Ok(self.glyph_box(gid, size))
    }

    /// Upright text set in the math font (`\text`, operator names).
    fn text_box(&mut self, text: &str, size: f64, bold: bool) -> Result<B> {
        let mut b = B::default();
        for ch in text.chars() {
            if ch == ' ' {
                let w = match self.font.face.glyph_index(' ') {
                    Some(g) => {
                        let adv = self.font.mark(g.0);
                        self.pt(adv, size)
                    }
                    None => size * 0.333,
                };
                b.width += w;
                continue;
            }
            let ch = if bold {
                super::symbols::map_char(ch, super::symbols::Variant::Bold)
            } else {
                ch
            };
            let g = self.char_box(ch, size)?;
            let x = b.width;
            b.add(&g, x, 0.0);
            b.italic = g.italic;
        }
        Ok(b)
    }

    // --- rows and spacing --------------------------------------------------

    fn row(&mut self, nodes: &[Node], st: St) -> Result<B> {
        let mut st = st;
        let mut items: Vec<(Option<Class>, B, St)> = Vec::new();
        for node in nodes {
            match node {
                Node::StyleChange(s) => st.style = *s,
                Node::Space(mu) => {
                    let w = mu * self.size(st.style) / 18.0;
                    items.push((None, B::space(w), st));
                }
                _ => {
                    let (class, b) = self.atom(node, st)?;
                    items.push((Some(class), b, st));
                }
            }
        }

        // TeX's rule: a Bin with nothing binary to its left/right is an Ord.
        let mut prev: Option<usize> = None;
        for i in 0..items.len() {
            let Some(cls) = items[i].0 else { continue };
            let prev_cls = prev.and_then(|p| items[p].0);
            if cls == Class::Bin
                && matches!(
                    prev_cls,
                    None | Some(Class::Bin | Class::Op | Class::Rel | Class::Open | Class::Punct)
                )
            {
                items[i].0 = Some(Class::Ord);
            }
            if matches!(cls, Class::Rel | Class::Close | Class::Punct)
                && let Some(p) = prev
                && items[p].0 == Some(Class::Bin)
            {
                items[p].0 = Some(Class::Ord);
            }
            prev = Some(i);
        }
        if let Some(p) = prev
            && items[p].0 == Some(Class::Bin)
        {
            items[p].0 = Some(Class::Ord);
        }

        let mut out = B::default();
        let mut x = 0.0;
        let mut last: Option<Class> = None;
        let single = items.len() == 1;
        let mut italic = 0.0;
        for (cls, b, ist) in &items {
            if let (Some(l), Some(r)) = (last, cls) {
                x += spacing(l, *r, ist.style) * self.size(ist.style) / 18.0;
            }
            out.add(b, x, 0.0);
            x += b.width;
            out.width = x;
            italic = b.italic;
            if cls.is_some() {
                last = *cls;
            }
        }
        out.italic = italic;
        if single {
            out.glyph = items[0].1.glyph;
        }
        Ok(out)
    }

    fn atom(&mut self, node: &Node, st: St) -> Result<(Class, B)> {
        let size = self.size(st.style);
        Ok(match node {
            Node::Sym { ch, class } => (*class, self.char_box(*ch, size)?),
            Node::Empty => (Class::Ord, B::default()),
            Node::Group(nodes) => {
                let mut b = self.row(nodes, st)?;
                b.glyph = None;
                (Class::Ord, b)
            }
            Node::Text { text, bold } => (Class::Ord, self.text_box(text, size, *bold)?),
            Node::Op { ch, .. } => (Class::Op, self.big_op(*ch, st)?),
            Node::NamedOp { text, .. } => (Class::Op, self.text_box(text, size, false)?),
            Node::Frac {
                num,
                den,
                rule,
                style,
                delims,
            } => {
                let fst = match style {
                    Some(s) => St {
                        style: *s,
                        cramped: st.cramped,
                    },
                    None => st,
                };
                let f = self.frac(num, den, *rule, fst)?;
                let b = match delims {
                    Some((l, r)) => {
                        // TeX's \delim1 / \delim2 sizes for generalized fractions.
                        let fsize = self.size(fst.style);
                        let target = if fst.display() { 2.39 * fsize } else { 1.01 * fsize };
                        self.wrap_delims(f, Some(*l), Some(*r), Some(target), fst)?
                    }
                    None => f,
                };
                (Class::Inner, b)
            }
            Node::Sqrt { index, body } => (Class::Ord, self.sqrt(index.as_deref(), body, st)?),
            Node::Scripts { base, sub, sup } => self.scripts(base, sub.as_deref(), sup.as_deref(), st)?,
            Node::LeftRight { left, right, body } => {
                let saved = self.middle_target.take();
                let mut inner = self.row(body, st)?;
                let target = self.delimiter_target(inner.height, inner.depth, size);
                if body.iter().any(|n| matches!(n, Node::Middle(_))) {
                    // Second pass with \middle delimiters at the final size.
                    self.middle_target = Some(target);
                    inner = self.row(body, st)?;
                }
                self.middle_target = saved;
                (Class::Inner, self.wrap_delims(inner, *left, *right, Some(target), st)?)
            }
            Node::Middle(ch) => {
                let target = self.middle_target.unwrap_or(0.0);
                (Class::Ord, self.delim(*ch, target, st)?)
            }
            Node::BigDelim { ch, step, class } => {
                // TeX's \big..\Bigg: a delimiter sized as if for content
                // 8.5/11.5/14.5/17.5 pt tall at 10 pt.
                let h = [0.85, 1.15, 1.45, 1.75][(*step as usize).clamp(1, 4) - 1] * size;
                let target = self.delimiter_target(h, 0.0, size);
                (*class, self.delim(*ch, target, st)?)
            }
            Node::Accent { accent, wide, body } => (Class::Ord, self.accent(*accent, *wide, body, st)?),
            Node::Overline(body) => {
                let b = self.layout_cramped(body, st)?;
                let gap = self.k(self.c.overbar_vertical_gap(), size);
                let t = self.k(self.c.overbar_rule_thickness(), size);
                let extra = self.k(self.c.overbar_extra_ascender(), size);
                let mut out = B::default();
                out.add(&b, 0.0, 0.0);
                out.rule(0.0, b.height + gap, b.width, t);
                out.height += extra;
                (Class::Ord, out)
            }
            Node::Underline(body) => {
                let b = self.layout_node(body, st)?;
                let gap = self.k(self.c.underbar_vertical_gap(), size);
                let t = self.k(self.c.underbar_rule_thickness(), size);
                let extra = self.k(self.c.underbar_extra_descender(), size);
                let mut out = B::default();
                out.add(&b, 0.0, 0.0);
                out.rule(0.0, -(b.depth + gap + t), b.width, t);
                out.depth += extra;
                (Class::Ord, out)
            }
            Node::Array {
                kind,
                rows,
                left,
                right,
            } => {
                let a = self.array(kind, rows, st)?;
                if left.is_some() || right.is_some() {
                    (Class::Inner, self.wrap_delims(a, *left, *right, None, st)?)
                } else {
                    (Class::Ord, a)
                }
            }
            Node::Space(mu) => (Class::Ord, B::space(mu * size / 18.0)),
            Node::StyleChange(_) => (Class::Ord, B::default()),
        })
    }

    fn layout_node(&mut self, node: &Node, st: St) -> Result<B> {
        Ok(self.atom(node, st)?.1)
    }

    fn layout_cramped(&mut self, node: &Node, st: St) -> Result<B> {
        self.layout_node(
            node,
            St {
                cramped: true,
                ..st
            },
        )
    }

    // --- operators, scripts, limits ----------------------------------------

    fn big_op(&mut self, ch: char, st: St) -> Result<B> {
        let size = self.size(st.style);
        let mut gid = self.gid(ch)?;
        if st.display() {
            let min_h = self.c.display_operator_min_height() as f64;
            if let Some(cons) = self
                .table
                .variants
                .and_then(|v| v.vertical_constructions.get(GlyphId(gid)))
            {
                let mut pick = None;
                for v in cons.variants {
                    pick = Some(v.variant_glyph.0);
                    if v.advance_measurement as f64 >= min_h {
                        break;
                    }
                }
                if let Some(p) = pick {
                    gid = p;
                }
            }
        }
        let b = self.glyph_box(gid, size);
        // Centre on the math axis.
        let axis = self.k(self.c.axis_height(), size);
        let shift = (b.height - b.depth) / 2.0 - axis;
        let italic = b.italic;
        let glyph = b.glyph;
        let mut b = b.raised(-shift);
        b.italic = italic;
        b.glyph = glyph;
        Ok(b)
    }

    fn scripts(&mut self, base: &Node, sub: Option<&Node>, sup: Option<&Node>, st: St) -> Result<(Class, B)> {
        let (class, bbox) = self.atom(base, st)?;
        let limits = match base {
            Node::Op {
                default_limits, limits, ..
            }
            | Node::NamedOp {
                default_limits, limits, ..
            } => limits.unwrap_or(*default_limits && st.display()),
            _ => false,
        };
        let sub_b = sub.map(|n| self.layout_node(n, st.sub())).transpose()?;
        let sup_b = sup.map(|n| self.layout_node(n, st.sup())).transpose()?;
        let size = self.size(st.style);
        let c = self.c;

        if limits {
            let ic = bbox.italic;
            let w = [Some(&bbox), sub_b.as_ref(), sup_b.as_ref()]
                .into_iter()
                .flatten()
                .map(|b| b.width)
                .fold(0.0, f64::max);
            let mut parts: Vec<(f64, f64, &B)> = vec![((w - bbox.width) / 2.0, 0.0, &bbox)];
            if let Some(o) = &sup_b {
                let rise = self
                    .k(c.upper_limit_baseline_rise_min(), size)
                    .max(self.k(c.upper_limit_gap_min(), size) + o.depth);
                parts.push(((w - o.width) / 2.0 + ic / 2.0, bbox.height + rise, o));
            }
            if let Some(u) = &sub_b {
                let drop = self
                    .k(c.lower_limit_baseline_drop_min(), size)
                    .max(self.k(c.lower_limit_gap_min(), size) + u.height);
                parts.push(((w - u.width) / 2.0 - ic / 2.0, -(bbox.depth + drop), u));
            }
            let min_x = parts.iter().map(|p| p.0).fold(0.0, f64::min);
            let mut out = B::default();
            for (x, y, b) in parts {
                out.add(b, x - min_x, y);
            }
            return Ok((class, out));
        }

        let is_char = bbox.glyph.is_some() && matches!(base, Node::Sym { .. });
        let is_op = matches!(base, Node::Op { .. });
        let mut sup_shift = 0.0;
        let mut sub_shift = 0.0;
        if let Some(s) = &sup_b {
            let up = if st.cramped {
                c.superscript_shift_up_cramped()
            } else {
                c.superscript_shift_up()
            };
            sup_shift = self
                .k(up, size)
                .max(s.depth + self.k(c.superscript_bottom_min(), size));
            if !is_char {
                sup_shift = sup_shift.max(bbox.height - self.k(c.superscript_baseline_drop_max(), size));
            }
        }
        if let Some(s) = &sub_b {
            sub_shift = self
                .k(c.subscript_shift_down(), size)
                .max(s.height - self.k(c.subscript_top_max(), size));
            if !is_char {
                sub_shift = sub_shift.max(bbox.depth + self.k(c.subscript_baseline_drop_min(), size));
            }
        }
        if let (Some(p), Some(b)) = (&sup_b, &sub_b) {
            let gap = (sup_shift - p.depth) - (b.height - sub_shift);
            let gap_min = self.k(c.sub_superscript_gap_min(), size);
            if gap < gap_min {
                sub_shift += gap_min - gap;
            }
            let bottom_max = self.k(c.superscript_bottom_max_with_subscript(), size);
            let d = bottom_max - (sup_shift - p.depth);
            if d > 0.0 {
                sup_shift += d;
                sub_shift -= d;
            }
        }

        // OpenType: superscripts sit after the italic correction; on large
        // operators the subscript instead tucks back by it.
        let ic = bbox.italic;
        let (sup_x, sub_x) = if is_op {
            (bbox.width, bbox.width - ic)
        } else {
            (bbox.width + ic, bbox.width)
        };
        let after = self.k(c.space_after_script(), size);
        let mut out = B::default();
        out.add(&bbox, 0.0, 0.0);
        let mut w = bbox.width;
        if let Some(p) = &sup_b {
            out.add(p, sup_x, sup_shift);
            w = w.max(sup_x + p.width);
        }
        if let Some(b) = &sub_b {
            out.add(b, sub_x, -sub_shift);
            w = w.max(sub_x + b.width);
        }
        out.width = w + after;
        Ok((class, out))
    }

    // --- fractions, radicals, accents --------------------------------------

    fn frac(&mut self, num: &Node, den: &Node, rule: bool, st: St) -> Result<B> {
        let size = self.size(st.style);
        let c = self.c;
        let n = self.layout_node(num, st.num())?;
        let d = self.layout_node(den, st.den())?;
        let axis = self.k(c.axis_height(), size);
        let disp = st.display();
        let thick = self.k(c.fraction_rule_thickness(), size);
        let (num_shift, den_shift) = if rule {
            let (up, down, ngap, dgap) = if disp {
                (
                    c.fraction_numerator_display_style_shift_up(),
                    c.fraction_denominator_display_style_shift_down(),
                    c.fraction_num_display_style_gap_min(),
                    c.fraction_denom_display_style_gap_min(),
                )
            } else {
                (
                    c.fraction_numerator_shift_up(),
                    c.fraction_denominator_shift_down(),
                    c.fraction_numerator_gap_min(),
                    c.fraction_denominator_gap_min(),
                )
            };
            (
                self.k(up, size)
                    .max(axis + thick / 2.0 + self.k(ngap, size) + n.depth),
                self.k(down, size)
                    .max(d.height + self.k(dgap, size) + thick / 2.0 - axis),
            )
        } else {
            let (up, down, gmin) = if disp {
                (
                    c.stack_top_display_style_shift_up(),
                    c.stack_bottom_display_style_shift_down(),
                    c.stack_display_style_gap_min(),
                )
            } else {
                (c.stack_top_shift_up(), c.stack_bottom_shift_down(), c.stack_gap_min())
            };
            let (mut u, mut v) = (self.k(up, size), self.k(down, size));
            let gap = (u - n.depth) - (d.height - v);
            let gmin = self.k(gmin, size);
            if gap < gmin {
                u += (gmin - gap) / 2.0;
                v += (gmin - gap) / 2.0;
            }
            (u, v)
        };
        // TeX pads fractions with \nulldelimiterspace on both sides.
        let pad = 0.12 * size;
        let w = n.width.max(d.width);
        let mut out = B::default();
        out.add(&n, pad + (w - n.width) / 2.0, num_shift);
        out.add(&d, pad + (w - d.width) / 2.0, -den_shift);
        if rule {
            out.rule(pad, axis - thick / 2.0, w, thick);
        }
        out.width = w + 2.0 * pad;
        Ok(out)
    }

    fn sqrt(&mut self, index: Option<&Node>, body: &Node, st: St) -> Result<B> {
        let size = self.size(st.style);
        let c = self.c;
        let b = self.layout_cramped(body, st)?;
        let mut gap = self.k(
            if st.display() {
                c.radical_display_style_vertical_gap()
            } else {
                c.radical_vertical_gap()
            },
            size,
        );
        let thick = self.k(c.radical_rule_thickness(), size);
        let extra = self.k(c.radical_extra_ascender(), size);
        let target = b.height + b.depth + gap + thick;
        let rad = self.stretch('\u{221A}', target, size)?;
        let rad_total = rad.height + rad.depth;
        if rad_total > target {
            gap += (rad_total - target) / 2.0;
        }
        // Put the radical's top at the rule's top.
        let top = b.height + gap + thick;
        let rad_shift = top - rad.height;

        let mut out = B::default();
        let mut x = 0.0;
        if let Some(idx) = index {
            let ib = self.layout_node(
                idx,
                St {
                    style: Style::ScriptScript,
                    cramped: true,
                },
            )?;
            let before = self.k(c.radical_kern_before_degree(), size);
            let after = self.k(c.radical_kern_after_degree(), size);
            let raise = c.radical_degree_bottom_raise_percent() as f64 / 100.0 * rad_total;
            let bottom = rad_shift - rad.depth;
            out.add(&ib, before, bottom + raise + ib.depth);
            x = (before + ib.width + after).max(0.0);
        }
        out.add(&rad, x, rad_shift);
        let bx = x + rad.width;
        out.add(&b, bx, 0.0);
        out.rule(bx, top - thick, b.width, thick);
        out.height = out.height.max(top + extra);
        out.width = bx + b.width;
        Ok(out)
    }

    fn accent(&mut self, accent: char, wide: bool, body: &Node, st: St) -> Result<B> {
        let size = self.size(st.style);
        let b = self.layout_cramped(body, st)?;
        let mut agid = self.gid(accent)?;
        if wide
            && let Some(cons) = self
                .table
                .variants
                .and_then(|v| v.horizontal_constructions.get(GlyphId(agid)))
        {
            let need = b.width * self.font.upem / size;
            for v in cons.variants {
                agid = v.variant_glyph.0;
                if v.advance_measurement as f64 >= need {
                    break;
                }
            }
        }
        let a = self.glyph_box(agid, size);
        let attach = |l: &Self, gid: u16| -> Option<f64> {
            l.table
                .glyph_info
                .and_then(|gi| gi.top_accent_attachments)
                .and_then(|t| t.get(GlyphId(gid)))
                .map(|v| v.value as f64)
        };
        let ink_center = |l: &Self, gid: u16| -> f64 {
            l.font
                .face
                .glyph_bounding_box(GlyphId(gid))
                .map(|bb| (bb.x_min as f64 + bb.x_max as f64) / 2.0)
                .unwrap_or(0.0)
        };
        let base_x = match b.glyph {
            Some(g) => match attach(self, g) {
                Some(u) => self.pt(u, size),
                None => b.width / 2.0 + b.italic / 2.0,
            },
            None => b.width / 2.0,
        };
        let acc_x = self.pt(attach(self, agid).unwrap_or_else(|| ink_center(self, agid)), size);
        let abh = self.k(self.c.accent_base_height(), size);
        let shift = (b.height - abh).max(0.0);
        let mut out = B::default();
        out.add(&b, 0.0, 0.0);
        let ax = base_x - acc_x;
        out.add(&a, ax, shift);
        out.width = b.width;
        out.italic = b.italic;
        Ok(out)
    }

    // --- delimiters and stretching -----------------------------------------

    /// TeX's delimiter size for content of the given height/depth around the
    /// axis: `max(2δ·0.901, 2δ − 5pt)`.
    fn delimiter_target(&self, height: f64, depth: f64, size: f64) -> f64 {
        let axis = self.k(self.c.axis_height(), size);
        let delta = (height - axis).max(depth + axis);
        (2.0 * delta * 0.901).max(2.0 * delta - 0.5 * size)
    }

    /// A delimiter of at least `target` total height, centred on the axis.
    /// `None` is TeX's null delimiter (an empty `\nulldelimiterspace`).
    fn delim(&mut self, ch: Option<char>, target: f64, st: St) -> Result<B> {
        let size = self.size(st.style);
        let Some(ch) = ch else {
            return Ok(B::space(0.12 * size));
        };
        let b = self.stretch(ch, target, size)?;
        let axis = self.k(self.c.axis_height(), size);
        let shift = (b.height - b.depth) / 2.0 - axis;
        Ok(b.raised(-shift))
    }

    fn wrap_delims(&mut self, inner: B, left: Option<char>, right: Option<char>, target: Option<f64>, st: St) -> Result<B> {
        let size = self.size(st.style);
        let target = target.unwrap_or_else(|| self.delimiter_target(inner.height, inner.depth, size));
        let l = self.delim(left, target, st)?;
        let r = self.delim(right, target, st)?;
        let mut out = B::default();
        out.add(&l, 0.0, 0.0);
        let x = l.width;
        out.add(&inner, x, 0.0);
        let x = x + inner.width;
        out.add(&r, x, 0.0);
        out.width = x + r.width;
        Ok(out)
    }

    /// The smallest vertical variant of `ch` at least `target` tall, or a glyph
    /// assembly built to that height.
    fn stretch(&mut self, ch: char, target: f64, size: f64) -> Result<B> {
        let gid = self.gid(ch)?;
        let base = self.glyph_box(gid, size);
        if base.height + base.depth >= target {
            return Ok(base);
        }
        let Some(variants) = self.table.variants else {
            return Ok(base);
        };
        let Some(cons) = variants.vertical_constructions.get(GlyphId(gid)) else {
            return Ok(base);
        };
        let need = target * self.font.upem / size;
        let mut largest = None;
        for v in cons.variants {
            largest = Some(v.variant_glyph.0);
            if v.advance_measurement as f64 >= need {
                return Ok(self.glyph_box(v.variant_glyph.0, size));
            }
        }
        if let Some(asm) = cons.assembly {
            return Ok(self.assembly(asm, need, variants.min_connector_overlap as f64, size));
        }
        Ok(match largest {
            Some(g) => self.glyph_box(g, size),
            None => base,
        })
    }

    /// Build a vertical glyph assembly of total height ≥ `need` (font units),
    /// repeating extenders as needed and spreading the overlap evenly.
    fn assembly(&mut self, asm: GlyphAssembly, need: f64, min_overlap: f64, size: f64) -> B {
        let parts: Vec<_> = asm.parts.into_iter().collect();
        let has_ext = parts.iter().any(|p| p.part_flags.extender());
        let expand = |reps: usize| -> Vec<ttf_parser::math::GlyphPart> {
            let mut v = Vec::new();
            for p in &parts {
                let n = if p.part_flags.extender() { reps } else { 1 };
                for _ in 0..n {
                    v.push(*p);
                }
            }
            v
        };
        let mut reps = 1;
        let mut seq = expand(reps);
        while has_ext && reps < 200 {
            let total: f64 = seq.iter().map(|p| p.full_advance as f64).sum();
            let max_len = total - (seq.len().saturating_sub(1)) as f64 * min_overlap;
            if max_len >= need {
                break;
            }
            reps += 1;
            seq = expand(reps);
        }
        let total: f64 = seq.iter().map(|p| p.full_advance as f64).sum();
        let joints = seq.len().saturating_sub(1) as f64;
        let max_overlap = seq
            .windows(2)
            .map(|w| w[0].end_connector_length.min(w[1].start_connector_length) as f64)
            .fold(f64::INFINITY, f64::min);
        let mut overlap = if joints > 0.0 { (total - need) / joints } else { 0.0 };
        overlap = overlap.min(max_overlap.max(min_overlap)).max(min_overlap);

        let mut out = B::default();
        let mut y = 0.0;
        for p in &seq {
            let gid = p.glyph_id.0;
            let g = self.glyph_box(gid, size);
            // Align each part's ink bottom with the running position.
            let bottom = self
                .font
                .face
                .glyph_bounding_box(p.glyph_id)
                .map(|bb| bb.y_min as f64)
                .unwrap_or(0.0);
            out.add(&g, 0.0, self.pt(y - bottom, size));
            y += p.full_advance as f64 - overlap;
        }
        let height = self.pt(y + overlap, size);
        out.height = height;
        out.depth = 0.0;
        out.italic = self.k(asm.italics_correction, size);
        out
    }

    // --- arrays ------------------------------------------------------------

    fn array(&mut self, kind: &ArrayKind, rows: &[Vec<Node>], st: St) -> Result<B> {
        let size = self.size(st.style);
        let cell_st = St {
            style: match kind {
                ArrayKind::Matrix { small: true } => Style::Script,
                ArrayKind::Matrix { .. } | ArrayKind::Cases | ArrayKind::Array(_) => {
                    if st.style <= Style::Text {
                        Style::Text
                    } else {
                        st.style
                    }
                }
                ArrayKind::Aligned | ArrayKind::Gathered => Style::Display,
            },
            cramped: false,
        };
        let ncols = rows.iter().map(|r| r.len()).max().unwrap_or(0);
        let mut boxes: Vec<Vec<B>> = Vec::with_capacity(rows.len());
        for row in rows {
            let mut r = Vec::with_capacity(ncols);
            for cell in row {
                r.push(self.layout_node(cell, cell_st)?);
            }
            r.resize_with(ncols, B::default);
            boxes.push(r);
        }

        let align = |j: usize| -> Align {
            match kind {
                ArrayKind::Matrix { .. } | ArrayKind::Gathered => Align::Center,
                ArrayKind::Cases => Align::Left,
                ArrayKind::Aligned => {
                    if j.is_multiple_of(2) {
                        Align::Right
                    } else {
                        Align::Left
                    }
                }
                ArrayKind::Array(cols) => cols.get(j).copied().unwrap_or(Align::Center),
            }
        };
        // Space before column j (j ≥ 1).
        let col_gap = |j: usize| -> f64 {
            match kind {
                ArrayKind::Aligned => {
                    if j.is_multiple_of(2) {
                        2.0 * size
                    } else {
                        0.0
                    }
                }
                ArrayKind::Matrix { small: true } => 0.5 * size,
                _ => size,
            }
        };

        let widths: Vec<f64> = (0..ncols)
            .map(|j| boxes.iter().map(|r| r[j].width).fold(0.0, f64::max))
            .collect();
        // Rows are at least a strut tall (0.7/0.3 of a 1.2 em baselineskip).
        let skip = 1.2 * self.size(cell_st.style);
        let jot = match kind {
            ArrayKind::Aligned | ArrayKind::Gathered => 0.3 * size,
            _ => 0.0,
        };
        let metrics: Vec<(f64, f64)> = boxes
            .iter()
            .map(|r| {
                let h = r.iter().map(|b| b.height).fold(0.7 * skip, f64::max);
                let d = r.iter().map(|b| b.depth).fold(0.3 * skip, f64::max);
                (h, d)
            })
            .collect();
        let total: f64 = metrics.iter().map(|(h, d)| h + d).sum::<f64>() + jot * (rows.len().saturating_sub(1)) as f64;
        let axis = self.k(self.c.axis_height(), size);
        let top = total / 2.0 + axis;

        let mut out = B::default();
        let mut y = top;
        for (i, r) in boxes.iter().enumerate() {
            let (h, d) = metrics[i];
            let baseline = y - h;
            let mut x = 0.0;
            for (j, b) in r.iter().enumerate() {
                if j > 0 {
                    x += col_gap(j);
                }
                let dx = match align(j) {
                    Align::Left => 0.0,
                    Align::Center => (widths[j] - b.width) / 2.0,
                    Align::Right => widths[j] - b.width,
                };
                out.add(b, x + dx, baseline);
                x += widths[j];
            }
            y = baseline - d - jot;
        }
        out.width = widths.iter().sum::<f64>() + (1..ncols).map(col_gap).sum::<f64>();
        out.height = top;
        out.depth = total - top;
        Ok(out)
    }
}

/// TeX's inter-atom spacing (TeXbook p. 170), in mu. Negative table entries are
/// only applied in display and text styles.
fn spacing(l: Class, r: Class, style: Style) -> f64 {
    use Class::*;
    let idx = |c: Class| match c {
        Ord => 0,
        Op => 1,
        Bin => 2,
        Rel => 3,
        Open => 4,
        Close => 5,
        Punct => 6,
        Inner => 7,
    };
    #[rustfmt::skip]
    const TABLE: [[i8; 8]; 8] = [
        // Ord Op  Bin Rel Open Close Punct Inner
        [  0,  1, -2, -3,  0,  0,  0, -1], // Ord
        [  1,  1,  0, -3,  0,  0,  0, -1], // Op
        [ -2, -2,  0,  0, -2,  0,  0, -2], // Bin
        [ -3, -3,  0,  0, -3,  0,  0, -3], // Rel
        [  0,  0,  0,  0,  0,  0,  0,  0], // Open
        [  0,  1, -2, -3,  0,  0,  0, -1], // Close
        [ -1, -1,  0, -1, -1, -1, -1, -1], // Punct
        [ -1,  1, -2, -3, -1,  0, -1, -1], // Inner
    ];
    let v = TABLE[idx(l)][idx(r)];
    if v < 0 && style > Style::Text {
        return 0.0;
    }
    match v.abs() {
        1 => 3.0,
        2 => 4.0,
        3 => 5.0,
        _ => 0.0,
    }
}
