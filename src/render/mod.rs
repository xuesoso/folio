//! Rendering: terminal capability detection and the image-display protocols
//! (sixel, kitty) used to paint PDF page bitmaps into the terminal.

pub mod caps;
pub mod kitty;
mod kitty_diacritics;
pub mod rgba;
pub mod sixel;

use std::io::Write;

pub use caps::{Backend, TermCaps, detect_caps};
use rgba::RgbaImage;

/// A fixed image id reused for every page (each draw deletes the previous image
/// under this id and transmits the new one). The bytes of "pdf", < 2^24 so it
/// fits a 24-bit foreground color for kitty Unicode placeholders.
pub const IMAGE_ID: u32 = 0x0070_6466;

/// Where an image sits on screen, in terminal cells.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Placement {
    /// 0-based top-left cell.
    pub col: u16,
    pub row: u16,
    /// Size of the image's cell area.
    pub cols: u16,
    pub rows: u16,
}

/// A sub-rectangle of a transmitted image, in image pixels — the visible window
/// for region panning (display this part of an already-sent image).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SrcRect {
    pub x: u32,
    pub y: u32,
    pub w: u32,
    pub h: u32,
}

/// A protocol-encoded image: the expensive, placement-independent bytes (base64
/// PNG for kitty, a sixel DCS stream for sixel). Caching these keeps revisiting a
/// view instant — `draw` only adds cheap framing and scales it to the cell box.
pub struct EncodedImage {
    pub payload: Vec<u8>,
}

impl EncodedImage {
    pub fn size(&self) -> usize {
        self.payload.len()
    }
}

/// Encodes [`RgbaImage`]s (already cropped to the visible window by the caller)
/// and paints the encoded result into the terminal.
///
/// `encode` does the expensive work (PNG/sixel compression) and its result is
/// cached by the caller; `draw` writes cheap framing around the cached payload
/// and transmits+displays it. Implementations write raw bytes to `out`; the
/// caller flushes once per frame.
pub trait ImageProtocol {
    fn encode(&self, img: &RgbaImage) -> crate::Result<EncodedImage>;
    fn draw(&mut self, out: &mut dyn Write, enc: &EncodedImage, at: &Placement)
    -> crate::Result<()>;
    fn clear(&mut self, out: &mut dyn Write, at: &Placement) -> crate::Result<()>;
    fn name(&self) -> &'static str;

    /// Whether this backend can display a sub-rectangle of an image transmitted
    /// once (kitty), so panning re-places the resident image instead of
    /// re-encoding and re-transmitting a fresh crop every frame. Sixel can't, so
    /// it keeps the per-crop `draw` path (the default `false`).
    fn supports_region_panning(&self) -> bool {
        false
    }

    /// Display `src` (a sub-rectangle, in image pixels) of a full render, scaled
    /// into the cell box `at`. When `enc_full` is `Some`, that full image is
    /// (re)transmitted first (a new zoom/page/theme); when `None`, the previously
    /// transmitted image is reused — the fast pan path that sends no pixels. Only
    /// called on backends where [`supports_region_panning`](Self::supports_region_panning)
    /// is `true`.
    fn draw_region(
        &mut self,
        _out: &mut dyn Write,
        _enc_full: Option<&EncodedImage>,
        _src: SrcRect,
        _at: &Placement,
    ) -> crate::Result<()> {
        Err(anyhow::anyhow!("region panning is not supported by this backend"))
    }
}

/// Build the display protocol implied by detected capabilities.
pub fn make_protocol(caps: &TermCaps) -> Box<dyn ImageProtocol> {
    match caps.backend {
        Backend::Kitty => {
            // Inside tmux we must route image data through passthrough and place
            // it with Unicode placeholders; outside tmux we transmit directly.
            let tmux = caps.in_tmux && caps.passthrough_ok;
            Box::new(kitty::KittyProtocol::new(IMAGE_ID, tmux))
        }
        Backend::Sixel => Box::new(sixel::SixelProtocol::new()),
    }
}

/// Append a 1-based absolute cursor-move (`CSI row;col H`) for a 0-based cell.
pub(crate) fn move_cursor(seq: &mut Vec<u8>, row0: u16, col0: u16) {
    seq.extend_from_slice(format!("\x1b[{};{}H", row0 as u32 + 1, col0 as u32 + 1).as_bytes());
}

/// Wrap a control sequence for tmux passthrough.
///
/// To reach the *outer* terminal from inside tmux, every `ESC` in the payload is
/// doubled and the whole thing is wrapped in `ESC P tmux; … ESC \`. The outer
/// terminal then receives the unwrapped sequence. Requires tmux
/// `allow-passthrough on`.
pub fn passthrough_wrap(seq: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(seq.len() + 16);
    out.extend_from_slice(b"\x1bPtmux;");
    for &b in seq {
        if b == 0x1b {
            out.push(0x1b);
        }
        out.push(b);
    }
    out.extend_from_slice(b"\x1b\\");
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn passthrough_doubles_esc_and_wraps() {
        let wrapped = passthrough_wrap(b"\x1b[c");
        assert_eq!(wrapped, b"\x1bPtmux;\x1b\x1b[c\x1b\\");
    }

    #[test]
    fn move_cursor_is_one_based() {
        let mut seq = Vec::new();
        move_cursor(&mut seq, 0, 0);
        assert_eq!(seq, b"\x1b[1;1H");
    }
}
