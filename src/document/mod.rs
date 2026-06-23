//! Document abstraction — the modular input→PDF seam.
//!
//! The viewer only knows about [`Document`]: something with pages it can query
//! and rasterize. The single backend is hayro (pure Rust, no native dependency).
//! Future input types (Word, Markdown, LaTeX) can convert to PDF and reuse it,
//! or implement [`Document`] directly.

pub mod hayro;
pub mod image;
pub mod latex;
pub mod markdown;

use std::panic::AssertUnwindSafe;
use std::path::Path;

use crate::Result;
use crate::render::rgba::RgbaImage;
use hayro::HayroDocument;

/// Intrinsic page size in PDF points (1/72 inch).
#[derive(Debug, Clone, Copy)]
pub struct PageInfo {
    pub width_pt: f32,
    pub height_pt: f32,
}

impl PageInfo {
    /// Width / height. Falls back to 1.0 for degenerate pages.
    pub fn aspect(&self) -> f32 {
        if self.height_pt > 0.0 {
            self.width_pt / self.height_pt
        } else {
            1.0
        }
    }
}

/// How a page should be rasterized.
#[derive(Debug, Clone, Copy)]
pub struct RenderRequest {
    /// Desired output width in pixels; height follows the page aspect ratio.
    pub target_width_px: u32,
    /// Optional cap on output height (used by fit-page).
    pub max_height_px: Option<u32>,
}

/// An openable, renderable document.
pub trait Document {
    fn page_count(&self) -> usize;
    fn page_info(&self, index: usize) -> Result<PageInfo>;
    fn render_page(&self, index: usize, req: &RenderRequest) -> Result<RgbaImage>;
    fn title(&self) -> Option<&str>;
}

/// Run a render closure, catching both errors and panics (hayro can panic on a
/// few PDFs) with the panic message suppressed so it doesn't corrupt the TUI.
fn try_silently<T>(f: impl FnOnce() -> Result<T>) -> Option<T> {
    let previous = std::panic::take_hook();
    std::panic::set_hook(Box::new(|_| {}));
    let outcome = std::panic::catch_unwind(AssertUnwindSafe(f));
    std::panic::set_hook(previous);
    outcome.ok().and_then(|r| r.ok())
}

/// Wraps the hayro document so a panic on a bad page becomes a recoverable error
/// (shown in place) rather than aborting the viewer.
struct Guarded(HayroDocument);

impl Document for Guarded {
    fn page_count(&self) -> usize {
        self.0.page_count()
    }

    fn page_info(&self, index: usize) -> Result<PageInfo> {
        self.0.page_info(index)
    }

    fn render_page(&self, index: usize, req: &RenderRequest) -> Result<RgbaImage> {
        try_silently(|| self.0.render_page(index, req))
            .ok_or_else(|| anyhow::anyhow!("could not render page {}", index + 1))
    }

    fn title(&self) -> Option<&str> {
        self.0.title()
    }
}

/// True if `path`'s extension is in `exts` (case-insensitive).
fn has_ext(path: &Path, exts: &[&str]) -> bool {
    path.extension()
        .and_then(|e| e.to_str())
        .map(|e| e.to_ascii_lowercase())
        .is_some_and(|e| exts.contains(&e.as_str()))
}

fn is_markdown(path: &Path) -> bool {
    has_ext(path, &["md", "markdown", "mdown", "mkd", "mkdn", "mdwn"])
}

fn is_latex(path: &Path) -> bool {
    has_ext(path, &["tex", "latex"])
}

fn is_image(path: &Path) -> bool {
    has_ext(path, &["png", "jpg", "jpeg", "gif", "webp", "bmp", "tif", "tiff"])
}

/// A short, human label for the input format, used in the status line. Derived
/// purely from the path extension, mirroring the dispatch in [`open_document`]:
/// Markdown and LaTeX become PDFs internally but are still labelled by source,
/// and each image type reports its own name.
pub fn kind_label(path: &Path) -> &'static str {
    if is_markdown(path) {
        "Markdown"
    } else if is_latex(path) {
        "LaTeX"
    } else if is_image(path) {
        image_label(path)
    } else {
        "PDF"
    }
}

/// Specific label for a static image, by extension (e.g. `jpg`/`jpeg` → JPEG).
fn image_label(path: &Path) -> &'static str {
    match path
        .extension()
        .and_then(|e| e.to_str())
        .map(|e| e.to_ascii_lowercase())
        .as_deref()
    {
        Some("png") => "PNG",
        Some("jpg") | Some("jpeg") => "JPEG",
        Some("gif") => "GIF",
        Some("webp") => "WebP",
        Some("bmp") => "BMP",
        Some("tif") | Some("tiff") => "TIFF",
        _ => "Image",
    }
}

/// Build a [`Document`] from converted PDF bytes, panic-guarding the parse.
fn from_pdf_bytes(bytes: Vec<u8>, path: &Path) -> Result<HayroDocument> {
    let label = path.display().to_string();
    try_silently(|| HayroDocument::from_bytes(bytes, &label))
        .ok_or_else(|| anyhow::anyhow!("could not render the PDF produced from {}", path.display()))
}

