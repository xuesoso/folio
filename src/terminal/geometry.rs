//! Terminal size detection in cells and pixels.
//!
//! `crossterm::terminal::size()` gives the cell grid reliably. Pixel dimensions
//! come from `window_size()` (TIOCGWINSZ), but that frequently reports zero
//! inside tmux — in which case [`render::caps`](crate::render::caps) refines the
//! cell size with a `CSI 14 t` / `CSI 16 t` escape probe, and failing that we
//! fall back to an assumed cell size.

use crossterm::terminal::{size, window_size};

use crate::Result;

/// Assumed pixels-per-cell when the terminal reports nothing at all.
pub const FALLBACK_CELL_W: u16 = 10;
pub const FALLBACK_CELL_H: u16 = 20;

/// Terminal dimensions in cells plus the pixel size of a single character cell.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TermSize {
    pub cols: u16,
    pub rows: u16,
    pub cell_w: u16,
    pub cell_h: u16,
    /// True when `cell_w`/`cell_h` came from the OS rather than the fallback —
    /// lets the caps probe decide whether an escape-sequence refinement is worth
    /// attempting.
    pub cell_px_known: bool,
}

impl TermSize {
    /// Override the per-cell pixel size (used after an escape-sequence probe).
    pub fn with_cell_px(mut self, cell_w: u16, cell_h: u16) -> Self {
        if cell_w > 0 && cell_h > 0 {
            self.cell_w = cell_w;
            self.cell_h = cell_h;
            self.cell_px_known = true;
        }
        self
    }
}

/// Detect terminal size. Cell counts come from `size()`; per-cell pixels from
/// `window_size()` when available, else the fallback constants.
pub fn detect() -> Result<TermSize> {
    let (cols, rows) = size()?;
    let cols = cols.max(1);
    let rows = rows.max(1);

    let (mut cell_w, mut cell_h, mut known) = (0u16, 0u16, false);
    if let Ok(ws) = window_size()
        && ws.width > 0
        && ws.height > 0
    {
        cell_w = ws.width / cols;
        cell_h = ws.height / rows;
        known = cell_w > 0 && cell_h > 0;
    }
    if cell_w == 0 {
        cell_w = FALLBACK_CELL_W;
    }
    if cell_h == 0 {
        cell_h = FALLBACK_CELL_H;
    }

    Ok(TermSize {
        cols,
        rows,
        cell_w,
        cell_h,
        cell_px_known: known,
    })
}
