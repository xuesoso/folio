//! Terminal lifecycle: an RAII guard that puts the terminal into the state we
//! need for full-screen image rendering (raw mode, alternate screen, hidden
//! cursor, mouse reporting) and *guarantees* it is restored — on normal exit, on
//! error, and on panic.

pub mod geometry;
pub mod query;

use std::io::{self, BufWriter, Stdout, Write};

use anyhow::Context;
use crossterm::cursor::{Hide, Show};
use crossterm::terminal::{
    EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode,
};
use crossterm::{ExecutableCommand, execute};

use crate::Result;

/// Enable mouse reporting: button press/release (`?1000`) with SGR-encoded
/// coordinates (`?1006`, so columns past 223 still report correctly).
///
/// Deliberately *not* crossterm's `EnableMouseCapture`, which also turns on
/// any-motion tracking (`?1003`): folio reacts to clicks and the wheel only, and
/// motion reports would wake the event loop (stealing idle time from prefetch)
/// on every mouse move across the window.
const MOUSE_ON: &[u8] = b"\x1b[?1000h\x1b[?1006h";
/// Turn the above back off, in reverse order.
const MOUSE_OFF: &[u8] = b"\x1b[?1006l\x1b[?1000l";

/// Owns the terminal's raw/alt-screen state plus the single buffered writer that
/// every frame is composed into (one `flush()` per frame keeps rendering
/// flicker-free).
pub struct Terminal {
    out: BufWriter<Stdout>,
    active: bool,
}

impl Terminal {
    /// Enter raw mode + alternate screen, hide the cursor, and start mouse
    /// reporting.
    pub fn new() -> Result<Self> {
        enable_raw_mode()
            .context("failed to enter terminal raw mode (is stdout a real terminal?)")?;
        let mut out = BufWriter::new(io::stdout());
        execute!(out, EnterAlternateScreen, Hide)?;
        out.write_all(MOUSE_ON)?;
        out.flush()?;
        Ok(Self { out, active: true })
    }

    /// The shared buffered writer. Compose a whole frame, then call [`flush`].
    ///
    /// [`flush`]: Terminal::flush
    pub fn out(&mut self) -> &mut BufWriter<Stdout> {
        &mut self.out
    }

    /// Flush the buffered writer to the real terminal.
    pub fn flush(&mut self) -> Result<()> {
        self.out.flush()?;
        Ok(())
    }

    fn restore(&mut self) {
        if !self.active {
            return;
        }
        let _ = self.out.write_all(MOUSE_OFF);
        let _ = execute!(self.out, Show, LeaveAlternateScreen);
        let _ = self.out.flush();
        let _ = disable_raw_mode();
        self.active = false;
    }
}

impl Drop for Terminal {
    fn drop(&mut self) {
        self.restore();
    }
}

/// Install a panic hook that restores the terminal *before* the panic message is
/// printed, so a crash never leaves the user stuck in raw mode / alt screen.
///
/// Call once, before constructing [`Terminal`]. The default hook still runs
/// afterwards (printing the panic to the now-restored screen).
pub fn install_panic_hook() {
    let default_hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        let _ = disable_raw_mode();
        let _ = io::stdout().write_all(MOUSE_OFF);
        let _ = io::stdout().execute(Show);
        let _ = io::stdout().execute(LeaveAlternateScreen);
        default_hook(info);
    }));
}
