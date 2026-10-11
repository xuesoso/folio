//! TeX math source → `Node` tree. A hand-written recursive-descent parser for
//! the commonly used subset of LaTeX/amsmath math; anything unknown is an error
//! (the caller then shows the source verbatim).

use super::symbols::{self, Class, Variant};

pub type Result<T> = std::result::Result<T, String>;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Style {
    Display,
    Text,
    Script,
    ScriptScript,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Align {
    Left,
    Center,
    Right,
}

#[derive(Debug, Clone, PartialEq)]
pub enum ArrayKind {
    /// `matrix` family: centred columns in text style.
    Matrix { small: bool },
    /// `cases`: two left-aligned columns, a quad apart.
    Cases,
    /// `aligned` family: alternating right/left columns in display style.
    Aligned,
    /// `gathered` family: one centred column in display style.
    Gathered,
    /// `array{…}` with explicit column alignments.
    Array(Vec<Align>),
}

#[derive(Debug, Clone, PartialEq)]
pub enum Node {
    Sym {
        ch: char,
        class: Class,
    },
    /// A large operator (∑, ∫, …). `limits`: explicit `\limits`/`\nolimits`.
    Op {
        ch: char,
        default_limits: bool,
        limits: Option<bool>,
    },
    /// An operator name set upright (`\sin`, `\lim`, `\operatorname{…}`).
    NamedOp {
        text: String,
        default_limits: bool,
        limits: Option<bool>,
    },
    /// Upright text (`\text{…}`), spaces kept.
    Text { text: String, bold: bool },
    Group(Vec<Node>),
    Frac {
        num: Box<Node>,
        den: Box<Node>,
        rule: bool,
        style: Option<Style>,
        delims: Option<(char, char)>,
    },
    Sqrt {
        index: Option<Box<Node>>,
        body: Box<Node>,
    },
    Scripts {
        base: Box<Node>,
        sub: Option<Box<Node>>,
        sup: Option<Box<Node>>,
    },
    LeftRight {
        left: Option<char>,
        right: Option<char>,
        body: Vec<Node>,
    },
    /// `\middle|` inside `\left…\right`: sized like the outer delimiters.
    Middle(Option<char>),
    /// `\big(` etc: a delimiter at a fixed size step (1..=4).
    BigDelim {
        ch: Option<char>,
        step: u8,
        class: Class,
    },
    Accent {
        accent: char,
        wide: bool,
        body: Box<Node>,
    },
    Overline(Box<Node>),
    Underline(Box<Node>),
    Array {
        kind: ArrayKind,
        rows: Vec<Vec<Node>>,
        left: Option<char>,
        right: Option<char>,
    },
    /// Explicit space in mu (1/18 em).
    Space(f64),
    /// `\displaystyle` etc: applies to the rest of the enclosing group.
    StyleChange(Style),
    /// An empty ordinary atom (`{}`), used for spacing.
    Empty,
}

#[derive(Debug, Clone, PartialEq)]
enum Tok {
    Cmd(String),
    Char(char),
    LBrace,
    RBrace,
    Sup,
    Sub,
    Amp,
    Prime,
    Tilde,
}

fn tokenize(src: &str) -> Vec<Tok> {
    let chars: Vec<char> = src.chars().collect();
    let mut toks = Vec::new();
    let mut i = 0;
    while i < chars.len() {
        let c = chars[i];
        i += 1;
        match c {
            '\\' => {
                if i >= chars.len() {
                    toks.push(Tok::Cmd(" ".into()));
                } else if chars[i].is_ascii_alphabetic() {
                    let start = i;
                    while i < chars.len() && chars[i].is_ascii_alphabetic() {
                        i += 1;
                    }
                    let mut name: String = chars[start..i].iter().collect();
                    // Starred forms (`align*`) only appear inside `\begin{…}`;
                    // `\operatorname*` is the one starred command we accept.
                    if name == "operatorname" && i < chars.len() && chars[i] == '*' {
                        name.push('*');
                        i += 1;
                    }
                    toks.push(Tok::Cmd(name));
                } else {
                    toks.push(Tok::Cmd(chars[i].to_string()));
                    i += 1;
                }
            }
            '{' => toks.push(Tok::LBrace),
            '}' => toks.push(Tok::RBrace),
            '^' => toks.push(Tok::Sup),
            '_' => toks.push(Tok::Sub),
            '&' => toks.push(Tok::Amp),
            '\'' => toks.push(Tok::Prime),
            '~' => toks.push(Tok::Tilde),
            '%' => {
                while i < chars.len() && chars[i] != '\n' {
                    i += 1;
                }
            }
            c if c.is_whitespace() => {}
            c => toks.push(Tok::Char(c)),
        }
    }
    toks
}

/// Why a row ended.
#[derive(Debug, PartialEq)]
enum Stop {
    Eof,
    Brace,
    Amp,
    Newline,
    Right(Option<char>),
    End(String),
    Bracket,
}

struct Parser {
    toks: Vec<Tok>,
    pos: usize,
    /// Raw source characters, for `\text{…}` (which keeps spaces).
    src: Vec<char>,
    /// Source char offset of each token (for `\text`).
    offsets: Vec<usize>,
    variant: Variant,
    /// Nesting depth of `\sqrt[…]` indices (where `]` ends a row).
    bracket_depth: usize,
}

/// Parse a formula. Display formulas may contain top-level `\\` (rows) and
/// `&` (alignment points), which become a `gathered`/`aligned` array.
pub fn parse(src: &str, display: bool) -> Result<Vec<Node>> {
    let toks = tokenize(src);
    let offsets = token_offsets(src);
    let mut p = Parser {
        toks,
        pos: 0,
        src: src.chars().collect(),
        offsets,
        variant: Variant::Normal,
        bracket_depth: 0,
    };
    let (mut rows, stop) = p.parse_cells()?;
    if stop != Stop::Eof {
        return Err(format!("unexpected {stop:?}"));
    }
    if rows.len() == 1 && rows[0].len() == 1 {
        return Ok(match rows.pop().and_then(|mut r| r.pop()) {
            Some(Node::Group(v)) => v,
            other => other.into_iter().collect(),
        });
    }
    if !display {
        return Err("alignment or line break outside an environment".into());
    }
    let aligned = rows.iter().any(|r| r.len() > 1);
    let kind = if aligned { ArrayKind::Aligned } else { ArrayKind::Gathered };
    Ok(vec![array(kind, rows, None, None)])
}

/// Char offset where each token starts, mirroring `tokenize`.
fn token_offsets(src: &str) -> Vec<usize> {
    let chars: Vec<char> = src.chars().collect();
    let mut offs = Vec::new();
    let mut i = 0;
    while i < chars.len() {
        let start = i;
        let c = chars[i];
        i += 1;
        match c {
            '\\' => {
                if i < chars.len() && chars[i].is_ascii_alphabetic() {
                    let s = i;
                    while i < chars.len() && chars[i].is_ascii_alphabetic() {
                        i += 1;
                    }
                    let name: String = chars[s..i].iter().collect();
                    if name == "operatorname" && i < chars.len() && chars[i] == '*' {
                        i += 1;
                    }
                } else if i < chars.len() {
                    i += 1;
                }
                offs.push(start);
            }
            '%' => {
                while i < chars.len() && chars[i] != '\n' {
                    i += 1;
                }
            }
            c if c.is_whitespace() => {}
            _ => offs.push(start),
        }
    }
    offs
}

fn array(kind: ArrayKind, mut rows: Vec<Vec<Node>>, left: Option<char>, right: Option<char>) -> Node {
    // A trailing `\\` leaves an empty last row.
    if rows.len() > 1
        && rows
            .last()
            .is_some_and(|r| r.iter().all(|c| matches!(c, Node::Group(v) if v.is_empty())))
    {
        rows.pop();
    }
    if kind == ArrayKind::Aligned {
        // amsmath starts each right-hand ("l") column with an empty group so a
        // leading relation gets its spacing: `&= x`.
        for row in rows.iter_mut() {
            for (j, cell) in row.iter_mut().enumerate() {
                if j % 2 == 1
                    && let Node::Group(v) = cell
                {
                    v.insert(0, Node::Empty);
                }
            }
        }
    }
    Node::Array { kind, rows, left, right }
}

impl Parser {
    fn peek(&self) -> Option<&Tok> {
        self.toks.get(self.pos)
    }

    fn next(&mut self) -> Option<Tok> {
        let t = self.toks.get(self.pos).cloned();
        self.pos += 1;
        t
    }

    fn expect_lbrace(&mut self) -> Result<()> {
        match self.next() {
            Some(Tok::LBrace) => Ok(()),
            other => Err(format!("expected {{, found {other:?}")),
        }
    }

    /// Read a `{name}` argument as plain text (environment names, `\text`).
    fn word_arg(&mut self) -> Result<String> {
        self.expect_lbrace()?;
        let mut s = String::new();
        loop {
            match self.next() {
                Some(Tok::RBrace) => return Ok(s),
                Some(Tok::Char(c)) => s.push(c),
                Some(Tok::Cmd(c)) if c == "*" => s.push('*'),
                other => return Err(format!("unexpected {other:?} in name")),
            }
        }
    }

    /// Read a braced argument as raw source text, keeping spaces (`\text`).
    fn raw_arg(&mut self) -> Result<String> {
        let open = self.pos;
        self.expect_lbrace()?;
        let mut depth = 1;
        while depth > 0 {
            match self.next() {
                Some(Tok::LBrace) => depth += 1,
                Some(Tok::RBrace) => depth -= 1,
                Some(_) => {}
                None => return Err("unbalanced braces".into()),
            }
        }
        let start = self.offsets[open] + 1;
        let end = self.offsets[self.pos - 1];
        let raw: String = self.src[start..end].iter().collect();
        // Un-escape the specials that are legal in text.
        let mut out = String::new();
        let mut chars = raw.chars().peekable();
        while let Some(c) = chars.next() {
            if c == '\\' {
                match chars.next() {
                    Some(n) if "{}$%&#_ ".contains(n) => out.push(n),
                    Some(',') | Some(';') | Some(':') => out.push(' '),
                    Some(n) => return Err(format!("unsupported \\{n}… in \\text")),
                    None => {}
                }
            } else if c == '~' {
                out.push(' ');
            } else if c != '{' && c != '}' {
                out.push(c);
            }
        }
        Ok(out)
    }

    /// Parse cells separated by `&` and rows separated by `\\` until a stop
    /// that isn't one of those.
    fn parse_cells(&mut self) -> Result<(Vec<Vec<Node>>, Stop)> {
        let mut rows = vec![Vec::new()];
        loop {
            let (nodes, stop) = self.parse_row()?;
            rows.last_mut().unwrap().push(Node::Group(nodes));
            match stop {
                Stop::Amp => {}
                Stop::Newline => rows.push(Vec::new()),
                other => return Ok((rows, other)),
            }
        }
    }

    /// Parse atoms until a delimiter ends the row.
    fn parse_row(&mut self) -> Result<(Vec<Node>, Stop)> {
        let mut nodes: Vec<Node> = Vec::new();
        loop {
            let Some(tok) = self.peek().cloned() else {
                return Ok((nodes, Stop::Eof));
            };
            match tok {
                Tok::RBrace => {
                    self.pos += 1;
                    return Ok((nodes, Stop::Brace));
                }
                Tok::Amp => {
                    self.pos += 1;
                    return Ok((nodes, Stop::Amp));
                }
                Tok::Char(']') if self.in_bracket() => {
                    self.pos += 1;
                    return Ok((nodes, Stop::Bracket));
                }
                Tok::Cmd(ref c) if c == "\\" || c == "cr" => {
                    self.pos += 1;
                    // Optional spacing argument: `\\[2pt]`.
                    if self.peek() == Some(&Tok::Char('[')) {
                        while let Some(t) = self.next() {
                            if t == Tok::Char(']') {
                                break;
                            }
                        }
                    }
                    return Ok((nodes, Stop::Newline));
                }
                Tok::Cmd(ref c) if c == "right" => {
                    self.pos += 1;
                    let d = self.delimiter()?;
                    return Ok((nodes, Stop::Right(d)));
                }
                Tok::Cmd(ref c) if c == "end" => {
                    self.pos += 1;
                    let name = self.word_arg()?;
                    return Ok((nodes, Stop::End(name)));
                }
                Tok::Sup | Tok::Sub => {
                    self.pos += 1;
                    let arg = self.script_arg()?;
                    let base = nodes.pop().unwrap_or(Node::Empty);
                    nodes.push(attach(base, arg, tok == Tok::Sup)?);
                }
                Tok::Prime => {
                    self.pos += 1;
                    let mut primes = 1;
                    while self.peek() == Some(&Tok::Prime) {
                        self.pos += 1;
                        primes += 1;
                    }
                    let ch = match primes {
                        1 => '\u{2032}',
                        2 => '\u{2033}',
                        _ => '\u{2034}',
                    };
                    let base = nodes.pop().unwrap_or(Node::Empty);
                    let prime = Node::Sym { ch, class: Class::Ord };
                    nodes.push(attach_prime(base, prime));
                }
                Tok::Cmd(ref c) if c == "middle" => {
                    self.pos += 1;
                    let d = self.delimiter()?;
                    nodes.push(Node::Middle(d));
                }
                Tok::Cmd(ref c) if c == "limits" || c == "nolimits" => {
                    self.pos += 1;
                    let want = c == "limits";
                    match nodes.last_mut() {
                        Some(Node::Op { limits, .. } | Node::NamedOp { limits, .. }) => *limits = Some(want),
                        _ => return Err(format!("\\{c} must follow an operator")),
                    }
                }
                _ => {
                    let node = self.atom()?;
                    nodes.push(node);
                }
            }
        }
    }

    /// Whether we're parsing a `\sqrt[…]` index (so `]` ends the row).
    fn in_bracket(&self) -> bool {
        self.bracket_depth > 0
    }

    fn script_arg(&mut self) -> Result<Node> {
        match self.peek() {
            Some(Tok::LBrace) => {
                self.pos += 1;
                let (nodes, stop) = self.parse_row()?;
                if stop != Stop::Brace {
                    return Err("unclosed group in script".into());
                }
                Ok(group(nodes))
            }
            Some(Tok::Sup | Tok::Sub) | None => Err("missing script argument".into()),
            _ => self.atom(),
        }
    }

    /// A macro argument: a braced group or a single token.
    fn arg(&mut self) -> Result<Node> {
        match self.peek() {
            Some(Tok::LBrace) => {
                self.pos += 1;
                let (nodes, stop) = self.parse_row()?;
                if stop != Stop::Brace {
                    return Err("unclosed group".into());
                }
                Ok(group(nodes))
            }
            None => Err("missing argument".into()),
            _ => self.atom(),
        }
    }

    /// An argument parsed in another math alphabet.
    fn arg_in(&mut self, v: Variant) -> Result<Node> {
        let saved = self.variant;
        self.variant = v;
        let r = self.arg();
        self.variant = saved;
        r
    }

    fn delimiter(&mut self) -> Result<Option<char>> {
        match self.next() {
            Some(Tok::Char('.')) => Ok(None),
            Some(Tok::Char(c)) => symbols::delimiter_char(c)
                .map(Some)
                .ok_or_else(|| format!("bad delimiter {c}")),
            Some(Tok::Cmd(c)) => symbols::delimiter_cmd(&c)
                .map(Some)
                .ok_or_else(|| format!("bad delimiter \\{c}")),
            other => Err(format!("bad delimiter {other:?}")),
        }
    }

    fn sym(&self, ch: char, class: Class) -> Node {
        Node::Sym {
            ch: symbols::map_char(ch, self.variant),
            class,
        }
    }

    fn atom(&mut self) -> Result<Node> {
        let tok = self.next().ok_or("unexpected end")?;
        match tok {
            Tok::Char(c) => {
                let (ch, class) = symbols::char_class(c);
                Ok(self.sym(ch, class))
            }
            Tok::LBrace => {
                let (nodes, stop) = self.parse_row()?;
                if stop != Stop::Brace {
                    return Err("unclosed group".into());
                }
                Ok(Node::Group(nodes))
            }
            Tok::Tilde => Ok(Node::Space(6.0)),
            Tok::Cmd(name) => self.command(&name),
            other => Err(format!("unexpected {other:?}")),
        }
    }

    fn command(&mut self, name: &str) -> Result<Node> {
        if let Some((ch, class)) = symbols::symbol(name) {
            // Greek letters follow the active alphabet; other symbols don't.
            let is_greek = ('\u{0391}'..='\u{03F5}').contains(&ch);
            let ch = if is_greek { symbols::map_char(ch, self.variant) } else { ch };
            return Ok(Node::Sym { ch, class });
        }
        if let Some((ch, default_limits)) = symbols::big_op(name) {
            return Ok(Node::Op {
                ch,
                default_limits,
                limits: None,
            });
        }
        if let Some((text, default_limits)) = symbols::named_op(name) {
            return Ok(Node::NamedOp {
                text: text.into(),
                default_limits,
                limits: None,
            });
        }
        if let Some((accent, wide)) = symbols::accent(name) {
            let body = self.arg()?;
            return Ok(Node::Accent {
                accent,
                wide,
                body: Box::new(body),
            });
        }
        let space = |mu: f64| Ok(Node::Space(mu));
        match name {
            "frac" | "dfrac" | "tfrac" | "cfrac" => {
                let num = self.arg()?;
                let den = self.arg()?;
                Ok(Node::Frac {
                    num: Box::new(num),
                    den: Box::new(den),
                    rule: true,
                    style: match name {
                        "dfrac" | "cfrac" => Some(Style::Display),
                        "tfrac" => Some(Style::Text),
                        _ => None,
                    },
                    delims: None,
                })
            }
            "binom" | "dbinom" | "tbinom" => {
                let num = self.arg()?;
                let den = self.arg()?;
                Ok(Node::Frac {
                    num: Box::new(num),
                    den: Box::new(den),
                    rule: false,
                    style: match name {
                        "dbinom" => Some(Style::Display),
                        "tbinom" => Some(Style::Text),
                        _ => None,
                    },
                    delims: Some(('(', ')')),
                })
            }
            "sqrt" => {
                let index = if self.peek() == Some(&Tok::Char('[')) {
                    self.pos += 1;
                    self.bracket_depth += 1;
                    let r = self.parse_row();
                    self.bracket_depth -= 1;
                    let (nodes, stop) = r?;
                    if stop != Stop::Bracket {
                        return Err("unclosed \\sqrt[".into());
                    }
                    Some(Box::new(group(nodes)))
                } else {
                    None
                };
                let body = self.arg()?;
                Ok(Node::Sqrt {
                    index,
                    body: Box::new(body),
                })
            }
            "left" => {
                let left = self.delimiter()?;
                let (body, stop) = self.parse_row()?;
                match stop {
                    Stop::Right(right) => Ok(Node::LeftRight { left, right, body }),
                    _ => Err("\\left without \\right".into()),
                }
            }
            "big" | "Big" | "bigg" | "Bigg" | "bigl" | "Bigl" | "biggl" | "Biggl" | "bigr" | "Bigr" | "biggr"
            | "Biggr" | "bigm" | "Bigm" | "biggm" | "Biggm" => {
                let step = match name.trim_end_matches(['l', 'r', 'm']) {
                    "big" => 1,
                    "Big" => 2,
                    "bigg" => 3,
                    _ => 4,
                };
                let class = match name.chars().last() {
                    Some('l') => Class::Open,
                    Some('r') => Class::Close,
                    Some('m') => Class::Rel,
                    _ => Class::Ord,
                };
                let ch = self.delimiter()?;
                Ok(Node::BigDelim { ch, step, class })
            }
            "overline" => Ok(Node::Overline(Box::new(self.arg()?))),
            "underline" => Ok(Node::Underline(Box::new(self.arg()?))),
            "mathbf" => self.arg_in(Variant::Bold),
            "boldsymbol" | "bm" => self.arg_in(Variant::BoldItalic),
            "mathit" => self.arg_in(Variant::Normal),
            "mathrm" | "mathup" => self.arg_in(Variant::Upright),
            "mathbb" => self.arg_in(Variant::DoubleStruck),
            "mathcal" | "mathscr" => self.arg_in(Variant::Script),
            "mathfrak" => self.arg_in(Variant::Fraktur),
            "mathsf" => self.arg_in(Variant::SansSerif),
            "mathtt" => self.arg_in(Variant::Monospace),
            "text" | "textrm" | "textnormal" | "mbox" | "textup" | "textit" => Ok(Node::Text {
                text: self.raw_arg()?,
                bold: false,
            }),
            "textbf" => Ok(Node::Text {
                text: self.raw_arg()?,
                bold: true,
            }),
            "operatorname" | "operatorname*" => Ok(Node::NamedOp {
                text: self.raw_arg()?,
                default_limits: name.ends_with('*'),
                limits: None,
            }),
            "," | "thinspace" => space(3.0),
            ":" | ">" | "medspace" => space(4.0),
            ";" | "thickspace" => space(5.0),
            "!" | "negthinspace" => space(-3.0),
            " " => space(6.0),
            "quad" => space(18.0),
            "qquad" => space(36.0),
            "displaystyle" => Ok(Node::StyleChange(Style::Display)),
            "textstyle" => Ok(Node::StyleChange(Style::Text)),
            "scriptstyle" => Ok(Node::StyleChange(Style::Script)),
            "scriptscriptstyle" => Ok(Node::StyleChange(Style::ScriptScript)),
            "not" => {
                let negated = match self.next() {
                    Some(Tok::Char('=')) => '\u{2260}',
                    Some(Tok::Char('<')) => '\u{226E}',
                    Some(Tok::Char('>')) => '\u{226F}',
                    Some(Tok::Cmd(c)) => match c.as_str() {
                        "in" => '\u{2209}',
                        "equiv" => '\u{2262}',
                        "subset" => '\u{2284}',
                        "supset" => '\u{2285}',
                        "subseteq" => '\u{2288}',
                        "supseteq" => '\u{2289}',
                        "sim" => '\u{2241}',
                        "approx" => '\u{2249}',
                        "le" | "leq" => '\u{2270}',
                        "ge" | "geq" => '\u{2271}',
                        "mid" => '\u{2224}',
                        "parallel" => '\u{2226}',
                        _ => return Err(format!("unsupported \\not\\{c}")),
                    },
                    other => return Err(format!("unsupported \\not {other:?}")),
                };
                Ok(Node::Sym {
                    ch: negated,
                    class: Class::Rel,
                })
            }
            "label" | "tag" => {
                self.raw_arg()?;
                Ok(Node::Group(Vec::new()))
            }
            "nonumber" | "notag" => Ok(Node::Group(Vec::new())),
            "begin" => self.environment(),
            _ => Err(format!("unsupported command \\{name}")),
        }
    }

    fn environment(&mut self) -> Result<Node> {
        let name = self.word_arg()?;
        let (kind, left, right) = match name.as_str() {
            "matrix" => (ArrayKind::Matrix { small: false }, None, None),
            "smallmatrix" => (ArrayKind::Matrix { small: true }, None, None),
            "pmatrix" => (ArrayKind::Matrix { small: false }, Some('('), Some(')')),
            "bmatrix" => (ArrayKind::Matrix { small: false }, Some('['), Some(']')),
            "Bmatrix" => (ArrayKind::Matrix { small: false }, Some('{'), Some('}')),
            "vmatrix" => (ArrayKind::Matrix { small: false }, Some('|'), Some('|')),
            "Vmatrix" => (ArrayKind::Matrix { small: false }, Some('\u{2016}'), Some('\u{2016}')),
            "cases" | "dcases" => (ArrayKind::Cases, Some('{'), None),
            "rcases" => (ArrayKind::Cases, None, Some('}')),
            "aligned" | "align" | "align*" | "alignat" | "alignat*" | "alignedat" | "split" | "flalign"
            | "flalign*" | "eqnarray" | "eqnarray*" => {
                if name.starts_with("alignat") || name == "alignedat" {
                    self.word_arg()?; // column-pair count
                }
                (ArrayKind::Aligned, None, None)
            }
            "gathered" | "gather" | "gather*" | "multline" | "multline*" | "equation" | "equation*" => {
                (ArrayKind::Gathered, None, None)
            }
            "array" => {
                let spec = self.word_arg()?;
                let cols = spec
                    .chars()
                    .filter_map(|c| match c {
                        'l' => Some(Align::Left),
                        'c' => Some(Align::Center),
                        'r' => Some(Align::Right),
                        _ => None,
                    })
                    .collect();
                (ArrayKind::Array(cols), None, None)
            }
            _ => return Err(format!("unsupported environment {name}")),
        };
        let (rows, stop) = self.parse_cells()?;
        match stop {
            Stop::End(ref end) if *end == name => {}
            other => return Err(format!("{name} closed by {other:?}")),
        }
        Ok(array(kind, rows, left, right))
    }
}

/// A group of one node is just that node.
fn group(mut nodes: Vec<Node>) -> Node {
    if nodes.len() == 1 {
        nodes.pop().unwrap()
    } else {
        Node::Group(nodes)
    }
}

fn attach(base: Node, arg: Node, sup: bool) -> Result<Node> {
    match base {
        Node::Scripts {
            base,
            sub,
            sup: existing,
        } => {
            if sup && existing.is_none() {
                Ok(Node::Scripts {
                    base,
                    sub,
                    sup: Some(Box::new(arg)),
                })
            } else if !sup && sub.is_none() {
                Ok(Node::Scripts {
                    base,
                    sub: Some(Box::new(arg)),
                    sup: existing,
                })
            } else {
                Err("double script".into())
            }
        }
        base => {
            let arg = Some(Box::new(arg));
            let (sub, sup) = if sup { (None, arg) } else { (arg, None) };
            Ok(Node::Scripts {
                base: Box::new(base),
                sub,
                sup,
            })
        }
    }
}

/// `x'`: in an OpenType math font the prime glyph is already drawn raised, so
/// it follows the base at full size (TeX instead superscripts a lowered
/// `\prime`). Later scripts attach to base and prime together.
fn attach_prime(base: Node, prime: Node) -> Node {
    match base {
        Node::Group(mut v) if matches!(v.last(), Some(Node::Sym { ch: '\u{2032}'..='\u{2034}', .. })) => {
            v.push(prime);
            Node::Group(v)
        }
        base => Node::Group(vec![base, prime]),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_scripts_and_fractions() {
        let n = parse(r"x^2 + \frac{a}{b}", false).unwrap();
        assert_eq!(n.len(), 3);
        assert!(matches!(n[0], Node::Scripts { .. }));
        assert!(matches!(n[2], Node::Frac { rule: true, .. }));
    }

    #[test]
    fn parses_environments() {
        let n = parse(r"\begin{pmatrix} a & b \\ c & d \end{pmatrix}", true).unwrap();
        match &n[0] {
            Node::Array { rows, left, .. } => {
                assert_eq!(rows.len(), 2);
                assert_eq!(rows[0].len(), 2);
                assert_eq!(*left, Some('('));
            }
            other => panic!("expected array, got {other:?}"),
        }
        let n = parse(r"\begin{aligned} a &= b \\ c &= d \\ \end{aligned}", true).unwrap();
        match &n[0] {
            Node::Array { rows, kind, .. } => {
                assert_eq!(*kind, ArrayKind::Aligned);
                assert_eq!(rows.len(), 2, "trailing \\\\ is dropped");
            }
            other => panic!("expected array, got {other:?}"),
        }
    }

    #[test]
    fn top_level_rows_in_display_become_an_array() {
        let n = parse(r"a = b \\ c = d", true).unwrap();
        assert!(matches!(&n[0], Node::Array { kind: ArrayKind::Gathered, .. }));
        assert!(parse(r"a \\ b", false).is_err());
    }

    #[test]
    fn text_keeps_spaces() {
        let n = parse(r"x \text{ if } y", false).unwrap();
        assert!(n.iter().any(|n| matches!(n, Node::Text { text, .. } if text == " if ")));
    }

    #[test]
    fn unknown_commands_are_errors() {
        assert!(parse(r"\foo{x}", false).is_err());
        assert!(parse(r"\frac{a}{b", false).is_err());
        assert!(parse(r"\left( x", false).is_err());
    }

    #[test]
    fn parses_radical_index_and_null_delimiters() {
        let n = parse(r"\sqrt[3]{x}", false).unwrap();
        assert!(matches!(&n[0], Node::Sqrt { index: Some(_), .. }));
        let n = parse(r"\sqrt{x}", false).unwrap();
        assert!(matches!(&n[0], Node::Sqrt { index: None, .. }));
        let n = parse(r"\left. x \right|", false).unwrap();
        assert!(matches!(&n[0], Node::LeftRight { left: None, right: Some('|'), .. }));
        let n = parse(r"\left\{ x \middle| y \right\}", false).unwrap();
        match &n[0] {
            Node::LeftRight { body, .. } => assert!(body.contains(&Node::Middle(Some('|')))),
            other => panic!("expected \\left…\\right, got {other:?}"),
        }
    }

    #[test]
    fn parses_negations_and_operator_names() {
        let n = parse(r"x \not\in A", false).unwrap();
        assert_eq!(n[1], Node::Sym { ch: '\u{2209}', class: Class::Rel });
        let n = parse(r"\operatorname{tr} A", false).unwrap();
        assert!(matches!(&n[0], Node::NamedOp { text, default_limits: false, .. } if text == "tr"));
        let n = parse(r"\operatorname*{argmax}_x", false).unwrap();
        match &n[0] {
            Node::Scripts { base, .. } => {
                assert!(matches!(base.as_ref(), Node::NamedOp { default_limits: true, .. }))
            }
            other => panic!("expected scripts, got {other:?}"),
        }
        assert!(parse(r"a \not\foo b", false).is_err());
    }

    #[test]
    fn rejects_double_scripts_and_stray_alignment() {
        assert!(parse("x^a^b", false).is_err());
        assert!(parse("x_a_b", false).is_err());
        assert!(parse("x_a^b", false).is_ok());
        assert!(parse("a & b", false).is_err(), "& needs an environment inline");
        assert!(parse("a &= b", true).is_ok(), "display math aligns at &");
        assert!(parse(r"\right)", false).is_err());
        assert!(parse(r"\begin{pmatrix} a \end{bmatrix}", false).is_err());
    }

    #[test]
    fn primes_and_scripts_combine() {
        let n = parse("x'^2", false).unwrap();
        match &n[0] {
            Node::Scripts { base, sup: Some(_), sub: None } => match base.as_ref() {
                Node::Group(v) => {
                    assert_eq!(v.len(), 2);
                    assert_eq!(v[1], Node::Sym { ch: '\u{2032}', class: Class::Ord });
                }
                other => panic!("expected x and a prime, got {other:?}"),
            },
            other => panic!("expected one superscript, got {other:?}"),
        }
        let n = parse("f''", false).unwrap();
        assert!(matches!(&n[0], Node::Group(v) if v[1] == Node::Sym { ch: '\u{2033}', class: Class::Ord }));
    }

    #[test]
    fn aligned_right_columns_start_with_an_empty_atom() {
        let n = parse(r"\begin{aligned} a &= b \end{aligned}", true).unwrap();
        let Node::Array { rows, .. } = &n[0] else {
            panic!("expected array")
        };
        assert!(matches!(&rows[0][1], Node::Group(v) if v.first() == Some(&Node::Empty)));
        assert!(matches!(&rows[0][0], Node::Group(v) if v.first() != Some(&Node::Empty)));
    }

    #[test]
    fn variants_map_letters() {
        let n = parse(r"\mathbf{E} \mathbb{R} x", false).unwrap();
        assert_eq!(n[0], Node::Sym { ch: '\u{1D404}', class: Class::Ord });
        assert_eq!(n[1], Node::Sym { ch: '\u{211D}', class: Class::Ord });
        assert_eq!(n[2], Node::Sym { ch: '\u{1D465}', class: Class::Ord });
    }
}
