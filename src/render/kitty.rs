//! Kitty graphics protocol.
//!
//! The caller crops each page to the visible window and hands us that bitmap; we
//! encode it as PNG (`f=100`) and transmit+display it in one command. PDF pages
//! are mostly white, so PNG keeps the payload small.
//!
//! Two paths:
//! * **Direct** (not in tmux): `a=T` transmit-and-display at the cursor, `C=1` so
//!   the cursor doesn't move.
//! * **tmux / Unicode placeholders**: `a=T,U=1` transmits and creates a virtual
//!   placement sized to `c×r` cells, then we print a grid of U+10EEEE placeholder
//!   cells (image id in the foreground color, row/col via combining diacritics).
//!   tmux lays out the placeholder text; the outer terminal composites the image.
//!
//! A fixed image id is reused for every page: each draw first deletes the prior
//! image under that id, then transmits the new one.
//!
//! **Region panning** (direct path only): instead of re-encoding and
//! re-transmitting a fresh crop on every pan, the full render is transmitted once
//! (`a=t`) and each view displays a sub-rectangle of it (`a=p` with `x,y,w,h`).
//! Panning then sends no pixels — only a tiny placement command — which is the big
//! win for dense (photographic) images. See `draw_region`. The tmux/placeholder
//! path can't crop to a pixel rectangle, so it keeps the per-crop `draw` path.
//!
//! Reference: <https://sw.kovidgoyal.net/kitty/graphics-protocol/>

use std::io::Write;

use crate::Result;
use crate::render::kitty_diacritics::ROWCOLUMN_DIACRITICS;
use crate::render::rgba::RgbaImage;
use crate::render::{EncodedImage, ImageProtocol, Placement, SrcRect, move_cursor, passthrough_wrap};

const PLACEHOLDER: char = '\u{10EEEE}';
const CHUNK: usize = 4096;

pub struct KittyProtocol {
    /// Two image ids, alternated each draw: the new frame is transmitted+shown
    /// under one id *before* the previous id is deleted, so there's no blank gap
    /// (no black flash) while the new image transmits over a slow link.
    ids: [u32; 2],
    next: usize,
    shown: Option<u32>,
    tmux: bool,
    /// Region panning: the full image currently resident in the terminal, which
    /// pan re-places without re-transmitting (`None` until the first transmit).
    region_img: Option<u32>,
    /// Alternating placement id (1/2) so a new placement is created before the old
    /// is deleted — no flash, and placements never accumulate.
    region_pid: u32,
    /// Background painted behind the tmux placeholder cells. Without it they
    /// carry the terminal's default background, which shows through as (often
    /// white) strips wherever an image tile doesn't fully cover its cell.
    bg: Option<[u8; 3]>,
}

impl KittyProtocol {
    pub fn new(base_id: u32, tmux: bool) -> Self {
        Self {
            ids: [base_id, base_id.wrapping_add(1)],
            next: 0,
            shown: None,
            tmux,
            region_img: None,
            region_pid: 0,
            bg: None,
        }
    }

    /// Set the background painted behind tmux placeholder cells (see `bg`).
    pub fn with_bg(mut self, bg: Option<[u8; 3]>) -> Self {
        self.bg = bg;
        self
    }

    fn push(&self, seq: &mut Vec<u8>, bytes: &[u8]) {
        if self.tmux {
            seq.extend_from_slice(&passthrough_wrap(bytes));
        } else {
            seq.extend_from_slice(bytes);
        }
    }

    fn emit_delete_id(&self, seq: &mut Vec<u8>, id: u32) {
        let apc = format!("\x1b_Ga=d,d=I,i={id}\x1b\\");
        self.push(seq, apc.as_bytes());
    }

    /// Transmit + display, chunked. `controls` is the first chunk's control block
    /// (without the trailing `m=` flag, which we add).
    fn emit_transmit(&self, seq: &mut Vec<u8>, controls: &str, payload: &[u8]) {
        let mut offset = 0;
        let mut first = true;
        loop {
            let end = (offset + CHUNK).min(payload.len());
            let piece = &payload[offset..end];
            let more = end < payload.len();

            let mut apc = Vec::with_capacity(piece.len() + controls.len() + 16);
            apc.extend_from_slice(b"\x1b_G");
            if first {
                apc.extend_from_slice(controls.as_bytes());
                apc.extend_from_slice(format!(",m={}", more as u8).as_bytes());
                first = false;
            } else {
                apc.extend_from_slice(format!("m={}", more as u8).as_bytes());
            }
            apc.push(b';');
            apc.extend_from_slice(piece);
            apc.extend_from_slice(b"\x1b\\");
            self.push(seq, &apc);

            offset = end;
            if !more {
                break;
            }
        }
    }

