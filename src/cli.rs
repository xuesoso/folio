//! Hand-rolled command-line parsing (avoids a `clap` dependency).

use std::path::PathBuf;

use crate::Result;
use crate::app::state::{Sharpness, Theme, ThemeSetting};

const USAGE: &str = "\
Usage: folio <FILE> [options]
       folio gen-config

A terminal document viewer that renders pages via sixel or kitty graphics
protocols. FILE may be a PDF, a Markdown (.md) or LaTeX (.tex, opt-in) file, or a
static image (.png, .jpg, .jpeg, .gif, .webp, .bmp, .tif, .tiff).

Commands:
  gen-config                        Write a default config file to the user config
                                    dir (~/.config/folio/config.toml);
                                    does nothing if it already exists
  print <FILE> [geometry]           Render one page into a terminal cell box and
                                    print the graphics escapes to stdout, then exit
                                    (headless — for embedding a preview in another
                                    TUI). Flags: --protocol kitty|sixel|iterm, --col,
                                    --row, --cols, --rows, --cell-width,
                                    --cell-height, --tmux, --page,
                                    --theme off|nord|auto (auto: nord
                                    when --bg is dark, else off),
                                    --sharpness low|medium|high|full,
                                    --bg RRGGBB|none (fill the cell box with this
                                    background; transparent images flatten onto it),
                                    --image-id N (kitty image id, 1..16777215;
                                    default $FOLIO_IMAGE_ID, else a fixed id)

Options:
  --backend <auto|kitty|sixel>      Force a display protocol (default: auto-detect)
  --theme <off|nord|auto>           Color theme / dark mode (auto: nord when the
                                    terminal background is dark, else off;
                                    default: remembered per file, else off)
  --sharpness <low|medium|high|full>  Display sharpness vs. speed (kitty; default:
                                    remembered per file, else medium)
  -h, --help                        Print help
  -V, --version                     Print version

Navigation (once a document is open):
  j / k  (s / w)  scroll down / up (pages flip at top & bottom edges)
  h / l  (a / d)  pan left / right        i       dark mode (nord)
  n / Space / p   next / prev page        g / G   first / last page
  + / -           zoom in / out           = / 0   fit-page    _  fit-width
  H / L           rotate ccw / cw (90°)   [ / ]   sharpness softer / sharper
  m               toggle bookmark         '       bookmark list
  q / Esc         quit (resumes here next time)";

/// Which display protocol to use.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BackendChoice {
    /// Detect kitty vs sixel automatically.
    Auto,
    Kitty,
    Sixel,
}

/// Parsed command-line arguments for opening a document.
#[derive(Debug)]
pub struct Args {
    pub path: PathBuf,
    pub backend: BackendChoice,
    /// Sharpness override; `None` means use the per-file/default value.
    pub sharpness: Option<Sharpness>,
    /// Theme override; `None` means use the per-file/default value.
    pub theme: Option<ThemeSetting>,
}

/// A parsed top-level invocation.
#[derive(Debug)]
pub enum Command {
    /// Open a document in the viewer.
    Open(Args),
    /// Write a default config file if one does not already exist.
    GenConfig,
    /// Render one page into a cell box and print the graphics escapes (headless).
    Print(PrintArgs),
}

/// Arguments for the headless `print` subcommand: render one page into a given
/// terminal cell box and emit the graphics escapes to stdout, then exit. Used to
/// embed a preview inside another TUI (e.g. rustranger's preview pane).
/// Display protocol for the headless `print` subcommand. Unlike the interactive
/// viewer (which auto-detects kitty/sixel), the caller chooses explicitly — and
/// may pick the iTerm2 inline-images protocol, which the viewer doesn't use.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PrintProtocol {
    Kitty,
    Sixel,
    Iterm,
}

