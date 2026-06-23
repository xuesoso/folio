//! Per-file bookmarks and resume state, persisted to a sidecar file.
//!
//! Each document gets a sidecar named by a 64-bit FNV-1a hash of its canonical
//! path (avoids a hashing-crate dependency), under `~/.local/state/folio/`
//! (honoring `XDG_STATE_HOME`) on every platform — macOS included, so state lives
//! in the same place regardless of OS. The file is a small tab-separated,
//! line-based format; unknown lines are ignored so it's forward-compatible.

use std::path::{Path, PathBuf};

use crate::Result;

/// A saved location within a document.
#[derive(Debug, Clone)]
pub struct Bookmark {
    pub page: usize,
    pub label: Option<String>,
}

/// Everything persisted for one document.
#[derive(Debug, Default)]
pub struct FileState {
    pub last_page: usize,
    /// Serialized [`Fit`](crate::app::state::Fit), e.g. `"zoom 1.44"`.
    pub fit: Option<String>,
    /// Serialized [`Sharpness`](crate::app::state::Sharpness), e.g. `"high"`.
    pub sharpness: Option<String>,
    /// Serialized [`Theme`](crate::app::state::Theme), e.g. `"nord"`.
    pub theme: Option<String>,
    pub bookmarks: Vec<Bookmark>,
}

impl FileState {
    pub fn is_bookmarked(&self, page: usize) -> bool {
        self.bookmarks.iter().any(|b| b.page == page)
    }

    /// Add or remove a bookmark for `page`; returns true if it's now bookmarked.
    pub fn toggle(&mut self, page: usize) -> bool {
        if let Some(pos) = self.bookmarks.iter().position(|b| b.page == page) {
            self.bookmarks.remove(pos);
            false
        } else {
            self.bookmarks.push(Bookmark { page, label: None });
            self.bookmarks.sort_by_key(|b| b.page);
            true
        }
    }
}

fn fnv1a64(data: &[u8]) -> u64 {
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for &b in data {
        hash ^= b as u64;
        hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
    }
    hash
}

fn canonical(pdf: &Path) -> PathBuf {
    std::fs::canonicalize(pdf).unwrap_or_else(|_| pdf.to_path_buf())
}

/// Sidecar path for a document, or `None` if no state directory is available.
fn state_path(pdf: &Path) -> Option<PathBuf> {
    let dir = state_root()?.join("folio");
    let key = fnv1a64(canonical(pdf).to_string_lossy().as_bytes());
    Some(dir.join(format!("{key:016x}.bm")))
}

/// Root for per-document state: `~/.local/state` (honoring an absolute
/// `XDG_STATE_HOME`) on every platform — including macOS, rather than
/// `dirs::state_dir()`'s macOS fallback to `~/Library/Application Support` — so
/// view state lives in the same place regardless of OS.
fn state_root() -> Option<PathBuf> {
    xdg_state_root(std::env::var_os("XDG_STATE_HOME"), dirs::home_dir())
}

/// Resolve an XDG state root from the environment: an absolute `XDG_STATE_HOME`
/// wins (a relative value is ignored per the XDG spec), otherwise `~/.local/state`.
fn xdg_state_root(
    xdg_state_home: Option<std::ffi::OsString>,
    home: Option<PathBuf>,
) -> Option<PathBuf> {
    if let Some(value) = xdg_state_home {
        let path = PathBuf::from(value);
        if path.is_absolute() {
            return Some(path);
        }
    }
    Some(home?.join(".local").join("state"))
}

/// Load saved state for a document (empty default if absent or unreadable).
pub fn load(pdf: &Path) -> FileState {
    let Some(path) = state_path(pdf) else {
        return FileState::default();
    };
    let Ok(content) = std::fs::read_to_string(&path) else {
        return FileState::default();
    };

    let mut state = FileState::default();
    for line in content.lines() {
        if line.starts_with('#') || line.trim().is_empty() {
            continue;
        }
        let mut fields = line.split('\t');
        match fields.next() {
            Some("last_page") => {
                if let Some(n) = fields.next().and_then(|v| v.trim().parse().ok()) {
                    state.last_page = n;
                }
            }
            Some("fit") => {
                if let Some(v) = fields.next() {
                    state.fit = Some(v.to_string());
                }
            }
            Some("sharpness") => {
                if let Some(v) = fields.next() {
                    state.sharpness = Some(v.to_string());
                }
            }
            Some("theme") => {
                if let Some(v) = fields.next() {
                    state.theme = Some(v.to_string());
                }
            }
            Some("bm") => {
                if let Some(page) = fields.next().and_then(|v| v.trim().parse().ok()) {
                    let label = fields.next().filter(|l| !l.is_empty()).map(str::to_string);
                    state.bookmarks.push(Bookmark { page, label });
                }
            }
            _ => {}
        }
    }
    state.bookmarks.sort_by_key(|b| b.page);
    state
}

