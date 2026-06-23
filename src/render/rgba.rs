//! A simple owned RGBA8 image buffer — the common currency between the PDF
//! backend (which produces it) and the display protocols (which consume it).
//!
//! The PDF backend produces raw RGBA and both the sixel and kitty encoders take
//! raw RGBA, so a plain `Vec<u8>` is all we need.

use crate::Result;

/// Row-major, 4-bytes-per-pixel (R, G, B, A) image.
#[derive(Clone)]
pub struct RgbaImage {
    pub width: u32,
    pub height: u32,
    pub data: Vec<u8>,
}

impl RgbaImage {
    /// A fully-transparent image of the given size.
    pub fn new(width: u32, height: u32) -> Self {
        let len = width as usize * height as usize * 4;
        Self {
            width,
            height,
            data: vec![0; len],
        }
    }

    /// Wrap an existing RGBA buffer, validating its length.
    pub fn from_raw(width: u32, height: u32, data: Vec<u8>) -> Result<Self> {
        let expected = width as usize * height as usize * 4;
        anyhow::ensure!(
            data.len() == expected,
            "rgba buffer size mismatch: expected {expected} bytes, got {}",
            data.len()
        );
        Ok(Self {
            width,
            height,
            data,
        })
    }

    pub fn is_empty(&self) -> bool {
        self.width == 0 || self.height == 0
    }

    /// Set a single pixel (no-op if out of bounds). Used by tests for synthetic
    /// patterns.
    #[cfg(test)]
    pub fn set_pixel(&mut self, x: u32, y: u32, rgba: [u8; 4]) {
        if x >= self.width || y >= self.height {
            return;
        }
        let i = (y as usize * self.width as usize + x as usize) * 4;
        self.data[i..i + 4].copy_from_slice(&rgba);
    }

    /// Rotate by `turns` × 90° clockwise (`turns` is taken mod 4). The viewer uses
    /// this to apply a view rotation to a full render before cropping/encoding, so
    /// every display backend rotates identically and format-agnostically. 90°/270°
    /// swap width and height; 0° is a cheap clone.
    pub fn rotated(&self, turns: u8) -> RgbaImage {
        let turns = turns % 4;
        if turns == 0 {
            return self.clone();
        }
        let (w, h) = (self.width as usize, self.height as usize);
        let (nw, nh) = if turns == 2 { (w, h) } else { (h, w) };
        let mut out = RgbaImage::new(nw as u32, nh as u32);
        for y in 0..h {
            for x in 0..w {
                // Destination of source pixel (x, y) for each clockwise rotation.
                let (dx, dy) = match turns {
                    1 => (h - 1 - y, x),         // 90° CW
                    2 => (w - 1 - x, h - 1 - y), // 180°
                    _ => (y, w - 1 - x),         // 270° CW
                };
                let si = (y * w + x) * 4;
                let di = (dy * nw + dx) * 4;
                out.data[di..di + 4].copy_from_slice(&self.data[si..si + 4]);
            }
        }
        out
    }

    /// Extract a sub-rectangle, clamped to the image bounds. The viewer uses this
    /// to pan around a page rendered larger than the viewport (an in-memory crop
    /// instead of re-rasterizing).
    pub fn crop(&self, x: u32, y: u32, w: u32, h: u32) -> RgbaImage {
        let x = x.min(self.width);
        let y = y.min(self.height);
        let w = w.min(self.width - x);
        let h = h.min(self.height - y);
        let mut out = RgbaImage::new(w, h);
        let src_stride = self.width as usize * 4;
        let dst_stride = w as usize * 4;
        for row in 0..h as usize {
            let src_off = (y as usize + row) * src_stride + x as usize * 4;
            let dst_off = row * dst_stride;
            out.data[dst_off..dst_off + dst_stride]
                .copy_from_slice(&self.data[src_off..src_off + dst_stride]);
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn crop_extracts_subrect() {
        let mut img = RgbaImage::new(4, 4);
        img.set_pixel(2, 1, [10, 20, 30, 40]);
        let c = img.crop(2, 1, 2, 2);
        assert_eq!(c.width, 2);
        assert_eq!(c.height, 2);
        assert_eq!(&c.data[0..4], &[10, 20, 30, 40]);
    }

    #[test]
    fn crop_clamps_to_bounds() {
        let img = RgbaImage::new(4, 4);
        let c = img.crop(3, 3, 10, 10);
        assert_eq!((c.width, c.height), (1, 1));
    }

    #[test]
    fn rotate90_swaps_dims_and_moves_top_left_to_top_right() {
        // 3×2 image with a marker at the top-left corner (0, 0).
        let mut img = RgbaImage::new(3, 2);
        img.set_pixel(0, 0, [1, 2, 3, 4]);
        let r = img.rotated(1); // 90° clockwise
        assert_eq!((r.width, r.height), (2, 3)); // dims swapped
        // Top-left of a 3-wide image lands at the top-right of the rotated image.
        assert_eq!(r.crop(r.width - 1, 0, 1, 1).data, [1, 2, 3, 4]);
    }

    #[test]
    fn rotate180_is_self_inverse_and_keeps_dims() {
        let mut img = RgbaImage::new(3, 2);
        img.set_pixel(2, 1, [9, 8, 7, 6]);
        let twice = img.rotated(2).rotated(2);
        assert_eq!((twice.width, twice.height), (3, 2));
        assert_eq!(twice.data, img.data);
    }

    #[test]
    fn four_quarter_turns_returns_original() {
        let mut img = RgbaImage::new(4, 3);
        img.set_pixel(1, 2, [5, 6, 7, 8]);
        let back = img.rotated(1).rotated(1).rotated(1).rotated(1);
        assert_eq!((back.width, back.height), (4, 3));
        assert_eq!(back.data, img.data);
    }
}
