mod app;
mod bookmarks;
mod cache;
mod cli;
mod config;
mod document;
mod error;
mod render;
mod terminal;

pub use error::Result;

use anyhow::Context;

fn main() {
    if let Err(err) = run() {
        eprintln!("error: {err:#}");
        std::process::exit(1);
    }
}

fn run() -> Result<()> {
    match cli::parse()? {
        cli::Command::GenConfig => gen_config(),
        cli::Command::Open(args) => run_viewer(args),
        cli::Command::Print(args) => run_print(args),
    }
}

/// `print` subcommand: render one page into a terminal cell box and emit the
/// graphics escapes to stdout, then exit — no interactive viewer, no alternate
/// screen, no capability probing. Lets another TUI (e.g. rustranger) embed a
/// document/image preview inside a sub-rectangle of its own screen.
fn run_print(args: cli::PrintArgs) -> Result<()> {
    use std::io::Write;

    use render::{ImageProtocol, Placement, IMAGE_ID};

    let doc = document::open_document_with_bg(&args.path, None, args.bg)
        .with_context(|| format!("could not open {}", args.path.display()))?;

    // Fit the page into the pixel box, preserving aspect — the DISPLAY size that
    // fills the cell box on screen.
    let box_w = (args.cols as u32 * args.cell_w as u32).max(1);
    let box_h = (args.rows as u32 * args.cell_h as u32).max(1);
    let info = doc
        .page_info(args.page)
        .with_context(|| format!("reading page {}", args.page + 1))?;
    let aspect = if info.height_pt > 0.0 { info.width_pt / info.height_pt } else { 1.0 };
    let (disp_w, disp_h) = if box_w as f32 / box_h as f32 > aspect {
        ((box_h as f32 * aspect).round() as u32, box_h) // limited by height
    } else {
        (box_w, (box_w as f32 / aspect).round() as u32) // limited by width
    };
    let (disp_w, disp_h) = (disp_w.max(1), disp_h.max(1));

    // Render resolution: the display size, capped by the sharpness pixel budget
    // (kitty then upscales the smaller image into the display cell box).
    let (render_w, render_h) = match args.sharpness.max_display_pixels() {
        Some(cap) if (disp_w as u64) * (disp_h as u64) > cap => {
            let s = (cap as f64 / (disp_w as f64 * disp_h as f64)).sqrt();
            (((disp_w as f64 * s) as u32).max(1), ((disp_h as f64 * s) as u32).max(1))
        }
        _ => (disp_w, disp_h),
    };
    let req = document::RenderRequest {
        target_width_px: render_w,
        max_height_px: Some(render_h),
    };
    let mut rgba = doc
        .render_page(args.page, &req)
        .with_context(|| format!("rendering page {}", args.page + 1))?;
    args.theme.apply(&mut rgba.data); // Nord recolor (no-op for --theme off)

    // With --bg, pad the (themed) render onto a canvas covering the WHOLE cell
    // box so the box shows the embedder's background, not the terminal's; the
    // footprint is then the full box. Without it, the footprint is just the
    // aspect-fit image.
    let (rgba, img_cols, img_rows) = if let Some(bg) = args.bg {
        (rgba.centered_on(box_w, box_h, bg), args.cols, args.rows)
    } else {
        let img_cols = disp_w.div_ceil(args.cell_w as u32).clamp(1, args.cols as u32) as u16;
        let img_rows = disp_h.div_ceil(args.cell_h as u32).clamp(1, args.rows as u32) as u16;
        (rgba, img_cols, img_rows)
    };

    // Sixel encodes in 6-pixel-tall bands; a height that isn't a multiple of 6
    // gets padded by the encoder with its background register (typically white),
    // leaving a light strip under the image. Trim to the band boundary instead —
    // the (≤5 px) sliver of the bottom cells shows the cell background, which the
    // embedding UI paints in its own theme.
    let rgba = if matches!(args.protocol, cli::PrintProtocol::Sixel) && rgba.height % 6 != 0 {
        rgba.crop(0, 0, rgba.width, rgba.height - rgba.height % 6)
    } else {
        rgba
    };

    let mut proto: Box<dyn ImageProtocol> = match args.protocol {
        cli::PrintProtocol::Sixel => Box::new(render::sixel::SixelProtocol::new()),
        cli::PrintProtocol::Iterm => Box::new(render::iterm::ItermProtocol::new(args.tmux)),
        cli::PrintProtocol::Kitty => Box::new(render::kitty::KittyProtocol::new(IMAGE_ID, args.tmux).with_bg(args.bg)),
    };
    let enc = proto.encode(&rgba)?;
    let place = Placement {
        col: args.col,
        row: args.row,
        cols: img_cols,
        rows: img_rows,
    };

    let mut out = std::io::stdout().lock();
    proto.draw(&mut out, &enc, &place)?;
    out.flush()?;
    Ok(())
}

/// `gen-config` subcommand: scaffold a default config file (never overwrites).
fn gen_config() -> Result<()> {
    match config::write_default()? {
        config::GenOutcome::Created(path) => {
            println!("Wrote default config to {}", path.display());
            println!("All settings start commented out — edit the file to enable them.");
        }
        config::GenOutcome::Existed(path) => {
            println!("Config already exists at {} — left unchanged.", path.display());
        }
    }
    Ok(())
}

/// Open a document and run the interactive viewer.
fn run_viewer(args: cli::Args) -> Result<()> {
    // The user config is read up front: it supplies both the (opt-in) LaTeX
    // engine used while opening and the view defaults applied in `App::new`.
    let config = config::Config::load();

    // Open the document *before* entering the alternate screen, so any error
    // (missing file, bad PDF, LaTeX compile failure) prints to a normal terminal.
    let doc = document::open_document(&args.path, config.latex_engine.as_deref())
        .with_context(|| format!("could not open {}", args.path.display()))?;

    terminal::install_panic_hook();
    let mut term = terminal::Terminal::new()?;

    let size = terminal::geometry::detect()?;
    let caps = render::detect_caps(&mut term, size, args.backend)?;
    // Discard any late-arriving probe replies so they aren't read as keystrokes.
    terminal::query::drain_stdin();
    let proto = render::make_protocol(&caps);

    // Effective defaults, highest precedence first: CLI flags, then the user
    // config file; both override the remembered per-file state inside `App::new`.
    let defaults = config.with_cli_overrides(args.sharpness, args.theme);

    let mut app = app::App::new(term, caps, proto, doc, args.path, defaults);
    app.run()
}