    fn draw_direct(&self, seq: &mut Vec<u8>, id: u32, enc: &EncodedImage, at: &Placement) {
        move_cursor(seq, at.row, at.col);
        // c/r force the image to fill the cell footprint, so a downscaled bitmap
        // is upscaled by the terminal instead of occupying fewer cells.
        let controls = format!("a=T,f=100,i={},c={},r={},C=1,q=2", id, at.cols, at.rows);
        self.emit_transmit(seq, &controls, &enc.payload);
    }

    fn draw_tmux(&self, seq: &mut Vec<u8>, id: u32, enc: &EncodedImage, at: &Placement) {
        let max = ROWCOLUMN_DIACRITICS.len() as u16;
        let cols = at.cols.min(max);
        let rows = at.rows.min(max);

        let controls = format!("a=T,U=1,f=100,i={},c={},r={},q=2", id, cols, rows);
        self.emit_transmit(seq, &controls, &enc.payload);

        // fg encodes the image id; bg (when set) backs the cells so any sliver an
        // image tile doesn't cover shows the embedder's theme, not terminal white.
        let mut sgr = format!(
            "\x1b[38;2;{};{};{}m",
            (id >> 16) & 0xff,
            (id >> 8) & 0xff,
            id & 0xff
        );
        if let Some([r, g, b]) = self.bg {
            sgr.push_str(&format!("\x1b[48;2;{r};{g};{b}m"));
        }
        for r in 0..rows {
            move_cursor(seq, at.row + r, at.col);
            seq.extend_from_slice(sgr.as_bytes());
            let mut line = String::new();
            for c in 0..cols {
                line.push(PLACEHOLDER);
                line.push(ROWCOLUMN_DIACRITICS[r as usize]);
                line.push(ROWCOLUMN_DIACRITICS[c as usize]);
            }
            seq.extend_from_slice(line.as_bytes());
            seq.extend_from_slice(b"\x1b[39;49m");
        }
    }

    /// Display the source rectangle `src` of image `id`, scaled into the cell box
    /// `at`, under placement id `pid`. `a=p` (put) reuses already-transmitted data,
    /// so no pixels cross the wire — the fast pan path.
    fn emit_place_region(&self, seq: &mut Vec<u8>, id: u32, pid: u32, src: &SrcRect, at: &Placement) {
        move_cursor(seq, at.row, at.col);
        let apc = format!(
            "\x1b_Ga=p,i={id},p={pid},x={},y={},w={},h={},c={},r={},C=1,q=2\x1b\\",
            src.x, src.y, src.w, src.h, at.cols, at.rows
        );
        self.push(seq, apc.as_bytes());
    }

    /// Delete just one placement of an image, keeping the image data resident
    /// (lowercase `d=i` frees no pixels) so further regions can still be placed.
    fn emit_delete_placement(&self, seq: &mut Vec<u8>, id: u32, pid: u32) {
        let apc = format!("\x1b_Ga=d,d=i,i={id},p={pid}\x1b\\");
        self.push(seq, apc.as_bytes());
    }
}

impl ImageProtocol for KittyProtocol {
    fn encode(&self, img: &RgbaImage) -> Result<EncodedImage> {
        Ok(EncodedImage {
            payload: base64_encode(&encode_png(img)?),
        })
    }

    fn draw(&mut self, out: &mut dyn Write, enc: &EncodedImage, at: &Placement) -> Result<()> {
        if enc.payload.is_empty() {
            return Ok(());
        }
        let id = self.ids[self.next];
        let mut seq = Vec::new();
        // Draw the new image first...
        if self.tmux {
            self.draw_tmux(&mut seq, id, enc, at);
        } else {
            self.draw_direct(&mut seq, id, enc, at);
        }
        // ...then delete the previously shown one, so there's never a blank gap.
        if let Some(old) = self.shown
            && old != id
        {
            self.emit_delete_id(&mut seq, old);
        }
        out.write_all(&seq)?;
        self.shown = Some(id);
        self.next ^= 1;
        Ok(())
    }

