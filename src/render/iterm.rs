//! The iTerm2 inline-images protocol (OSC 1337 `File=`): base64 PNG placed at
//! the cursor, sized in terminal cells.
//!
//! iTerm2 renders sixel and this protocol, but not kitty graphics. Inside tmux,
//! sixel depends on tmux's own image re-encoding (fragile); this protocol can be
//! pushed through tmux *passthrough* instead — the cursor is moved with a normal
//! (pane-level) sequence first, then the wrapped OSC reaches iTerm2 directly, so
//! tmux's image handling is bypassed entirely (mirroring the kitty-in-tmux path).
//!
//! There is no delete command: the image occupies its cell rectangle, and
//! repainting those cells clears it (like sixel).

use std::io::Write;

use super::kitty::{base64_encode, encode_png};
use super::{move_cursor, passthrough_wrap, EncodedImage, ImageProtocol, Placement};
use crate::Result;
use crate::render::rgba::RgbaImage;

pub struct ItermProtocol {
    /// Wrap the OSC for tmux passthrough (`allow-passthrough on` required).
    tmux: bool,
}

impl ItermProtocol {
    pub fn new(tmux: bool) -> Self {
        Self { tmux }
    }
}

impl ImageProtocol for ItermProtocol {
    fn encode(&self, img: &RgbaImage) -> Result<EncodedImage> {
        Ok(EncodedImage {
            payload: base64_encode(&encode_png(img)?),
        })
    }

    fn draw(&mut self, out: &mut dyn Write, enc: &EncodedImage, at: &Placement) -> Result<()> {
        if enc.payload.is_empty() {
            return Ok(());
        }
        let mut seq = Vec::with_capacity(enc.payload.len() + 128);
        // Save the cursor, move to the box top-left, draw, restore. The moves
        // stay UNWRAPPED: in tmux they position the pane cursor (which tmux
        // mirrors to the client), so the passthrough image lands at the right
        // screen position — and the restore undoes the move afterwards.
        seq.extend_from_slice(b"\x1b7");
        move_cursor(&mut seq, at.row, at.col);

        // OSC 1337 File: inline display, sized in cells, BEL-terminated (BEL
        // contains no ESC, keeping the passthrough ESC-doubling minimal).
        // doNotMoveCursor=1 stops iTerm2 from advancing the cursor below the
        // image — without it, a preview-box-tall image pushes the cursor past
        // the bottom and SCROLLS the outer screen, corrupting the display.
        let mut osc = Vec::with_capacity(enc.payload.len() + 96);
        osc.extend_from_slice(
            format!(
                "\x1b]1337;File=inline=1;width={};height={};preserveAspectRatio=1;doNotMoveCursor=1:",
                at.cols, at.rows
            )
            .as_bytes(),
        );
        osc.extend_from_slice(&enc.payload);
        osc.push(0x07);

        if self.tmux {
            seq.extend_from_slice(&passthrough_wrap(&osc));
        } else {
            seq.extend_from_slice(&osc);
        }
        seq.extend_from_slice(b"\x1b8");
        out.write_all(&seq)?;
        Ok(())
    }

    fn clear(&mut self, _out: &mut dyn Write, _at: &Placement) -> Result<()> {
        // No delete command; the caller repaints the cell rectangle instead.
        Ok(())
    }

    fn name(&self) -> &'static str {
        "iterm2"
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tiny() -> RgbaImage {
        RgbaImage::from_raw(2, 2, vec![255; 16]).unwrap()
    }

    #[test]
    fn direct_draw_emits_move_then_osc_1337() {
        let mut p = ItermProtocol::new(false);
        let enc = p.encode(&tiny()).unwrap();
        let mut out = Vec::new();
        let at = Placement { col: 4, row: 2, cols: 10, rows: 5 };
        p.draw(&mut out, &enc, &at).unwrap();
        let s = String::from_utf8_lossy(&out);
        assert!(s.starts_with("\x1b7\x1b[3;5H"), "save + cursor move first: {s:?}");
        assert!(s.contains("\x1b]1337;File=inline=1;width=10;height=5"));
        assert!(s.contains("doNotMoveCursor=1"));
        assert!(s.ends_with("\x1b8"), "restores the cursor: {s:?}");
    }

    #[test]
    fn tmux_draw_wraps_osc_but_not_the_cursor_move() {
        let mut p = ItermProtocol::new(true);
        let enc = p.encode(&tiny()).unwrap();
        let mut out = Vec::new();
        let at = Placement { col: 0, row: 0, cols: 3, rows: 2 };
        p.draw(&mut out, &enc, &at).unwrap();
        let s = String::from_utf8_lossy(&out);
        // Unwrapped save + pane-level move, then the passthrough-wrapped OSC.
        assert!(s.starts_with("\x1b7\x1b[1;1H\x1bPtmux;"), "{s:?}");
        // Inner ESCs are doubled by the wrapper.
        assert!(s.contains("\x1b\x1b]1337;File="));
        // Passthrough terminator, then the unwrapped cursor restore.
        assert!(s.ends_with("\x1b\\\x1b8"), "{s:?}");
    }
}
