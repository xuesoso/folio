//! Terminal capability detection (ported from plotty's probe strategy).
//!
//! In a single round-trip we ask the terminal for:
//!   - its per-cell pixel size (`CSI 16 t`) and text-area pixel size (`CSI 14 t`),
//!   - whether it supports the kitty graphics protocol (a kitty `a=q` query),
//!   - optionally, its background colour (OSC 11), for `--theme auto`,
//!   - its Device Attributes (DA1, `ESC [ c`) — which doubles as a terminator and
//!     reports sixel support (attribute `4`).
//!
//! DA1 is sent last and every terminal answers it, so waiting for the DA1 reply
//! bounds how long we block. Anything we can't determine falls back to sixel,
//! the broadly-supported default.

use std::process::Command;
use std::time::Duration;

use crate::Result;
use crate::cli::BackendChoice;
use crate::render::passthrough_wrap;
use crate::terminal::Terminal;
use crate::terminal::geometry::TermSize;
use crate::terminal::query::query;

const PROBE_DEADLINE: Duration = Duration::from_millis(200);

/// kitty graphics support query: transmit-and-query a 1x1 RGB image using a
/// sentinel id. A kitty-capable terminal replies `ESC _ G i=<id>;OK ESC \`.
const KITTY_QUERY: &[u8] = b"\x1b_Gi=4294967295,s=1,v=1,a=q,t=d,f=24;AAAA\x1b\\";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Backend {
    Kitty,
    Sixel,
}

/// The detected terminal capabilities the rest of the app renders against.
#[derive(Debug, Clone, Copy)]
pub struct TermCaps {
    pub backend: Backend,
    pub in_tmux: bool,
    pub passthrough_ok: bool,
    pub size: TermSize,
    /// Terminal background colour from the OSC 11 reply; `None` if it wasn't
    /// asked for or the terminal didn't answer.
    pub bg: Option<[u8; 3]>,
}

/// Probe the terminal and resolve which backend to use. `choice` lets the user
/// force a backend; geometry/tmux are still detected either way. `want_bg` also
/// asks for the background colour (only needed to resolve `--theme auto`).
pub fn detect_caps(
    term: &mut Terminal,
    mut size: TermSize,
    choice: BackendChoice,
    want_bg: bool,
) -> Result<TermCaps> {
    let in_tmux = in_tmux();
    let passthrough_ok = in_tmux && tmux_passthrough_on();

    let probe = run_probe(
        term,
        in_tmux,
        passthrough_ok,
        !size.cell_px_known,
        want_bg,
        size.cols,
        size.rows,
    )?;
    if let Some((cw, ch)) = probe.cell_px {
        size = size.with_cell_px(cw, ch);
    }

    let backend = match choice {
        BackendChoice::Kitty => Backend::Kitty,
        BackendChoice::Sixel => Backend::Sixel,
        BackendChoice::Auto => choose_backend(in_tmux, &probe),
    };

    Ok(TermCaps {
        backend,
        in_tmux,
        passthrough_ok,
        size,
        bg: probe.bg,
    })
}

#[derive(Debug, Default)]
struct Probe {
    kitty_ok: bool,
    sixel: bool,
    cell_px: Option<(u16, u16)>,
    bg: Option<[u8; 3]>,
}

fn run_probe(
    term: &mut Terminal,
    in_tmux: bool,
    passthrough_ok: bool,
    want_cell: bool,
    want_bg: bool,
    cols: u16,
    rows: u16,
) -> Result<Probe> {
    let mut req: Vec<u8> = Vec::new();
    if want_cell {
        req.extend_from_slice(b"\x1b[16t"); // cell size in pixels
        req.extend_from_slice(b"\x1b[14t"); // text-area size in pixels
    }
    // Only emit the kitty APC query where it can actually reach a terminal:
    // outside tmux, or inside tmux with passthrough enabled (then it must be
    // wrapped). Sending raw kitty APC into tmux without passthrough is pointless
    // (tmux swallows it) so we skip it and rely on the env heuristic instead.
    if !in_tmux {
        req.extend_from_slice(KITTY_QUERY);
    } else if passthrough_ok {
        req.extend_from_slice(&passthrough_wrap(KITTY_QUERY));
    }
    if want_bg {
        // Background colour. Sent raw even inside tmux, which answers it itself
        // with the pane's colour (its outer terminal's default when unset).
        req.extend_from_slice(b"\x1b]11;?\x1b\\");
    }
    req.extend_from_slice(b"\x1b[c"); // DA1 — terminator + sixel attribute

    let buf = query(term.out(), &req, PROBE_DEADLINE, da1_complete)?;

    let mut cell_px = parse_csi_size(&buf, 6);
    if cell_px.is_none()
        && cols > 0
        && rows > 0
        && let Some((w, h)) = parse_csi_size(&buf, 4)
    {
        cell_px = Some((w / cols, h / rows));
    }

    Ok(Probe {
        kitty_ok: kitty_ok(&buf),
        sixel: da1_has_sixel(&buf),
        cell_px,
        bg: parse_osc11(&buf),
    })
}

