//! Static raster images as a one-page [`Document`] — the direct (no-PDF) seam.
//!
//! PNG, JPEG, GIF (first frame only), WebP, BMP, and TIFF are decoded with the
//! pure-Rust `image` crate (already in the dependency tree via hayro and
//! icy_sixel, so this adds no new compiled crates) and presented as a single
//! page. Unlike Markdown and LaTeX, there is no PDF round-trip: the decoded
//! pixels flow straight into the shared [`RgbaImage`] → sixel/kitty pipeline.
//!
//! Transparency is flattened onto an opaque white background at open — matching
//! the white page background the PDF renderer fills — so both display protocols
//! and the Nord theme transform see fully opaque pixels exactly as they do for a
//! PDF page.

use std::path::Path;

use image::imageops::FilterType;
use image::{ImageBuffer, Rgba};

use crate::Result;
use crate::document::{Document, PageInfo, RenderRequest};
use crate::render::rgba::RgbaImage;

/// The decoded image (the `image` crate's owned RGBA buffer), alpha already
/// flattened to opaque. Resampled per render request; never mutated after open.
type Decoded = ImageBuffer<Rgba<u8>, Vec<u8>>;

pub struct ImageDocument {
    img: Decoded,
}

/// Reject images whose declared dimensions exceed this on either axis — the
/// cheapest guard against a "decompression bomb" (a tiny file claiming enormous
/// dimensions), checked before any pixels are allocated.
const MAX_IMAGE_DIM: u32 = 50_000;
/// Cap total decode allocation so even an in-range-but-huge image (or an
/// intermediate buffer) can't exhaust memory. 512 MiB ≈ a 128-megapixel RGBA
/// image — far above any real page/photo, far below a bomb.
const MAX_IMAGE_ALLOC: u64 = 512 * 1024 * 1024;

impl ImageDocument {
    /// Open `path`, flattening any transparency onto `bg` (white matches the PDF
    /// renderer; an embedding UI may pass its own theme background instead).
    pub fn open(path: &Path, bg: [u8; 3]) -> Result<Self> {
        let bytes = std::fs::read(path)
            .map_err(|e| anyhow::anyhow!("failed to read {}: {e}", path.display()))?;
        let decoded = decode_limited(&bytes, MAX_IMAGE_DIM, MAX_IMAGE_ALLOC)
            .map_err(|e| anyhow::anyhow!("could not decode image {}: {e}", path.display()))?;
        let mut img = decoded.to_rgba8();
        anyhow::ensure!(
            img.width() > 0 && img.height() > 0,
            "image {} has zero size",
            path.display()
        );
        flatten_onto(&mut img, bg);
        Ok(Self { img })
    }
}

/// Decode an in-memory image with resource limits enforced, so a malicious or
/// corrupt image can't OOM the process. Format is guessed from the magic bytes;
/// an animated GIF decodes only its first frame — the static view we want.
fn decode_limited(bytes: &[u8], max_dim: u32, max_alloc: u64) -> Result<image::DynamicImage> {
    let mut limits = image::Limits::default();
    limits.max_image_width = Some(max_dim);
    limits.max_image_height = Some(max_dim);
    limits.max_alloc = Some(max_alloc);

    let mut reader = image::ImageReader::new(std::io::Cursor::new(bytes))
        .with_guessed_format()
        .map_err(|e| anyhow::anyhow!("{e}"))?;
    reader.limits(limits);
    reader.decode().map_err(|e| anyhow::anyhow!("{e}"))
}

impl Document for ImageDocument {
    fn page_count(&self) -> usize {
        1
    }

    fn page_info(&self, index: usize) -> Result<PageInfo> {
        anyhow::ensure!(index == 0, "page {} out of range", index + 1);
        // The viewer consumes these only as an aspect ratio, so treating one
        // pixel as one point is exactly right.
        Ok(PageInfo {
            width_pt: self.img.width() as f32,
            height_pt: self.img.height() as f32,
        })
    }

    fn render_page(&self, index: usize, req: &RenderRequest) -> Result<RgbaImage> {
        anyhow::ensure!(index == 0, "page {} out of range", index + 1);
        let (w, h) = (self.img.width(), self.img.height());

        // Mirror the hayro scale logic: fit to the target width, then cap height.
        let mut scale = req.target_width_px.max(1) as f32 / (w.max(1) as f32);
        if let Some(max_h) = req.max_height_px
            && h as f32 * scale > max_h as f32
        {
            scale = max_h as f32 / (h.max(1) as f32);
        }
        let tw = ((w as f32 * scale).round() as u32).max(1);
        let th = ((h as f32 * scale).round() as u32).max(1);

        let data = if tw == w && th == h {
            self.img.as_raw().clone()
        } else {
            // Lanczos3 when shrinking (best detail retention); a cheaper bilinear
            // when enlarging, where the wider filter support buys little and can
            // ring around hard edges. We always resample from the full-resolution
            // source, so repeated zoom never compounds resampling loss.
            let filter = if tw < w {
                FilterType::Lanczos3
            } else {
                FilterType::Triangle
            };
            image::imageops::resize(&self.img, tw, th, filter).into_raw()
        };
        RgbaImage::from_raw(tw, th, data)
    }

    fn title(&self) -> Option<&str> {
        None
    }
}

