//! TeX command tables and Unicode math-alphabet mapping.

/// TeX atom classes, which drive inter-atom spacing.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Class {
    Ord,
    Op,
    Bin,
    Rel,
    Open,
    Close,
    Punct,
    Inner,
}

/// A math alphabet (`\mathbf`, `\mathbb`, …). `Normal` is TeX's default:
/// italic Latin and lowercase Greek, upright digits and uppercase Greek.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Variant {
    Normal,
    Upright,
    Bold,
    BoldItalic,
    DoubleStruck,
    Script,
    Fraktur,
    SansSerif,
    Monospace,
}

/// Class of a plain source character.
pub fn char_class(c: char) -> (char, Class) {
    match c {
        '+' => ('+', Class::Bin),
        '-' => ('\u{2212}', Class::Bin),
        '*' => ('\u{2217}', Class::Bin),
        '=' | '<' | '>' | ':' => (c, Class::Rel),
        ',' | ';' => (c, Class::Punct),
        '(' | '[' => (c, Class::Open),
        ')' | ']' | '!' | '?' => (c, Class::Close),
        _ => (c, Class::Ord),
    }
}

/// Map a character into a math alphabet.
pub fn map_char(c: char, v: Variant) -> char {
    let off = |base: u32, from: char| char::from_u32(base + (c as u32 - from as u32)).unwrap_or(c);
    let upper = c.is_ascii_uppercase();
    let lower = c.is_ascii_lowercase();
    let digit = c.is_ascii_digit();
    let greek_lower = ('\u{03B1}'..='\u{03C9}').contains(&c);
    let greek_upper = ('\u{0391}'..='\u{03A9}').contains(&c) && c != '\u{03A2}';
    match v {
        Variant::Upright => c,
        Variant::Normal => {
            if c == 'h' {
                '\u{210E}'
            } else if upper {
                off(0x1D434, 'A')
            } else if lower {
                off(0x1D44E, 'a')
            } else if greek_lower {
                off(0x1D6FC, '\u{03B1}')
            } else {
                match c {
                    '\u{03F5}' => '\u{1D716}', // ϵ
                    '\u{03D1}' => '\u{1D717}', // ϑ
                    '\u{03F0}' => '\u{1D718}', // ϰ
                    '\u{03D5}' => '\u{1D719}', // ϕ
                    '\u{03F1}' => '\u{1D71A}', // ϱ
                    '\u{03D6}' => '\u{1D71B}', // ϖ
                    _ => c,
                }
            }
        }
        Variant::Bold => {
            if upper {
                off(0x1D400, 'A')
            } else if lower {
                off(0x1D41A, 'a')
            } else if digit {
                off(0x1D7CE, '0')
            } else if greek_upper {
                off(0x1D6A8, '\u{0391}')
            } else if greek_lower {
                off(0x1D6C2, '\u{03B1}')
            } else {
                c
            }
        }
        Variant::BoldItalic => {
            if upper {
                off(0x1D468, 'A')
            } else if lower {
                off(0x1D482, 'a')
            } else if digit {
                off(0x1D7CE, '0')
            } else if greek_upper {
                off(0x1D71C, '\u{0391}')
            } else if greek_lower {
                off(0x1D736, '\u{03B1}')
            } else {
                c
            }
        }
        Variant::DoubleStruck => match c {
            'C' => '\u{2102}',
            'H' => '\u{210D}',
            'N' => '\u{2115}',
            'P' => '\u{2119}',
            'Q' => '\u{211A}',
            'R' => '\u{211D}',
            'Z' => '\u{2124}',
            _ if upper => off(0x1D538, 'A'),
            _ if lower => off(0x1D552, 'a'),
            _ if digit => off(0x1D7D8, '0'),
            _ => c,
        },
        Variant::Script => match c {
            'B' => '\u{212C}',
            'E' => '\u{2130}',
            'F' => '\u{2131}',
            'H' => '\u{210B}',
            'I' => '\u{2110}',
            'L' => '\u{2112}',
            'M' => '\u{2133}',
            'R' => '\u{211B}',
            _ if upper => off(0x1D49C, 'A'),
            // Latin Modern Math has no lowercase script letters (nor does
            // LaTeX's \mathcal), so lowercase falls back to math italic.
            _ if lower => map_char(c, Variant::Normal),
            _ => c,
        },
        Variant::Fraktur => match c {
            'C' => '\u{212D}',
            'H' => '\u{210C}',
            'I' => '\u{2111}',
            'R' => '\u{211C}',
            'Z' => '\u{2128}',
            _ if upper => off(0x1D504, 'A'),
            _ if lower => off(0x1D51E, 'a'),
            _ => c,
        },
        Variant::SansSerif => {
            if upper {
                off(0x1D5A0, 'A')
            } else if lower {
                off(0x1D5BA, 'a')
            } else if digit {
                off(0x1D7E2, '0')
            } else {
                c
            }
        }
        Variant::Monospace => {
            if upper {
                off(0x1D670, 'A')
            } else if lower {
                off(0x1D68A, 'a')
            } else if digit {
                off(0x1D7F6, '0')
            } else {
                c
            }
        }
    }
}