#[derive(Debug)]
pub struct PrintArgs {
    pub path: PathBuf,
    pub protocol: PrintProtocol,
    /// Target cell box: 0-based top-left cell (col,row) and size (cols,rows).
    pub col: u16,
    pub row: u16,
    pub cols: u16,
    pub rows: u16,
    /// Cell size in pixels, so the page is rasterized at the right resolution.
    pub cell_w: u16,
    pub cell_h: u16,
    /// Wrap the escapes for tmux passthrough.
    pub tmux: bool,
    /// 0-based page index (default 0).
    pub page: usize,
    /// Color theme (off/nord) applied to the rendered page.
    pub theme: Theme,
    /// Render sharpness (resolution vs. speed).
    pub sharpness: Sharpness,
    /// Background color (RGB): transparent images are flattened onto it and the
    /// render is padded to fill the whole cell box with it, so the embedding
    /// UI's theme background shows instead of the terminal's. None = white
    /// flatten, no padding (the aspect-fit image only).
    pub bg: Option<[u8; 3]>,
    /// Kitty image id to transmit under. Embedders that can share one outer
    /// terminal (several panes in tmux) must each pass their own, or every
    /// pane's placeholders point at whichever image was transmitted last.
    pub image_id: u32,
}

/// Parse `std::env::args`. Exits the process directly for `--help` / `--version`.
pub fn parse() -> Result<Command> {
    let mut args = std::env::args().skip(1).peekable();

    // `gen-config` is a subcommand and must be the first argument.
    if args.peek().map(String::as_str) == Some("gen-config") {
        args.next();
        if let Some(extra) = args.next() {
            anyhow::bail!("gen-config takes no further arguments (got '{extra}')\n\n{USAGE}");
        }
        return Ok(Command::GenConfig);
    }

    // `print` is a headless subcommand and must be the first argument.
    if args.peek().map(String::as_str) == Some("print") {
        args.next();
        return parse_print(args, std::env::var("FOLIO_IMAGE_ID").ok());
    }

    let mut path: Option<PathBuf> = None;
    let mut backend = BackendChoice::Auto;
    let mut sharpness: Option<Sharpness> = None;
    let mut theme: Option<ThemeSetting> = None;

    while let Some(arg) = args.next() {
        match arg.as_str() {
            "-h" | "--help" => {
                println!("{USAGE}");
                std::process::exit(0);
            }
            "-V" | "--version" => {
                println!("folio {}", env!("CARGO_PKG_VERSION"));
                std::process::exit(0);
            }
            "--backend" => {
                let value = args
                    .next()
                    .ok_or_else(|| anyhow::anyhow!("--backend requires a value (auto|kitty|sixel)"))?;
                backend = match value.as_str() {
                    "auto" => BackendChoice::Auto,
                    "kitty" => BackendChoice::Kitty,
                    "sixel" => BackendChoice::Sixel,
                    other => anyhow::bail!("unknown backend '{other}' (expected auto|kitty|sixel)"),
                };
            }
            "--theme" => {
                let value = args
                    .next()
                    .ok_or_else(|| anyhow::anyhow!("--theme requires a value (off|nord|auto)"))?;
                theme = Some(ThemeSetting::parse(&value).ok_or_else(|| {
                    anyhow::anyhow!("unknown theme '{value}' (expected off|nord|auto)")
                })?);
            }
            "--sharpness" => {
                let value = args
                    .next()
                    .ok_or_else(|| anyhow::anyhow!("--sharpness requires a value (low|medium|high|full)"))?;
                sharpness = Some(Sharpness::parse(&value).ok_or_else(|| {
                    anyhow::anyhow!("unknown sharpness '{value}' (expected low|medium|high|full)")
                })?);
            }
            other if other.starts_with('-') => {
                anyhow::bail!("unknown option '{other}'\n\n{USAGE}");
            }
            other => {
                if path.is_some() {
                    anyhow::bail!("unexpected extra argument '{other}'\n\n{USAGE}");
                }
                path = Some(PathBuf::from(other));
            }
        }
    }

    let path = path.ok_or_else(|| anyhow::anyhow!("missing file argument\n\n{USAGE}"))?;
    Ok(Command::Open(Args {
        path,
        backend,
        sharpness,
        theme,
    }))
}

