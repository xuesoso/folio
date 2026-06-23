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
    }
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