/// Symbol commands: `\alpha`, `\le`, `\infty`, …
pub const SYMBOLS: &[(&str, char, Class)] = {
    use Class::*;
    &[
        // Greek, lowercase (variant-mapped like letters).
        ("alpha", '\u{03B1}', Ord),
        ("beta", '\u{03B2}', Ord),
        ("gamma", '\u{03B3}', Ord),
        ("delta", '\u{03B4}', Ord),
        ("epsilon", '\u{03F5}', Ord),
        ("varepsilon", '\u{03B5}', Ord),
        ("zeta", '\u{03B6}', Ord),
        ("eta", '\u{03B7}', Ord),
        ("theta", '\u{03B8}', Ord),
        ("vartheta", '\u{03D1}', Ord),
        ("iota", '\u{03B9}', Ord),
        ("kappa", '\u{03BA}', Ord),
        ("varkappa", '\u{03F0}', Ord),
        ("lambda", '\u{03BB}', Ord),
        ("mu", '\u{03BC}', Ord),
        ("nu", '\u{03BD}', Ord),
        ("xi", '\u{03BE}', Ord),
        ("omicron", '\u{03BF}', Ord),
        ("pi", '\u{03C0}', Ord),
        ("varpi", '\u{03D6}', Ord),
        ("rho", '\u{03C1}', Ord),
        ("varrho", '\u{03F1}', Ord),
        ("sigma", '\u{03C3}', Ord),
        ("varsigma", '\u{03C2}', Ord),
        ("tau", '\u{03C4}', Ord),
        ("upsilon", '\u{03C5}', Ord),
        ("phi", '\u{03D5}', Ord),
        ("varphi", '\u{03C6}', Ord),
        ("chi", '\u{03C7}', Ord),
        ("psi", '\u{03C8}', Ord),
        ("omega", '\u{03C9}', Ord),
        // Greek, uppercase (upright by default).
        ("Gamma", '\u{0393}', Ord),
        ("Delta", '\u{0394}', Ord),
        ("Theta", '\u{0398}', Ord),
        ("Lambda", '\u{039B}', Ord),
        ("Xi", '\u{039E}', Ord),
        ("Pi", '\u{03A0}', Ord),
        ("Sigma", '\u{03A3}', Ord),
        ("Upsilon", '\u{03A5}', Ord),
        ("Phi", '\u{03A6}', Ord),
        ("Psi", '\u{03A8}', Ord),
        ("Omega", '\u{03A9}', Ord),
        // Binary operators.
        ("pm", '\u{00B1}', Bin),
        ("mp", '\u{2213}', Bin),
        ("times", '\u{00D7}', Bin),
        ("div", '\u{00F7}', Bin),
        ("cdot", '\u{22C5}', Bin),
        ("ast", '\u{2217}', Bin),
        ("star", '\u{22C6}', Bin),
        ("circ", '\u{2218}', Bin),
        ("bullet", '\u{2219}', Bin),
        ("oplus", '\u{2295}', Bin),
        ("ominus", '\u{2296}', Bin),
        ("otimes", '\u{2297}', Bin),
        ("oslash", '\u{2298}', Bin),
        ("odot", '\u{2299}', Bin),
        ("cup", '\u{222A}', Bin),
        ("cap", '\u{2229}', Bin),
        ("sqcup", '\u{2294}', Bin),
        ("sqcap", '\u{2293}', Bin),
        ("uplus", '\u{228E}', Bin),
        ("wedge", '\u{2227}', Bin),
        ("land", '\u{2227}', Bin),
        ("vee", '\u{2228}', Bin),
        ("lor", '\u{2228}', Bin),
        ("setminus", '\u{2216}', Bin),
        ("wr", '\u{2240}', Bin),
        ("diamond", '\u{22C4}', Bin),
        ("amalg", '\u{2A3F}', Bin),
        ("dagger", '\u{2020}', Bin),
        ("ddagger", '\u{2021}', Bin),
        // Relations.
        ("le", '\u{2264}', Rel),
        ("leq", '\u{2264}', Rel),
        ("ge", '\u{2265}', Rel),
        ("geq", '\u{2265}', Rel),
        ("leqslant", '\u{2A7D}', Rel),
        ("geqslant", '\u{2A7E}', Rel),
        ("ne", '\u{2260}', Rel),
        ("neq", '\u{2260}', Rel),
        ("equiv", '\u{2261}', Rel),
        ("approx", '\u{2248}', Rel),
        ("sim", '\u{223C}', Rel),
        ("simeq", '\u{2243}', Rel),
        ("cong", '\u{2245}', Rel),
        ("propto", '\u{221D}', Rel),
        ("ll", '\u{226A}', Rel),
        ("gg", '\u{226B}', Rel),
        ("prec", '\u{227A}', Rel),
        ("succ", '\u{227B}', Rel),
        ("preceq", '\u{2AAF}', Rel),
        ("succeq", '\u{2AB0}', Rel),
        ("subset", '\u{2282}', Rel),
        ("supset", '\u{2283}', Rel),
        ("subseteq", '\u{2286}', Rel),
        ("supseteq", '\u{2287}', Rel),
        ("subsetneq", '\u{228A}', Rel),
        ("supsetneq", '\u{228B}', Rel),
        ("in", '\u{2208}', Rel),
        ("notin", '\u{2209}', Rel),
        ("ni", '\u{220B}', Rel),
        ("owns", '\u{220B}', Rel),
        ("to", '\u{2192}', Rel),
        ("rightarrow", '\u{2192}', Rel),
        ("gets", '\u{2190}', Rel),
        ("leftarrow", '\u{2190}', Rel),
        ("leftrightarrow", '\u{2194}', Rel),
        ("Rightarrow", '\u{21D2}', Rel),
        ("Leftarrow", '\u{21D0}', Rel),
        ("Leftrightarrow", '\u{21D4}', Rel),
        ("longrightarrow", '\u{27F6}', Rel),
        ("longleftarrow", '\u{27F5}', Rel),
        ("longleftrightarrow", '\u{27F7}', Rel),
        ("Longrightarrow", '\u{27F9}', Rel),
        ("implies", '\u{27F9}', Rel),
        ("Longleftarrow", '\u{27F8}', Rel),
        ("impliedby", '\u{27F8}', Rel),
        ("Longleftrightarrow", '\u{27FA}', Rel),
        ("iff", '\u{27FA}', Rel),
        ("mapsto", '\u{21A6}', Rel),
        ("longmapsto", '\u{27FC}', Rel),
        ("uparrow", '\u{2191}', Rel),
        ("downarrow", '\u{2193}', Rel),
        ("updownarrow", '\u{2195}', Rel),
        ("Uparrow", '\u{21D1}', Rel),
        ("Downarrow", '\u{21D3}', Rel),
        ("hookrightarrow", '\u{21AA}', Rel),
        ("hookleftarrow", '\u{21A9}', Rel),
        ("rightharpoonup", '\u{21C0}', Rel),
        ("leftharpoonup", '\u{21BC}', Rel),
        ("rightleftharpoons", '\u{21CC}', Rel),
        ("nearrow", '\u{2197}', Rel),
        ("searrow", '\u{2198}', Rel),
        ("perp", '\u{27C2}', Rel),
        ("parallel", '\u{2225}', Rel),
        ("mid", '\u{2223}', Rel),
        ("vdash", '\u{22A2}', Rel),
        ("dashv", '\u{22A3}', Rel),
        ("models", '\u{22A8}', Rel),
        ("doteq", '\u{2250}', Rel),
        ("coloneqq", '\u{2254}', Rel),
        ("asymp", '\u{224D}', Rel),
        ("bowtie", '\u{22C8}', Rel),
        // Ordinary symbols.
        ("infty", '\u{221E}', Ord),
        ("partial", '\u{2202}', Ord),
        ("nabla", '\u{2207}', Ord),
        ("forall", '\u{2200}', Ord),
        ("exists", '\u{2203}', Ord),
        ("nexists", '\u{2204}', Ord),
        ("emptyset", '\u{2205}', Ord),
        ("varnothing", '\u{2300}', Ord),
        ("hbar", '\u{210F}', Ord),
        ("ell", '\u{2113}', Ord),
        ("wp", '\u{2118}', Ord),
        ("Re", '\u{211C}', Ord),
        ("Im", '\u{2111}', Ord),
        ("aleph", '\u{2135}', Ord),
        ("prime", '\u{2032}', Ord),
        ("angle", '\u{2220}', Ord),
        ("triangle", '\u{25B3}', Ord),
        ("neg", '\u{00AC}', Ord),
        ("lnot", '\u{00AC}', Ord),
        ("top", '\u{22A4}', Ord),
        ("bot", '\u{22A5}', Ord),
        ("ldots", '\u{2026}', Inner),
        ("dots", '\u{2026}', Inner),
        ("dotsc", '\u{2026}', Inner),
        ("dotso", '\u{2026}', Inner),
        ("cdots", '\u{22EF}', Inner),
        ("dotsb", '\u{22EF}', Inner),
        ("dotsm", '\u{22EF}', Inner),
        ("dotsi", '\u{22EF}', Inner),
        ("vdots", '\u{22EE}', Ord),
        ("ddots", '\u{22F1}', Inner),
        ("degree", '\u{00B0}', Ord),
        ("backslash", '\\', Ord),
        ("|", '\u{2016}', Ord),
        ("Vert", '\u{2016}', Ord),
        ("vert", '|', Ord),
        ("imath", '\u{1D6A4}', Ord),
        ("jmath", '\u{1D6A5}', Ord),
        ("clubsuit", '\u{2663}', Ord),
        ("diamondsuit", '\u{2662}', Ord),
        ("heartsuit", '\u{2661}', Ord),
        ("spadesuit", '\u{2660}', Ord),
        ("checkmark", '\u{2713}', Ord),
        // Delimiters as plain symbols.
        ("{", '{', Open),
        ("lbrace", '{', Open),
        ("}", '}', Close),
        ("rbrace", '}', Close),
        ("langle", '\u{27E8}', Open),
        ("rangle", '\u{27E9}', Close),
        ("lfloor", '\u{230A}', Open),
        ("rfloor", '\u{230B}', Close),
        ("lceil", '\u{2308}', Open),
        ("rceil", '\u{2309}', Close),
        ("lvert", '|', Open),
        ("rvert", '|', Close),
        ("lVert", '\u{2016}', Open),
        ("rVert", '\u{2016}', Close),
        // Escaped specials.
        ("$", '$', Ord),
        ("%", '%', Ord),
        ("&", '&', Ord),
        ("#", '#', Ord),
        ("_", '_', Ord),
    ]
};