    fn clear(&mut self, out: &mut dyn Write, at: &Placement) -> Result<()> {
        let mut seq = Vec::new();
        let ids = self.ids;
        for id in ids {
            self.emit_delete_id(&mut seq, id);
        }
        for r in 0..at.rows {
            move_cursor(&mut seq, at.row + r, at.col);
            seq.extend_from_slice(b"\x1b[2K");
        }
        out.write_all(&seq)?;
        self.shown = None;
        // The resident region image lives under one of `ids`, now deleted.
        self.region_img = None;
        self.region_pid = 0;
        Ok(())
    }

    fn name(&self) -> &'static str {
        if self.tmux { "kitty (tmux)" } else { "kitty" }
    }

    fn supports_region_panning(&self) -> bool {
        // The Unicode-placeholder (tmux) path tiles the whole image across cells
        // and can't crop to an arbitrary pixel rectangle, so region panning is
        // limited to the direct path.
        !self.tmux
    }

    fn draw_region(
        &mut self,
        out: &mut dyn Write,
        enc_full: Option<&EncodedImage>,
        src: SrcRect,
        at: &Placement,
    ) -> Result<()> {
        let mut seq = Vec::new();
        // Alternate placement ids so the new placement is created before the old
        // one is removed — no flash, no accumulation.
        let next_pid = if self.region_pid == 1 { 2 } else { 1 };

        match enc_full {
            // A new full image (zoom/page/theme changed): transmit it once without
            // displaying (`a=t`), place the visible region, then free the previous
            // resident image (after placing, so there's no blank gap).
            Some(enc) if !enc.payload.is_empty() => {
                let new_id = match self.region_img {
                    Some(cur) if cur == self.ids[0] => self.ids[1],
                    _ => self.ids[0],
                };
                let controls = format!("a=t,f=100,i={new_id},q=2");
                self.emit_transmit(&mut seq, &controls, &enc.payload);
                self.emit_place_region(&mut seq, new_id, next_pid, &src, at);
                if let Some(old) = self.region_img.replace(new_id)
                    && old != new_id
                {
                    self.emit_delete_id(&mut seq, old);
                }
            }
            // Empty payload — nothing to show.
            Some(_) => return Ok(()),
            // Same resident image: just re-place the new region (no pixels sent),
            // then drop the previous placement.
            None => {
                let Some(id) = self.region_img else {
                    return Ok(());
                };
                let prev_pid = self.region_pid;
                self.emit_place_region(&mut seq, id, next_pid, &src, at);
                self.emit_delete_placement(&mut seq, id, prev_pid);
            }
        }
        self.region_pid = next_pid;
        out.write_all(&seq)?;
        Ok(())
    }
}

/// Encode an image to PNG with fast compression (fdeflate-backed).
pub(crate) fn encode_png(img: &RgbaImage) -> Result<Vec<u8>> {
    let mut buf = Vec::new();
    let mut encoder = png::Encoder::new(&mut buf, img.width, img.height);
    encoder.set_color(png::ColorType::Rgba);
    encoder.set_depth(png::BitDepth::Eight);
    encoder.set_compression(png::Compression::Fast);
    let mut writer = encoder
        .write_header()
        .map_err(|e| anyhow::anyhow!("png header: {e}"))?;
    writer
        .write_image_data(&img.data)
        .map_err(|e| anyhow::anyhow!("png data: {e}"))?;
    writer
        .finish()
        .map_err(|e| anyhow::anyhow!("png finish: {e}"))?;
    Ok(buf)
}

