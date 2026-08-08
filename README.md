# folio

A terminal viewer for **PDF**, **Markdown**, **LaTeX**, and **images**, rendering
pages as graphics using the **sixel** or **kitty** protocols — in the spirit of
`ranger` and `visidata`. PDFs and images render directly; Markdown is converted to
PDF on the fly, and LaTeX is compiled with your TeX engine (opt-in). Works over SSH
and inside tmux, renders pages lazily, and supports zoom, pan, and bookmarks.

## Features

- Renders PDF pages via **kitty graphics** or **sixel**, auto-detected.
- Opens **PDF**, **Markdown** (`.md`), **LaTeX** (`.tex`), and **static images**
  (`.png`, `.jpg`/`.jpeg`, `.gif`, `.webp`, `.bmp`, `.tif`/`.tiff`). Markdown and
  LaTeX are converted to PDF on the fly (LaTeX is opt-in); images render directly
  as a single page with no PDF round-trip (see [Markdown](#markdown),
  [LaTeX](#latex), and [Images](#images)).
- **100% Rust, no native dependency** — PDFs are rasterized by
  [hayro](https://github.com/LaurenzV/hayro). The result is a single
  self-contained binary with nothing to install or fetch.
- Works over **SSH** and inside **tmux** (with `allow-passthrough` for kitty).
- **Lazy** rendering with an LRU cache + predictive prefetch — navigation,
  scrolling, and zooming stay responsive.
- **Fast panning on kitty**: the zoomed page is transmitted once and panned by
  re-placing a sub-rectangle, so dragging around a large/photographic image sends
  no new pixels per frame (auto-detected; sixel uses the standard per-crop path).
- **wasd** and **vim** keybindings; zoom, pan & **90° rotation**; per-file **bookmarks** and
  **resume** (reopens where you left off).
- **Dark mode**: a Nord-tinted dark theme — `--theme`, live `i` toggle,
  remembered per file.
- Configurable **sharpness** (crispness vs. speed): `--sharpness` flag, live
  `[` / `]` keys, remembered per file.
- Modular **`Document` seam** — Markdown and LaTeX convert to PDF behind it, while
  images implement it directly; other formats (Word, …) can follow either route.

## Requirements

- Rust 1.92+ (edition 2024) and `cargo`.
- A terminal that supports kitty graphics (kitty, Ghostty, WezTerm) or sixel
  (xterm `-ti vt340`, foot, mlterm, recent VTE, …). `img2sixel` is a handy way to
  test sixel support.

No PDF library to install — hayro is pure Rust.

## Build

```sh
cargo build --release
./target/release/folio path/to/file.pdf
```

Because there's no C/C++ dependency, it also cross-compiles to a fully static
binary, e.g. `cargo build --release --target aarch64-unknown-linux-musl`.

> Tip: `cargo run` (debug) is fine — `Cargo.toml` builds dependencies optimized
> even in debug, so rendering stays fast while your own code is debuggable.

## Usage

```
folio <FILE> [--backend auto|kitty|sixel]
             [--theme off|nord]
             [--sharpness low|medium|high|full]
folio gen-config   # write a starter config file (see below)
folio print <FILE> [--protocol kitty|sixel] [--col N --row N --cols N --rows N]
            [--cell-width N --cell-height N] [--tmux] [--page N]
            [--theme off|nord] [--sharpness low|medium|high|full]
```

`folio print` is a **headless** mode: it renders one page into a terminal cell
box and writes the graphics escapes to stdout, then exits — no interactive
viewer. It lets another TUI embed a document/image preview inside a sub-rectangle
of its own screen (e.g. [rustranger](https://github.com/xuesoso/rustranger)'s
preview pane calls it per file).

`FILE` is a PDF, a Markdown file (`.md`, `.markdown`, `.mdown`, `.mkd`, `.mkdn`,
`.mdwn`), a LaTeX file (`.tex`, `.latex`), or a static image (`.png`, `.jpg`,
`.jpeg`, `.gif`, `.webp`, `.bmp`, `.tif`, `.tiff`) — e.g. `folio notes.md` or
`folio diagram.png`. Sample documents to try are in [`samples/`](samples/)
(`sample.md`, `sample.tex`, `sample.pdf`, `sample.png`).

`--theme nord` recolors pages for dark reading: it maps the background and text to
a dark Nord background and a light Nord foreground, while **keeping the color of
figures/images** (hue preserved, lightness inverted for dark mode); `off` is the
original. Toggle live with `i`; remembered per file.

`--sharpness` trades crispness for speed (kitty only): `full` renders at native
resolution (sharpest, slowest), lower tiers encode fewer pixels and let the
terminal upscale (faster). It can also be changed live with `[` / `]`, and is
remembered per file.

### Config file

Persistent defaults can be set in `~/.config/folio/config.toml` (the same
XDG-style path on Linux and macOS; `XDG_CONFIG_HOME` is honored). Generate a
documented starter file with:

```sh
folio gen-config
```

This writes a template with every option commented out (it never overwrites an
existing config). Four settings are supported — `theme`, `sharpness`, `fit`, and
`latex_engine`:

```toml
theme = nord            # off | nord
sharpness = high        # low | medium | high | full
fit = width             # page | width   (zoom levels are not configurable)
latex_engine = tectonic # command used to compile .tex files (see LaTeX, below)
```

It's a simple `key = value` file: `#` comment lines and blank lines are ignored,
quotes are optional, keys are case-insensitive, and unknown keys or bad values are
skipped. Each setting is optional — omit a line to leave that setting unmanaged.

**Precedence (highest first):** command-line flag → config file → remembered
per-file state → built-in default. Because the config sits *above* the per-file
state, a value set here is re-applied every time you open a document, even if you
changed it live (with `i` / `[` / `]` / `=` / `_`) in a previous session; a CLI
flag still overrides it for a single run. The page position is always resumed from
the per-file state. Zoom level is never read from the config.

### Keys

| Keys | Action |
|---|---|
| `j` `k` (`s` `w`, ↓ ↑) | Scroll down / up; at a page edge, press again to flip to the next/previous page (single press when the page already fits) |
| `h` `l` (`a` `d`, ← →) | Pan left / right (when zoomed wider than the screen) |
| `n` / `Space` / `p` | Next / previous page |
| `g` / `G` | First / last page |
| `:`〈n〉`Enter` | Go to page n |
| `+` / `-` | Zoom in / out |
| `=` / `0` | Fit-page · `_` Fit-width |
| `H` / `L` | Rotate 90° counter-clockwise / clockwise (applies to every page) |
| `[` / `]` | Sharpness softer / sharper (low → full) |
| `i` | Toggle Nord dark mode (off ↔ nord) |
| `m` | Toggle bookmark on the current page |
| `'` (or `` ` ``) | Bookmark list (`j`/`k` move, `Enter` jump, `d` delete, `Esc` close) |
| `?` | Help overlay |
| `q` / `Esc` | Quit (resumes here next time) |

### Mouse

| Input | Action |
|---|---|
| Wheel up / down | Scroll up / down (at a page edge, the next notch flips the page — same as `k`/`j`) |
| Horizontal wheel / two-finger swipe | Pan left / right |
| Left click | Next page |
| Right click | Previous page |

Only clicks and wheel notches are reported (no motion tracking), so the mouse
stays out of the way while reading. As with any full-screen TUI, the terminal's
own text selection needs the usual override while folio is running — hold
`Shift` (xterm, kitty, Ghostty, GNOME Terminal) or `Option`/`Alt` (iTerm2).

Bookmarks and resume state are stored per file under `~/.local/state/folio/`
(honoring `XDG_STATE_HOME`) — the same path on Linux and macOS.

## Markdown

Opening a Markdown file converts it to a PDF in memory (CommonMark via
[pulldown-cmark](https://github.com/raphlinus/pulldown-cmark), laid out with
[pdf-writer](https://github.com/typst/pdf-writer)) and then views it like any
other PDF — so zoom, pan, themes, sharpness, and bookmarks all apply.

Supported: headings, paragraphs, **bold**/*italic*, inline `code`, fenced code
blocks, ordered/unordered (and nested) lists, block quotes, and thematic breaks.

Fonts and symbols: body text uses the standard-14 Helvetica/Courier (which hayro
draws from its bundled substitutes — no font data for normal text). Characters
outside WinAnsi (Greek, math operators, arrows, …) are rendered from an embedded
**DejaVu Sans** fallback, which is only included in documents that actually use
such characters.

Not yet handled: tables, images, and math — `$…$` / `$$…$$` is shown as literal
text (to typeset math, open the document as [LaTeX](#latex) instead).

## LaTeX

LaTeX support is **opt-in**: folio never runs an external program unless you set a
`latex_engine` in your config (see [Config file](#config-file)). With it set,
opening a `.tex` / `.latex` file compiles the document to a PDF using that engine
and views the result — so you get full TeX fidelity, including typeset mathematics.

```toml
latex_engine = tectonic   # or latexmk, pdflatex, xelatex, lualatex
```

Known engines get the right arguments automatically (`pdflatex` / `xelatex` /
`lualatex` run twice to resolve cross-references; `latexmk` and `tectonic` manage
their own passes); an unrecognized command is invoked with pdflatex-compatible
flags. **Shell-escape is always disabled**, the document is compiled in a
temporary directory that is removed afterwards, and the working directory is the
file's own folder so `\input` and `\includegraphics` resolve. If compilation
fails, folio shows the relevant tail of the TeX log; if the engine isn't
installed, it says so.

## Images

Static images open as a single page and render **directly** — there is no PDF
conversion. PNG, JPEG, GIF, WebP, BMP, and TIFF are decoded with the pure-Rust
[image](https://github.com/image-rs/image) crate and the pixels go straight into
the same sixel/kitty pipeline, so zoom, pan, fit modes, sharpness, dark mode, and
bookmarks all apply exactly as they do to a PDF page. An animated GIF shows its
**first frame** only. Transparency is composited onto white (matching a PDF
page's background). Try it with `folio samples/sample.png`.

## Terminal notes

- **tmux:** for kitty graphics, enable passthrough: `tmux set -p allow-passthrough on`
  (tmux ≥ 3.3). For sixel, use tmux ≥ 3.4 with `set -as terminal-features ',*:sixel'`.
- **SSH:** works — only the encoded image bytes cross the wire. The *client*
  terminal must support the chosen protocol.
- **Nested tmux** (a local tmux *and* a remote tmux over SSH) is not generally
  supported: the inner process can't address the outer multiplexer. Use a single
  tmux layer, or enable sixel on both tmux instances and force `--backend sixel`.

## Architecture

- `document/` — the `Document` trait (the modular input seam), the hayro backend
  (`hayro.rs`, panic-guarded), the Markdown (`markdown.rs`) and LaTeX (`latex.rs`)
  → PDF converters, and the direct static-image backend (`image.rs`).
- `render/` — terminal capability detection (`caps`) and the `kitty` / `sixel`
  image protocols, behind an `ImageProtocol` trait (which also exposes optional
  region panning — transmit-once + sub-rectangle placement — for kitty).
- `terminal/` — RAII terminal setup, size/pixel detection, and bounded terminal
  queries.
- `cache.rs` — LRU cache of encoded page crops.
- `app/` — the event loop, view state (zoom/pan), and overlays.
- `bookmarks.rs` — per-file bookmarks and resume state.

## License

MIT — see [LICENSE](LICENSE).

The bundled Unicode fallback font, **DejaVu Sans** (`assets/fonts/`), is under its
own permissive license — see
[`assets/fonts/DejaVuSans-LICENSE`](assets/fonts/DejaVuSans-LICENSE).