/// Parse the flags for the `print` subcommand (see [`PrintArgs`]). `env_id` is
/// `$FOLIO_IMAGE_ID`, the image id used when `--image-id` isn't given — an
/// embedder can set it once without having to change every user's command line.
fn parse_print(mut args: impl Iterator<Item = String>, env_id: Option<String>) -> Result<Command> {
    fn u16v(v: Option<String>, name: &str) -> Result<u16> {
        v.ok_or_else(|| anyhow::anyhow!("{name} requires a value"))?
            .parse()
            .map_err(|e| anyhow::anyhow!("{name}: {e}"))
    }

    let mut path: Option<PathBuf> = None;
    let mut protocol = PrintProtocol::Kitty;
    let (mut col, mut row, mut cols, mut rows) = (0u16, 0u16, 40u16, 20u16);
    let (mut cell_w, mut cell_h) = (10u16, 20u16);
    let mut tmux = false;
    let mut page = 0usize;
    let mut theme = Theme::Off;
    // `--theme auto` resolves after parsing (the --bg fill may follow it).
    let mut theme_auto = false;
    let mut sharpness = Sharpness::Full;
    let mut bg: Option<[u8; 3]> = None;
    let mut image_id = match env_id.as_deref().map(str::trim).filter(|v| !v.is_empty()) {
        Some(v) => parse_image_id(v).map_err(|e| anyhow::anyhow!("FOLIO_IMAGE_ID: {e}"))?,
        None => crate::render::IMAGE_ID,
    };

    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--image-id" => {
                let v = args.next().ok_or_else(|| anyhow::anyhow!("--image-id requires a value"))?;
                image_id = parse_image_id(&v).map_err(|e| anyhow::anyhow!("--image-id: {e}"))?;
            }
            "--bg" => {
                let v = args
                    .next()
                    .ok_or_else(|| anyhow::anyhow!("--bg requires a value (RRGGBB or none)"))?;
                bg = parse_bg(&v)?;
            }
            "--theme" => {
                let value = args.next();
                match value.as_deref().map(str::trim) {
                    Some("auto") => theme_auto = true,
                    other => {
                        theme_auto = false;
                        theme = Theme::parse(other.unwrap_or_default())
                            .ok_or_else(|| anyhow::anyhow!("--theme expects off|nord|auto"))?;
                    }
                }
            }
            "--sharpness" => {
                sharpness = match args.next().as_deref().map(Sharpness::parse) {
                    Some(Some(s)) => s,
                    _ => anyhow::bail!("--sharpness expects low|medium|high|full"),
                }
            }
            "--protocol" => {
                protocol = match args.next().as_deref() {
                    Some("kitty") => PrintProtocol::Kitty,
                    Some("sixel") => PrintProtocol::Sixel,
                    Some("iterm" | "iterm2") => PrintProtocol::Iterm,
                    other => anyhow::bail!("--protocol expects kitty|sixel|iterm (got {other:?})"),
                }
            }
            "--col" => col = u16v(args.next(), "--col")?,
            "--row" => row = u16v(args.next(), "--row")?,
            "--cols" => cols = u16v(args.next(), "--cols")?,
            "--rows" => rows = u16v(args.next(), "--rows")?,
            "--cell-width" => cell_w = u16v(args.next(), "--cell-width")?,
            "--cell-height" => cell_h = u16v(args.next(), "--cell-height")?,
            "--page" => {
                page = args
                    .next()
                    .ok_or_else(|| anyhow::anyhow!("--page requires a value"))?
                    .parse()
                    .map_err(|e| anyhow::anyhow!("--page: {e}"))?
            }
            "--tmux" => tmux = true,
            "-h" | "--help" => {
                println!("{USAGE}");
                std::process::exit(0);
            }
            other if other.starts_with('-') => anyhow::bail!("unknown print option '{other}'"),
            other => {
                if path.is_some() {
                    anyhow::bail!("print: unexpected extra argument '{other}'");
                }
                path = Some(PathBuf::from(other));
            }
        }
    }

    let path = path.ok_or_else(|| anyhow::anyhow!("print: missing file argument"))?;
    // `--theme auto` can only resolve once parsing is done: Nord dark mode
    // when the --bg box fill is dark, else the original page colours.
    let theme = if theme_auto { Theme::for_background(bg) } else { theme };
    Ok(Command::Print(PrintArgs {
        path,
        protocol,
        col,
        row,
        cols: cols.max(1),
        rows: rows.max(1),
        cell_w: cell_w.max(1),
        cell_h: cell_h.max(1),
        tmux,
        page,
        theme,
        sharpness,
        bg,
        image_id,
    }))
}

