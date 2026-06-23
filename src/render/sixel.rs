//! Sixel display protocol.
//!
//! The caller crops each page to the visible window; we encode that crop (the
//! expensive Wu-quantization + Floyd–Steinberg dithering via the pure-Rust
//! `icy_sixel` crate) in [`encode`](SixelProtocol::encode) — cached by the caller
//! — and [`draw`](SixelProtocol::draw) positions the cursor and emits the cached
//! DCS, restoring the cursor so the status line stays put.
//!
//! Inside tmux we rely on tmux's native sixel support (tmux ≥ 3.4); sixel needs
//! no passthrough trickery because tmux parses the DCS itself.

use std::io::Write;

use icy_sixel::{EncodeOptions, sixel_encode};

use crate::Result;
use crate::render::rgba::RgbaImage;
use crate::render::{EncodedImage, ImageProtocol, Placement, move_cursor};

pub struct SixelProtocol {
    opts: EncodeOptions,
}

impl SixelProtocol {
    pub fn new() -> Self {
        Self {
            opts: EncodeOptions::default(),
        }
    }
}

impl ImageProtocol for SixelProtocol {
    fn encode(&self, img: &RgbaImage) -> Result<EncodedImage> {
        let dcs = if img.is_empty() {
            String::new()
        } else {
            sixel_encode(
                &img.data,
                img.width as usize,
                img.height as usize,
                &self.opts,
            )?
        };
        Ok(EncodedImage {
            payload: dcs.into_bytes(),
        })
    }

    fn draw(&mut self, out: &mut dyn Write, enc: &EncodedImage, at: &Placement) -> Result<()> {
        if enc.payload.is_empty() {
            return Ok(());
        }
        let mut seq = Vec::with_capacity(enc.payload.len() + 16);
        seq.extend_from_slice(b"\x1b7"); // save cursor
        move_cursor(&mut seq, at.row, at.col);
        seq.extend_from_slice(&enc.payload);
        seq.extend_from_slice(b"\x1b8"); // restore cursor
        out.write_all(&seq)?;
        Ok(())
    }

    fn clear(&mut self, out: &mut dyn Write, at: &Placement) -> Result<()> {
        let mut seq = Vec::new();
        for r in 0..at.rows {
            move_cursor(&mut seq, at.row + r, at.col);
            seq.extend_from_slice(b"\x1b[2K");
        }
        out.write_all(&seq)?;
        Ok(())
    }

    fn name(&self) -> &'static str {
        "sixel"
    }
}