/// Open a file as a [`Document`]. Static images (PNG/JPEG/GIF/WebP/BMP/TIFF)
/// render directly as a single page; PDFs render directly via hayro; Markdown is
/// converted to PDF on the fly; LaTeX is compiled with the configured engine
/// (`latex_engine`, opt-in) — all then render through the same pipeline.
pub fn open_document(path: &Path, latex_engine: Option<&str>) -> Result<Box<dyn Document>> {
    anyhow::ensure!(path.exists(), "file not found: {}", path.display());

    // Static images render directly — no PDF conversion, no panic guard (decoding
    // returns a `Result` rather than panicking the way hayro can).
    if is_image(path) {
        return Ok(Box::new(image::ImageDocument::open(path)?));
    }

    let doc = if is_markdown(path) {
        let text = std::fs::read_to_string(path)
            .map_err(|e| anyhow::anyhow!("failed to read {}: {e}", path.display()))?;
        let title = path.file_name().and_then(|s| s.to_str());
        let pdf_bytes = markdown::markdown_to_pdf(&text, title)
            .map_err(|e| anyhow::anyhow!("failed to convert {}: {e}", path.display()))?;
        from_pdf_bytes(pdf_bytes, path)?
    } else if is_latex(path) {
        let engine = latex_engine.ok_or_else(|| {
            anyhow::anyhow!(
                "{} is a LaTeX file, but no engine is configured.\n\
                 Set `latex_engine` in your folio config (~/.config/folio/config.toml), e.g.\n\
                 `latex_engine = tectonic` — folio runs no external TeX engine unless you opt in.",
                path.display()
            )
        })?;
        let pdf_bytes = latex::latex_to_pdf(path, engine)
            .map_err(|e| anyhow::anyhow!("failed to convert {}: {e}", path.display()))?;
        from_pdf_bytes(pdf_bytes, path)?
    } else {
        try_silently(|| HayroDocument::open(path)).ok_or_else(|| {
            anyhow::anyhow!("could not open {} (unsupported or corrupt PDF)", path.display())
        })?
    };

    Ok(Box::new(Guarded(doc)))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn open_document_renders_fixture() {
        let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/sample.pdf");
        if !path.exists() {
            return;
        }
        let doc = open_document(&path, None).expect("open");
        assert_eq!(doc.page_count(), 3);

        let info = doc.page_info(0).expect("page info");
        assert!(info.width_pt > 0.0 && info.height_pt > 0.0);

        let req = RenderRequest {
            target_width_px: 200,
            max_height_px: None,
        };
        let img = doc.render_page(0, &req).expect("render page 0");
        assert!(img.width > 0 && img.height > 0);
        assert_eq!(img.data.len(), (img.width * img.height * 4) as usize);
    }

    #[test]
    fn open_markdown_fixture_converts_and_renders() {
        let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/sample.md");
        if !path.exists() {
            return;
        }
        assert!(is_markdown(&path));
        // Goes through extension detection → markdown_to_pdf → hayro.
        let doc = open_document(&path, None).expect("open markdown");
        assert!(doc.page_count() >= 1);

        let req = RenderRequest {
            target_width_px: 400,
            max_height_px: None,
        };
        let img = doc.render_page(0, &req).expect("render page 0");
        assert!(img.width > 0 && img.height > 0);
    }

    #[test]
    fn latex_without_configured_engine_errors_clearly() {
        let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/sample.tex");
        if !path.exists() {
            return;
        }
        assert!(is_latex(&path));
        // Opt-in: no engine configured → a clear error, and nothing is spawned.
        let err = open_document(&path, None)
            .err()
            .expect("should require an engine");
        assert!(err.to_string().contains("latex_engine"), "got: {err}");
    }

    #[test]
    fn open_png_renders_directly_as_one_page() {
        // A 3×2 image written to a temp file, opened through the dispatch — the
        // image path never touches hayro/PDF. `::image` is the extern crate (a
        // bare `image` here would resolve to the sibling module).
        let mut img = ::image::RgbaImage::new(3, 2);
        for px in img.pixels_mut() {
            px.0 = [200, 30, 30, 255];
        }
        let path = std::env::temp_dir().join(format!("folio-test-{}.png", std::process::id()));
        img.save(&path).expect("write temp png");

        assert!(is_image(&path));
        assert_eq!(kind_label(&path), "PNG");

        let doc = open_document(&path, None).expect("open png");
        assert_eq!(doc.page_count(), 1);
        let info = doc.page_info(0).expect("page info");
        assert!((info.aspect() - 1.5).abs() < 1e-6);

        let req = RenderRequest {
            target_width_px: 30,
            max_height_px: None,
        };
        let rendered = doc.render_page(0, &req).expect("render");
        assert_eq!(rendered.width, 30);
        assert_eq!(rendered.data.len(), (rendered.width * rendered.height * 4) as usize);

        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn kind_label_covers_every_format() {
        assert_eq!(kind_label(Path::new("a.pdf")), "PDF");
        assert_eq!(kind_label(Path::new("a.md")), "Markdown");
        assert_eq!(kind_label(Path::new("a.tex")), "LaTeX");
        assert_eq!(kind_label(Path::new("a.JPEG")), "JPEG");
        assert_eq!(kind_label(Path::new("a.jpg")), "JPEG");
        assert_eq!(kind_label(Path::new("a.gif")), "GIF");
        assert_eq!(kind_label(Path::new("a.webp")), "WebP");
        assert_eq!(kind_label(Path::new("a.tiff")), "TIFF");
    }
}