fn choose_backend(in_tmux: bool, probe: &Probe) -> Backend {
    if probe.kitty_ok || kitty_capable_env(in_tmux) {
        return Backend::Kitty;
    }
    // Whether or not DA1 advertised sixel, it's our broadly-supported fallback.
    let _ = probe.sixel;
    Backend::Sixel
}

// ---- environment / tmux helpers --------------------------------------------

fn in_tmux() -> bool {
    if std::env::var_os("TMUX").is_some() {
        return true;
    }
    if let Ok(term) = std::env::var("TERM")
        && (term.starts_with("tmux") || term.starts_with("screen"))
    {
        return true;
    }
    matches!(std::env::var("TERM_PROGRAM").as_deref(), Ok("tmux"))
}

fn tmux_passthrough_on() -> bool {
    let output = Command::new("tmux")
        .args(["show-options", "-Apqv", "allow-passthrough"])
        .output();
    match output {
        Ok(o) => {
            let v = String::from_utf8_lossy(&o.stdout);
            let v = v.trim();
            v == "on" || v == "all"
        }
        Err(_) => false,
    }
}

fn tmux_client_termname() -> Option<String> {
    let output = Command::new("tmux")
        .args(["display-message", "-p", "#{client_termname}"])
        .output()
        .ok()?;
    let s = String::from_utf8_lossy(&output.stdout).trim().to_string();
    if s.is_empty() { None } else { Some(s) }
}

fn kitty_capable_env(in_tmux: bool) -> bool {
    if std::env::var_os("KITTY_WINDOW_ID").is_some() {
        return true;
    }
    let mut names: Vec<String> = Vec::new();
    if let Ok(v) = std::env::var("TERM") {
        names.push(v);
    }
    if let Ok(v) = std::env::var("TERM_PROGRAM") {
        names.push(v);
    }
    if in_tmux && let Some(v) = tmux_client_termname() {
        names.push(v);
    }
    names.iter().any(|n| {
        let n = n.to_ascii_lowercase();
        n.contains("kitty") || n.contains("ghostty") || n.contains("wezterm")
    })
}

// ---- reply parsers ----------------------------------------------------------

fn find(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    if needle.is_empty() || haystack.len() < needle.len() {
        return None;
    }
    haystack.windows(needle.len()).position(|w| w == needle)
}

fn split_numbers(body: &[u8], sep: u8) -> Vec<u32> {
    body.split(|&b| b == sep)
        .filter_map(|tok| std::str::from_utf8(tok).ok()?.trim().parse::<u32>().ok())
        .collect()
}

/// True once a DA1 reply (`ESC [ ? … c`) is present.
fn da1_complete(buf: &[u8]) -> bool {
    match find(buf, b"\x1b[?") {
        Some(start) => buf[start + 3..].contains(&b'c'),
        None => false,
    }
}

/// DA1 attribute `4` indicates sixel graphics support.
fn da1_has_sixel(buf: &[u8]) -> bool {
    if let Some(start) = find(buf, b"\x1b[?") {
        let rest = &buf[start + 3..];
        if let Some(end) = rest.iter().position(|&b| b == b'c') {
            return split_numbers(&rest[..end], b';').contains(&4);
        }
    }
    false
}

/// kitty graphics reply is `ESC _ G … ; OK ESC \`.
fn kitty_ok(buf: &[u8]) -> bool {
    if let Some(start) = find(buf, b"\x1b_G") {
        let rest = &buf[start..];
        if let Some(end) = find(rest, b"\x1b\\") {
            return find(&rest[..end], b"OK").is_some();
        }
    }
    false
}

/// Parse an OSC 11 background reply, `ESC ] 11 ; rgb:R/G/B` ended by BEL or
/// `ESC \`. Each channel is 1–4 hex digits (xterm's `XParseColor` form, usually
/// 4); it is scaled to 8 bits.
fn parse_osc11(buf: &[u8]) -> Option<[u8; 3]> {
    let start = find(buf, b"\x1b]11;")? + 5;
    let rest = &buf[start..];
    let end = rest.iter().position(|&b| b == 0x07 || b == 0x1b)?;
    let body = std::str::from_utf8(&rest[..end]).ok()?;
    let mut chans = body.strip_prefix("rgb:")?.split('/');
    let mut rgb = [0u8; 3];
    for c in &mut rgb {
        let hex = chans.next()?;
        if hex.is_empty() || hex.len() > 4 {
            return None;
        }
        let v = u32::from_str_radix(hex, 16).ok()?;
        let max = (1u32 << (4 * hex.len())) - 1;
        *c = ((v * 255 + max / 2) / max) as u8;
    }
    chans.next().is_none().then_some(rgb)
}