/// Composite any translucency onto the opaque background `bg` so every pixel
/// ends fully opaque (white matches the PDF renderer; embedders may pass a UI
/// theme color). Done once at open on the full-resolution image, before any
/// resampling — so resizing never has to reason about alpha and transparent
/// edges don't fringe.
fn flatten_onto(img: &mut Decoded, bg: [u8; 3]) {
    for px in img.pixels_mut() {
        let [r, g, b, a] = px.0;
        if a != 255 {
            let a = a as u16;
            let inv = 255 - a;
            // Straight-alpha over bg: out = src*a + bg*(1-a).
            px.0 = [
                ((r as u16 * a + bg[0] as u16 * inv) / 255) as u8,
                ((g as u16 * a + bg[1] as u16 * inv) / 255) as u8,
                ((b as u16 * a + bg[2] as u16 * inv) / 255) as u8,
                255,
            ];
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A 4×2 test image with a known pattern, alpha already opaque.
    fn fixture() -> ImageDocument {
        let mut img = Decoded::new(4, 2);
        for px in img.pixels_mut() {
            px.0 = [10, 20, 30, 255];
        }
        ImageDocument { img }
    }

    #[test]
    fn one_page_with_pixel_aspect() {
        let doc = fixture();
        assert_eq!(doc.page_count(), 1);
        let info = doc.page_info(0).expect("page info");
        assert_eq!((info.width_pt, info.height_pt), (4.0, 2.0));
        assert!((info.aspect() - 2.0).abs() < 1e-6);
        assert!(doc.page_info(1).is_err());
    }

    #[test]
    fn render_scales_to_target_width_keeping_aspect() {
        let doc = fixture();
        let img = doc
            .render_page(
                0,
                &RenderRequest {
                    target_width_px: 8,
                    max_height_px: None,
                },
            )
            .expect("render");
        assert_eq!((img.width, img.height), (8, 4)); // aspect preserved
        assert_eq!(img.data.len(), (8 * 4 * 4) as usize);
    }

    #[test]
    fn render_caps_height() {
        let doc = fixture();
        // Asking for width 8 would give height 4; cap it at 3.
        let img = doc
            .render_page(
                0,
                &RenderRequest {
                    target_width_px: 8,
                    max_height_px: Some(3),
                },
            )
            .expect("render");
        assert!(img.height <= 3);
    }

    #[test]
    fn native_size_skips_resampling() {
        let doc = fixture();
        let img = doc
            .render_page(
                0,
                &RenderRequest {
                    target_width_px: 4,
                    max_height_px: None,
                },
            )
            .expect("render");
        assert_eq!((img.width, img.height), (4, 2));
        // Untouched pixels come straight through.
        assert_eq!(&img.data[0..4], &[10, 20, 30, 255]);
    }

    #[test]
    fn every_enabled_format_decodes() {
        // Encode a tiny image to each format we advertise, then decode it back
        // through the same `load_from_memory` path `open` uses — proving the
        // feature flags wire up every decoder (auto-detected from magic bytes).
        use image::ImageFormat;
        for fmt in [
            ImageFormat::Png,
            ImageFormat::Jpeg,
            ImageFormat::Gif,
            ImageFormat::Bmp,
            ImageFormat::Tiff,
            ImageFormat::WebP,
        ] {
            let mut src = image::RgbaImage::new(3, 2);
            for px in src.pixels_mut() {
                px.0 = [120, 60, 30, 255];
            }
            let dynimg = image::DynamicImage::ImageRgba8(src);
            let mut buf = std::io::Cursor::new(Vec::new());
            dynimg
                .write_to(&mut buf, fmt)
                .unwrap_or_else(|e| panic!("encode {fmt:?}: {e}"));
            let decoded = image::load_from_memory(buf.get_ref())
                .unwrap_or_else(|e| panic!("decode {fmt:?}: {e}"));
            assert_eq!((decoded.width(), decoded.height()), (3, 2), "{fmt:?}");
        }
    }

    /// Encode an `w`×`h` opaque PNG into memory for the limit tests.
    fn png_bytes(w: u32, h: u32) -> Vec<u8> {
        let mut src = image::RgbaImage::new(w, h);
        for px in src.pixels_mut() {
            px.0 = [1, 2, 3, 255];
        }
        let mut buf = std::io::Cursor::new(Vec::new());
        image::DynamicImage::ImageRgba8(src)
            .write_to(&mut buf, image::ImageFormat::Png)
            .expect("encode png");
        buf.into_inner()
    }

    #[test]
    fn decode_limited_accepts_within_limits() {
        let bytes = png_bytes(16, 8);
        let img = decode_limited(&bytes, 50_000, 512 * 1024 * 1024).expect("decode");
        assert_eq!((img.width(), img.height()), (16, 8));
    }

    #[test]
    fn decode_limited_rejects_oversized_dimensions() {
        // A 16-px-wide image with the max dimension pinned to 8 must be refused
        // before any pixels are allocated — the decompression-bomb guard.
        let bytes = png_bytes(16, 8);
        assert!(decode_limited(&bytes, 8, 512 * 1024 * 1024).is_err());
    }

    #[test]
    fn decode_limited_rejects_excessive_allocation() {
        // Dimensions are fine, but the allocation budget is far too small to hold
        // the decoded pixels — the second line of defense.
        let bytes = png_bytes(64, 64);
        assert!(decode_limited(&bytes, 50_000, 1024).is_err());
    }

    #[test]
    fn flatten_makes_pixels_opaque_over_background() {
        let mut img = Decoded::new(1, 1);
        img.put_pixel(0, 0, Rgba([0, 0, 0, 0])); // fully transparent black
        flatten_onto(&mut img, [255, 255, 255]);
        assert_eq!(img.get_pixel(0, 0).0, [255, 255, 255, 255]); // becomes white

        let mut img = Decoded::new(1, 1);
        img.put_pixel(0, 0, Rgba([0, 0, 0, 0]));
        flatten_onto(&mut img, [0x28, 0x26, 0x2b]); // theme background
        assert_eq!(img.get_pixel(0, 0).0, [0x28, 0x26, 0x2b, 255]);
    }
}