pub fn symbol(name: &str) -> Option<(char, Class)> {
    SYMBOLS.iter().find(|(n, ..)| *n == name).map(|&(_, c, k)| (c, k))
}

/// Large operators: (glyph, limits above/below in display style).
pub const BIG_OPS: &[(&str, char, bool)] = &[
    ("sum", '\u{2211}', true),
    ("prod", '\u{220F}', true),
    ("coprod", '\u{2210}', true),
    ("bigcup", '\u{22C3}', true),
    ("bigcap", '\u{22C2}', true),
    ("bigvee", '\u{22C1}', true),
    ("bigwedge", '\u{22C0}', true),
    ("bigoplus", '\u{2A01}', true),
    ("bigotimes", '\u{2A02}', true),
    ("bigodot", '\u{2A00}', true),
    ("biguplus", '\u{2A04}', true),
    ("bigsqcup", '\u{2A06}', true),
    ("int", '\u{222B}', false),
    ("iint", '\u{222C}', false),
    ("iiint", '\u{222D}', false),
    ("oint", '\u{222E}', false),
    ("oiint", '\u{222F}', false),
];

pub fn big_op(name: &str) -> Option<(char, bool)> {
    BIG_OPS.iter().find(|(n, ..)| *n == name).map(|&(_, c, l)| (c, l))
}

