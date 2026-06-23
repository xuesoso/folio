//! Pure-Rust PDF rendering via the `hayro` crate — the only PDF engine (no
//! native dependency).
//!
//! hayro is younger than the C/C++ engines, so a few PDFs may render imperfectly
//! or panic; the `Guarded` wrapper in [`crate::document`] turns a panic into a
//! recoverable error rather than letting it abort the viewer.

use std::path::Path;

use hayro::hayro_interpret::InterpreterSettings;
use hayro::hayro_interpret::hayro_syntax::Pdf;
use hayro::vello_cpu::color::palette::css::WHITE;
use hayro::{RenderCache, RenderSettings, render};

use crate::Result;
use crate::document::{Document, PageInfo, RenderRequest};
use crate::render::rgba::RgbaImage;

pub struct HayroDocument {
    pdf: Pdf,
    /// Per-page (width, height) in PDF points, computed once at open so
    /// `page_info` is a cheap, panic-free lookup.
    dims: Vec<(f32, f32)>,
}

impl HayroDocument {
    pub fn open(path: &Path) -> Result<Self> {
        let bytes = std::fs::read(path)
            .map_err(|e| anyhow::anyhow!("failed to read {}: {e}", path.display()))?;
        Self::from_bytes(bytes, &path.display().to_string())
    }

    /// Build a document from in-memory PDF bytes. `label` is used only in error
    /// messages (e.g. the source path, or "<converted Markdown>").
    pub fn from_bytes(bytes: Vec<u8>, label: &str) -> Result<Self> {
        let pdf = Pdf::new(bytes)
            .map_err(|e| anyhow::anyhow!("hayro could not parse {label}: {e:?}"))?;
        let dims: Vec<(f32, f32)> = pdf.pages().iter().map(|p| p.render_dimensions()).collect();
        anyhow::ensure!(!dims.is_empty(), "PDF has no pages: {label}");
        Ok(Self { pdf, dims })
    }
}

impl Document for HayroDocument {
    fn page_count(&self) -> usize {
        self.dims.len()
    }

    fn page_info(&self, index: usize) -> Result<PageInfo> {
        let (w, h) = *self
            .dims
            .get(index)
            .ok_or_else(|| anyhow::anyhow!("page {} out of range", index + 1))?;
        Ok(PageInfo {
            width_pt: w,
            height_pt: h,
        })
    }

    fn render_page(&self, index: usize, req: &RenderRequest) -> Result<RgbaImage> {
        let pages = self.pdf.pages();
        let page = pages
            .get(index)
            .ok_or_else(|| anyhow::anyhow!("page {} out of range", index + 1))?;
        let (w_pt, h_pt) = page.render_dimensions();

        // Scale so the page is `target_width_px` wide; cap height if requested.
        let mut scale = req.target_width_px.max(1) as f32 / w_pt.max(1.0);
        if let Some(max_h) = req.max_height_px
            && h_pt * scale > max_h as f32
        {
            scale = max_h as f32 / h_pt.max(1.0);
        }

        let settings = RenderSettings {
            x_scale: scale,
            y_scale: scale,
            width: None,
            height: None,
            bg_color: WHITE, // opaque white background
        };
        let cache = RenderCache::new();
        let interpreter = InterpreterSettings::default();
        let pixmap = render(page, &cache, &interpreter, &settings);

        let width = pixmap.width() as u32;
        let height = pixmap.height() as u32;
        // Premultiplied RGBA8; with an opaque background this equals straight RGBA.
        let data = pixmap.data_as_u8_slice().to_vec();
        RgbaImage::from_raw(width, height, data)
    }

    fn title(&self) -> Option<&str> {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    #[ignore]
    fn timing_render() {
        let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/sample.pdf");
        let doc = HayroDocument::open(&path).expect("open");
        for w in [800u32, 1600, 2400, 3200] {
            let t = std::time::Instant::now();
            let img = doc
                .render_page(
                    0,
                    &RenderRequest {
                        target_width_px: w,
                        max_height_px: None,
                    },
                )
                .expect("render");
            eprintln!(
                "hayro render w={w} -> {}x{} in {:?}",
                img.width,
                img.height,
                t.elapsed()
            );
        }
    }
}
