//! Hand-rolled command-line parsing (avoids a `clap` dependency).

use std::path::PathBuf;

use crate::Result;
use crate::app::state::{Sharpness, Theme};

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

Options:
  --backend <auto|kitty|sixel>      Force a display protocol (default: auto-detect)
  --theme <off|nord>                Color theme / dark mode (default: remembered
                                    per file, else off)
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
    pub theme: Option<Theme>,
}

/// A parsed top-level invocation.
#[derive(Debug)]
pub enum Command {
    /// Open a document in the viewer.
    Open(Args),
    /// Write a default config file if one does not already exist.
    GenConfig,
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

    let mut path: Option<PathBuf> = None;
    let mut backend = BackendChoice::Auto;
    let mut sharpness: Option<Sharpness> = None;
    let mut theme: Option<Theme> = None;

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
                    .ok_or_else(|| anyhow::anyhow!("--theme requires a value (off|nord)"))?;
                theme = Some(Theme::parse(&value).ok_or_else(|| {
                    anyhow::anyhow!("unknown theme '{value}' (expected off|nord)")
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