/// Parse a `ESC [ <code> ; <h> ; <w> t` report, returning `(width, height)`.
/// Used for `CSI 16 t` (code 6, cell size) and `CSI 14 t` (code 4, text area).
fn parse_csi_size(buf: &[u8], code: u32) -> Option<(u16, u16)> {
    let mut i = 0;
    while let Some(pos) = find(&buf[i..], b"\x1b[") {
        let start = i + pos + 2;
        let rest = &buf[start..];
        match rest.iter().position(|&b| b == b't' || b == 0x1b) {
            Some(tpos) => {
                if rest[tpos] == b't' {
                    let nums = split_numbers(&rest[..tpos], b';');
                    if nums.len() == 3 && nums[0] == code {
                        let w = nums[2].min(u16::MAX as u32) as u16;
                        let h = nums[1].min(u16::MAX as u32) as u16;
                        if w > 0 && h > 0 {
                            return Some((w, h));
                        }
                    }
                }
                i = start + tpos + 1;
            }
            None => break,
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn da1_with_sixel() {
        let buf = b"\x1b[?62;4;6;9;22c";
        assert!(da1_complete(buf));
        assert!(da1_has_sixel(buf));
    }

    #[test]
    fn da1_without_sixel() {
        let buf = b"\x1b[?62;22c";
        assert!(da1_complete(buf));
        assert!(!da1_has_sixel(buf));
    }

    #[test]
    fn da1_incomplete_until_c() {
        assert!(!da1_complete(b"\x1b[?62;4"));
        assert!(da1_complete(b"\x1b[?62;4c"));
    }

    #[test]
    fn kitty_ok_reply() {
        assert!(kitty_ok(b"\x1b_Gi=4294967295;OK\x1b\\"));
    }

    #[test]
    fn kitty_error_reply_is_not_ok() {
        assert!(!kitty_ok(b"\x1b_Gi=31;ENOTSUPPORTED:bad\x1b\\"));
    }

    #[test]
    fn cell_size_from_16t() {
        assert_eq!(parse_csi_size(b"\x1b[6;20;10t", 6), Some((10, 20)));
    }

    #[test]
    fn text_area_from_14t() {
        assert_eq!(parse_csi_size(b"\x1b[4;800;1200t", 4), Some((1200, 800)));
    }

    #[test]
    fn ignores_wrong_code_and_short_reports() {
        assert_eq!(parse_csi_size(b"\x1b[4;800;1200t", 6), None);
        assert_eq!(parse_csi_size(b"\x1b[8;24;80t", 6), None); // resize report, not cell size
    }

    #[test]
    fn osc11_background_reply() {
        // 16-bit channels, ST-terminated (kitty, xterm, iTerm2).
        assert_eq!(parse_osc11(b"\x1b]11;rgb:2e2e/3434/4040\x1b\\"), Some([0x2e, 0x34, 0x40]));
        // BEL-terminated, 8-bit and 4-bit channels.
        assert_eq!(parse_osc11(b"\x1b]11;rgb:ff/80/00\x07"), Some([0xff, 0x80, 0x00]));
        assert_eq!(parse_osc11(b"\x1b]11;rgb:f/0/8\x07"), Some([0xff, 0x00, 0x88]));
        // Mixed into the rest of the probe replies.
        let buf = b"\x1b[6;20;10t\x1b]11;rgb:ffff/ffff/ffff\x1b\\\x1b[?62;4c";
        assert_eq!(parse_osc11(buf), Some([0xff, 0xff, 0xff]));
        // No reply, truncated, or malformed.
        assert_eq!(parse_osc11(b"\x1b[?62;4c"), None);
        assert_eq!(parse_osc11(b"\x1b]11;rgb:ffff/ffff"), None);
        assert_eq!(parse_osc11(b"\x1b]11;rgb:ffff/ffff\x07"), None);
        assert_eq!(parse_osc11(b"\x1b]11;rgb:zz/00/00\x07"), None);
    }

    #[test]
    fn mixed_round_trip_buffer() {
        let buf = b"\x1b[6;20;10t\x1b[4;800;1200t\x1b_Gi=1;OK\x1b\\\x1b[?62;4c";
        assert_eq!(parse_csi_size(buf, 6), Some((10, 20)));
        assert_eq!(parse_csi_size(buf, 4), Some((1200, 800)));
        assert!(kitty_ok(buf));
        assert!(da1_has_sixel(buf));
        assert!(da1_complete(buf));
    }
}