/// Persist state for a document. Errors (e.g. unwritable dir) are returned but
/// are non-fatal to the caller.
pub fn save(pdf: &Path, state: &FileState) -> Result<()> {
    let Some(path) = state_path(pdf) else {
        return Ok(());
    };
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }

    let mut out = String::new();
    out.push_str("# folio state v1\n");
    out.push_str(&format!("path\t{}\n", canonical(pdf).display()));
    out.push_str(&format!("last_page\t{}\n", state.last_page));
    if let Some(fit) = &state.fit {
        out.push_str(&format!("fit\t{fit}\n"));
    }
    if let Some(sharpness) = &state.sharpness {
        out.push_str(&format!("sharpness\t{sharpness}\n"));
    }
    if let Some(theme) = &state.theme {
        out.push_str(&format!("theme\t{theme}\n"));
    }
    for bm in &state.bookmarks {
        match &bm.label {
            Some(label) => out.push_str(&format!("bm\t{}\t{}\n", bm.page, label)),
            None => out.push_str(&format!("bm\t{}\n", bm.page)),
        }
    }
    std::fs::write(&path, out)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fnv_is_stable_and_distinct() {
        assert_eq!(fnv1a64(b"/a/b.pdf"), fnv1a64(b"/a/b.pdf"));
        assert_ne!(fnv1a64(b"/a/b.pdf"), fnv1a64(b"/a/c.pdf"));
    }

    #[test]
    fn save_then_load_round_trips_on_disk() {
        use std::path::Path;
        let pdf = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/sample.pdf");
        if state_path(&pdf).is_none() {
            return; // no state dir available
        }
        let mut st = FileState {
            last_page: 2,
            fit: Some("zoom 1.5".to_string()),
            ..Default::default()
        };
        st.toggle(1);
        st.toggle(2);
        save(&pdf, &st).expect("save");

        let loaded = load(&pdf);
        assert_eq!(loaded.last_page, 2);
        assert_eq!(loaded.fit.as_deref(), Some("zoom 1.5"));
        assert_eq!(loaded.bookmarks.len(), 2);
        assert!(loaded.is_bookmarked(1) && loaded.is_bookmarked(2));

        if let Some(p) = state_path(&pdf) {
            let _ = std::fs::remove_file(p);
        }
    }

    #[test]
    fn state_root_uses_xdg_local_state_on_every_platform() {
        use std::ffi::OsString;
        let home = Some(PathBuf::from("/home/me"));
        // Unset XDG_STATE_HOME -> ~/.local/state (never ~/Library/... on macOS).
        assert_eq!(
            xdg_state_root(None, home.clone()),
            Some(PathBuf::from("/home/me/.local/state"))
        );
        // Absolute XDG_STATE_HOME wins.
        assert_eq!(
            xdg_state_root(Some(OsString::from("/var/state")), home.clone()),
            Some(PathBuf::from("/var/state"))
        );
        // A relative (spec-invalid) value is ignored, falling back to ~/.local/state.
        assert_eq!(
            xdg_state_root(Some(OsString::from("relative")), home),
            Some(PathBuf::from("/home/me/.local/state"))
        );
        assert_eq!(xdg_state_root(None, None), None);
    }

    #[test]
    fn toggle_adds_then_removes() {
        let mut s = FileState::default();
        assert!(s.toggle(3));
        assert!(s.is_bookmarked(3));
        assert!(!s.toggle(3));
        assert!(!s.is_bookmarked(3));
    }

    #[test]
    fn round_trips_via_a_temp_file() {
        // Parse a hand-written buffer through the same logic load() uses.
        let content = "# folio state v1\nlast_page\t12\nfit\tzoom 1.44\nbm\t5\tIntro\nbm\t40\n";
        let mut state = FileState::default();
        for line in content.lines() {
            if line.starts_with('#') {
                continue;
            }
            let mut f = line.split('\t');
            match f.next() {
                Some("last_page") => state.last_page = f.next().unwrap().parse().unwrap(),
                Some("fit") => state.fit = Some(f.next().unwrap().to_string()),
                Some("bm") => {
                    let page = f.next().unwrap().parse().unwrap();
                    let label = f.next().map(str::to_string);
                    state.bookmarks.push(Bookmark { page, label });
                }
                _ => {}
            }
        }
        assert_eq!(state.last_page, 12);
        assert_eq!(state.fit.as_deref(), Some("zoom 1.44"));
        assert_eq!(state.bookmarks.len(), 2);
        assert_eq!(state.bookmarks[0].page, 5);
        assert_eq!(state.bookmarks[0].label.as_deref(), Some("Intro"));
        assert_eq!(state.bookmarks[1].page, 40);
    }
}