/// Parse a kitty image id. Zero means "no id" to the protocol, and the tmux path
/// encodes the id in a 24-bit foreground color, so only 1..=0xFFFFFF works.
fn parse_image_id(v: &str) -> Result<u32> {
    let id: u32 = v.trim().parse().map_err(|e| anyhow::anyhow!("{e} (got '{v}')"))?;
    anyhow::ensure!((1..=0xFF_FFFF).contains(&id), "expected 1..=16777215 (got {id})");
    Ok(id)
}

/// Parse a `--bg` value: `RRGGBB` (optionally `#`-prefixed) or `none`.
fn parse_bg(v: &str) -> Result<Option<[u8; 3]>> {
    let v = v.trim();
    if v.eq_ignore_ascii_case("none") || v.is_empty() {
        return Ok(None);
    }
    let hex = v.trim_start_matches('#');
    anyhow::ensure!(
        hex.len() == 6 && hex.bytes().all(|b| b.is_ascii_hexdigit()),
        "--bg expects RRGGBB or none (got '{v}')"
    );
    let n = u32::from_str_radix(hex, 16).unwrap();
    Ok(Some([(n >> 16) as u8, (n >> 8) as u8, n as u8]))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn print_args(argv: &[&str], env_id: Option<&str>) -> Result<PrintArgs> {
        let args = argv.iter().map(|s| s.to_string());
        match parse_print(args, env_id.map(str::to_string))? {
            Command::Print(p) => Ok(p),
            _ => unreachable!("parse_print only returns Print"),
        }
    }

    #[test]
    fn image_id_defaults_to_the_fixed_id() {
        assert_eq!(print_args(&["a.pdf"], None).unwrap().image_id, crate::render::IMAGE_ID);
        // An empty variable is "unset", not an error.
        assert_eq!(print_args(&["a.pdf"], Some("")).unwrap().image_id, crate::render::IMAGE_ID);
    }

    #[test]
    fn image_id_comes_from_env_and_the_flag_wins() {
        assert_eq!(print_args(&["a.pdf"], Some("4242")).unwrap().image_id, 4242);
        let p = print_args(&["--image-id", "77", "a.pdf"], Some("4242")).unwrap();
        assert_eq!(p.image_id, 77);
    }

    #[test]
    fn image_id_must_fit_a_placeholder_color() {
        assert!(print_args(&["--image-id", "0", "a.pdf"], None).is_err());
        assert!(print_args(&["--image-id", "16777216", "a.pdf"], None).is_err());
        assert!(print_args(&["--image-id", "x", "a.pdf"], None).is_err());
        assert!(print_args(&["--image-id"], None).is_err());
        assert!(print_args(&["a.pdf"], Some("0")).is_err());
        assert_eq!(print_args(&["--image-id", "16777215", "a.pdf"], None).unwrap().image_id, 0xFF_FFFF);
    }

    #[test]
    fn theme_auto_follows_the_box_fill() {
        // Dark fills render in Nord; light fills, `none`, and a missing --bg
        // (terminal default) keep the original page colours.
        let p = print_args(&["--theme", "auto", "--bg", "282828", "a.pdf"], None).unwrap();
        assert_eq!(p.theme, Theme::Nord);
        for bg in ["fafafa", "none"] {
            let p = print_args(&["--theme", "auto", "--bg", bg, "a.pdf"], None).unwrap();
            assert_eq!(p.theme, Theme::Off);
        }
        assert_eq!(print_args(&["--theme", "auto", "a.pdf"], None).unwrap().theme, Theme::Off);

        // Resolution happens after parsing, so --bg may follow --theme.
        let p = print_args(&["--bg", "002b36", "--theme", "auto", "a.pdf"], None).unwrap();
        assert_eq!(p.theme, Theme::Nord);

        // The last --theme flag wins; explicit themes still parse.
        assert_eq!(print_args(&["--theme", "nord", "--theme", "auto", "a.pdf"], None).unwrap().theme, Theme::Off);
        assert_eq!(print_args(&["--theme", "auto", "--theme", "nord", "a.pdf"], None).unwrap().theme, Theme::Nord);
        assert_eq!(print_args(&["a.pdf"], None).unwrap().theme, Theme::Off);
        assert!(print_args(&["--theme", "solarized", "a.pdf"], None).is_err());
    }
}