/// Named operators set in upright text: (text, limits above/below in display).
pub const NAMED_OPS: &[(&str, &str, bool)] = &[
    ("sin", "sin", false),
    ("cos", "cos", false),
    ("tan", "tan", false),
    ("cot", "cot", false),
    ("sec", "sec", false),
    ("csc", "csc", false),
    ("arcsin", "arcsin", false),
    ("arccos", "arccos", false),
    ("arctan", "arctan", false),
    ("sinh", "sinh", false),
    ("cosh", "cosh", false),
    ("tanh", "tanh", false),
    ("coth", "coth", false),
    ("log", "log", false),
    ("ln", "ln", false),
    ("lg", "lg", false),
    ("exp", "exp", false),
    ("dim", "dim", false),
    ("ker", "ker", false),
    ("deg", "deg", false),
    ("arg", "arg", false),
    ("hom", "hom", false),
    ("det", "det", true),
    ("gcd", "gcd", true),
    ("Pr", "Pr", true),
    ("lim", "lim", true),
    ("liminf", "lim inf", true),
    ("limsup", "lim sup", true),
    ("max", "max", true),
    ("min", "min", true),
    ("sup", "sup", true),
    ("inf", "inf", true),
    ("argmax", "arg max", true),
    ("argmin", "arg min", true),
];