/// Standard base64 with padding (avoids a `base64` crate dependency).
pub(crate) fn base64_encode(input: &[u8]) -> Vec<u8> {
    const T: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = Vec::with_capacity(input.len().div_ceil(3) * 4);
    for chunk in input.chunks(3) {
        let b0 = chunk[0] as u32;
        let b1 = *chunk.get(1).unwrap_or(&0) as u32;
        let b2 = *chunk.get(2).unwrap_or(&0) as u32;
        let n = (b0 << 16) | (b1 << 8) | b2;
        out.push(T[((n >> 18) & 63) as usize]);
        out.push(T[((n >> 12) & 63) as usize]);
        out.push(if chunk.len() > 1 {
            T[((n >> 6) & 63) as usize]
        } else {
            b'='
        });
        out.push(if chunk.len() > 2 {
            T[(n & 63) as usize]
        } else {
            b'='
        });
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn placement(cols: u16, rows: u16) -> Placement {
        Placement {
            col: 0,
            row: 0,
            cols,
            rows,
        }
    }

    #[test]
    fn base64_matches_known_vectors() {
        assert_eq!(base64_encode(b""), b"");
        assert_eq!(base64_encode(b"foo"), b"Zm9v");
        assert_eq!(base64_encode(b"foob"), b"Zm9vYg==");
        assert_eq!(base64_encode(b"fooba"), b"Zm9vYmE=");
        assert_eq!(base64_encode(b"foobar"), b"Zm9vYmFy");
    }

    #[test]
    fn png_is_far_smaller_than_raw_for_pagelike_image() {
        let mut img = RgbaImage::new(1200, 1500);
        for px in img.data.chunks_exact_mut(4) {
            px.copy_from_slice(&[255, 255, 255, 255]);
        }
        for y in (100..1400).step_by(40) {
            for x in 100..1100 {
                img.set_pixel(x, y, [0, 0, 0, 255]);
            }
        }
        let raw = img.data.len();
        let png = encode_png(&img).expect("encode").len();
        assert!(png * 10 < raw);
    }

    #[test]
    fn double_buffers_drawing_new_before_deleting_old() {
        let mut proto = KittyProtocol::new(0x706466, false);
        let enc = proto.encode(&RgbaImage::new(2, 2)).expect("encode");

        let mut out1 = Vec::new();
        proto.draw(&mut out1, &enc, &placement(1, 1)).expect("draw");
        let s1 = String::from_utf8_lossy(&out1);
        assert!(s1.contains("a=T,f=100,i=7365734,c=1,r=1,C=1,q=2,m=0;")); // id A
        assert!(!s1.contains("a=d")); // nothing to delete on the first frame

        let mut out2 = Vec::new();
        proto.draw(&mut out2, &enc, &placement(1, 1)).expect("draw");
        let s2 = String::from_utf8_lossy(&out2);
        let a_pos = s2.find("a=T,f=100,i=7365735").expect("id B drawn"); // base+1
        let d_pos = s2.find("a=d,d=I,i=7365734").expect("id A deleted");
        assert!(a_pos < d_pos, "new image must be drawn before old is deleted");
    }

    #[test]
    fn region_panning_supported_only_on_direct_path() {
        assert!(KittyProtocol::new(0x706466, false).supports_region_panning());
        assert!(!KittyProtocol::new(0x706466, true).supports_region_panning());
    }

    #[test]
    fn region_fresh_transmits_then_places_and_pan_reuses() {
        let mut proto = KittyProtocol::new(0x706466, false);
        let enc = proto.encode(&RgbaImage::new(8, 6)).expect("encode");
        let at = placement(4, 3);

        // Fresh full image: transmit-only (a=t) THEN place the source rectangle
        // (a=p with x/y/w/h and the cell box c/r).
        let mut out = Vec::new();
        let src = SrcRect { x: 1, y: 2, w: 4, h: 3 };
        proto.draw_region(&mut out, Some(&enc), src, &at).expect("fresh");
        let s = String::from_utf8_lossy(&out);
        let t = s.find("a=t,f=100,i=").expect("transmit-only");
        let p = s.find("a=p,i=").expect("placement");
        assert!(t < p, "transmit must precede placement: {s}");
        assert!(s.contains("x=1,y=2,w=4,h=3"), "source rect: {s}");
        assert!(s.contains("c=4,r=3"), "cell box: {s}");

        // Pan (no enc_full): MUST NOT re-transmit — only a fresh placement.
        let mut out2 = Vec::new();
        let src2 = SrcRect { x: 0, y: 0, w: 4, h: 3 };
        proto.draw_region(&mut out2, None, src2, &at).expect("pan");
        let s2 = String::from_utf8_lossy(&out2);
        assert!(!s2.contains("a=t"), "pan must send no pixels: {s2}");
        assert!(s2.contains("a=p,i="), "pan re-places: {s2}");
        assert!(s2.contains("x=0,y=0,w=4,h=3"), "new region: {s2}");
    }

    #[test]
    fn tmux_draw_uses_passthrough_and_placeholders() {
        let mut proto = KittyProtocol::new(0x706466, true);
        let enc = proto.encode(&RgbaImage::new(20, 20)).expect("encode");
        let mut out = Vec::new();
        proto.draw(&mut out, &enc, &placement(2, 1)).expect("draw");
        let s = String::from_utf8_lossy(&out);
        assert!(s.contains("\x1bPtmux;"));
        assert!(s.contains("a=T,U=1,f=100"));
        assert!(s.contains("c=2,r=1"));
        assert!(s.contains("\x1b[38;2;112;100;102m")); // first id (base) fg
        assert_eq!(s.matches(PLACEHOLDER).count(), 2);
    }
}