pub fn named_op(name: &str) -> Option<(&'static str, bool)> {
    NAMED_OPS.iter().find(|(n, ..)| *n == name).map(|&(_, t, l)| (t, l))
}

/// Accent commands: (combining accent char, wide/stretchy).
pub const ACCENTS: &[(&str, char, bool)] = &[
    ("hat", '\u{0302}', false),
    ("widehat", '\u{0302}', true),
    ("check", '\u{030C}', false),
    ("widecheck", '\u{030C}', true),
    ("tilde", '\u{0303}', false),
    ("widetilde", '\u{0303}', true),
    ("bar", '\u{0304}', false),
    ("breve", '\u{0306}', false),
    ("acute", '\u{0301}', false),
    ("grave", '\u{0300}', false),
    ("dot", '\u{0307}', false),
    ("ddot", '\u{0308}', false),
    ("dddot", '\u{20DB}', false),
    ("mathring", '\u{030A}', false),
    ("vec", '\u{20D7}', false),
    ("overrightarrow", '\u{20D7}', true),
    ("overleftarrow", '\u{20D6}', true),
];

pub fn accent(name: &str) -> Option<(char, bool)> {
    ACCENTS.iter().find(|(n, ..)| *n == name).map(|&(_, c, w)| (c, w))
}

/// Delimiter after `\left`, `\right`, `\big`, …; `Ok(None)` is the null
/// delimiter `.`.
pub fn delimiter_char(c: char) -> Option<char> {
    match c {
        '(' | ')' | '[' | ']' | '|' | '/' => Some(c),
        _ => None,
    }
}

/// Delimiter commands accepted after `\left`, `\right`, `\big`, ….
pub const DELIMITERS: &[(&str, char)] = &[
    ("{", '{'),
    ("lbrace", '{'),
    ("}", '}'),
    ("rbrace", '}'),
    ("|", '\u{2016}'),
    ("Vert", '\u{2016}'),
    ("lVert", '\u{2016}'),
    ("rVert", '\u{2016}'),
    ("vert", '|'),
    ("lvert", '|'),
    ("rvert", '|'),
    ("mid", '|'),
    ("langle", '\u{27E8}'),
    ("rangle", '\u{27E9}'),
    ("lfloor", '\u{230A}'),
    ("rfloor", '\u{230B}'),
    ("lceil", '\u{2308}'),
    ("rceil", '\u{2309}'),
    ("backslash", '\\'),
    ("uparrow", '\u{2191}'),
    ("downarrow", '\u{2193}'),
    ("updownarrow", '\u{2195}'),
    ("Uparrow", '\u{21D1}'),
    ("Downarrow", '\u{21D3}'),
];

pub fn delimiter_cmd(name: &str) -> Option<char> {
    DELIMITERS.iter().find(|(n, _)| *n == name).map(|&(_, c)| c)
}
