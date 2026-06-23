//! User-space configuration: persistent defaults read from `~/.config/folio/`
//! (`config.toml`). This XDG-style location (honoring `XDG_CONFIG_HOME`) is used
//! on macOS too, not `~/Library/Application Support`, so the config lives in the
//! same place on every Unix system.
//!
//! These defaults take precedence over the per-file remembered state, so a value
//! set here is applied on every open; a command-line flag still overrides for a
//! single run. Only `theme`, `sharpness`, and `fit` are configurable — zoom is a
//! transient view state and intentionally not settable here.

use std::path::PathBuf;

use crate::Result;
use crate::app::state::{Fit, Sharpness, Theme};

/// Defaults loaded from the user config file. Each field is `None` when the user
/// did not set it, so it only overrides where a value was actually provided.
#[derive(Debug, Default, Clone)]
pub struct Config {
    pub theme: Option<Theme>,
    pub sharpness: Option<Sharpness>,
    pub fit: Option<Fit>,
    /// Command used to compile `.tex` files. Opt-in: `None` means folio never
    /// runs an external program for LaTeX.
    pub latex_engine: Option<String>,
}

impl Config {
    /// Load the user config, or empty defaults if it is absent or unreadable.
    pub fn load() -> Config {
        config_path()
            .and_then(|p| std::fs::read_to_string(p).ok())
            .map(|c| Config::parse(&c))
            .unwrap_or_default()
    }

    /// Overlay command-line flags onto the config: a flag that was given wins over
    /// the config value (CLI is the highest-precedence source). Flags left unset
    /// (`None`) leave the config value in place.
    pub fn with_cli_overrides(mut self, sharpness: Option<Sharpness>, theme: Option<Theme>) -> Config {
        if sharpness.is_some() {
            self.sharpness = sharpness;
        }
        if theme.is_some() {
            self.theme = theme;
        }
        self
    }

    /// Parse `key = value` config text. Forgiving by design: `#` comment lines and
    /// blank lines are skipped, surrounding quotes are optional, keys are
    /// case-insensitive, and unknown keys or unparseable values are ignored — so a
    /// stale or typo'd config never breaks startup, it just falls through to the
    /// next source in the precedence chain.
    fn parse(content: &str) -> Config {
        let mut cfg = Config::default();
        for line in content.lines() {
            let line = line.trim();
            if line.is_empty() || line.starts_with('#') {
                continue;
            }
            let Some((key, value)) = line.split_once('=') else {
                continue;
            };
            let value = unquote(value.trim());
            match key.trim().to_ascii_lowercase().as_str() {
                "theme" => cfg.theme = Theme::parse(value),
                "sharpness" => cfg.sharpness = Sharpness::parse(value),
                "fit" => cfg.fit = parse_fit(value),
                "latex_engine" | "latex-engine" if !value.is_empty() => {
                    cfg.latex_engine = Some(value.to_string());
                }
                _ => {}
            }
        }
        cfg
    }
}

/// Config accepts only the two preset fits; zoom levels are not configurable.
fn parse_fit(s: &str) -> Option<Fit> {
    match s.trim().to_ascii_lowercase().as_str() {
        "page" | "fit-page" => Some(Fit::Page),
        "width" | "fit-width" => Some(Fit::Width),
        _ => None,
    }
}

/// Strip a single pair of matching surrounding quotes, if present.
fn unquote(s: &str) -> &str {
    let bytes = s.as_bytes();
    if bytes.len() >= 2 {
        let (first, last) = (bytes[0], bytes[bytes.len() - 1]);
        if first == last && (first == b'"' || first == b'\'') {
            return &s[1..s.len() - 1];
        }
    }
    s
}

/// `<config root>/folio/config.toml`, or `None` if no config root is available.
fn config_path() -> Option<PathBuf> {
    Some(config_root()?.join("folio").join("config.toml"))
}

/// On macOS, use the XDG-style `~/.config` (honoring an absolute
/// `XDG_CONFIG_HOME`) rather than `dirs::config_dir()`'s
/// `~/Library/Application Support`, so the config lives in the same place as on
/// every other Unix system.
#[cfg(target_os = "macos")]
fn config_root() -> Option<PathBuf> {
    xdg_config_root(std::env::var_os("XDG_CONFIG_HOME"), dirs::home_dir())
}

/// Everywhere else, defer to the platform config dir (`$XDG_CONFIG_HOME` or
/// `~/.config` on Linux; the OS equivalent elsewhere).
#[cfg(not(target_os = "macos"))]
fn config_root() -> Option<PathBuf> {
    dirs::config_dir()
}

/// Resolve an XDG config root from the environment: an absolute `XDG_CONFIG_HOME`
/// wins (per the XDG spec a relative value is ignored), otherwise `~/.config`.
/// Kept platform-agnostic and built under `test` so the macOS path is exercised
/// on any host.
#[cfg(any(target_os = "macos", test))]
fn xdg_config_root(
    xdg_config_home: Option<std::ffi::OsString>,
    home: Option<PathBuf>,
) -> Option<PathBuf> {
    if let Some(value) = xdg_config_home {
        let path = PathBuf::from(value);
        if path.is_absolute() {
            return Some(path);
        }
    }
    Some(home?.join(".config"))
}

/// The template written by `gen-config`. Every setting is shown but commented
/// out, so generating the file documents the options without changing behavior
/// until the user edits it.
const DEFAULT_CONFIG: &str = "\
# folio configuration
#
# Persistent defaults applied on every open. These override the remembered
# per-file state, but a command-line flag still overrides them for a single run.
# Uncomment a line to enable that setting. Keys are case-insensitive and quotes
# are optional. Zoom level is intentionally not configurable.

# Color theme / dark mode.              Values: off | nord
# theme = off

# Display sharpness vs. speed (kitty).  Values: low | medium | high | full
# sharpness = medium

# Page fit mode.                        Values: page | width
# fit = page

# LaTeX engine for viewing .tex files. Opt-in: folio runs no external program
# unless this is set. Known engines get the right arguments automatically and
# shell-escape is always disabled.
#                       Values: tectonic | latexmk | pdflatex | xelatex | lualatex
# latex_engine = tectonic
";

/// Outcome of `gen-config`, for the caller to report.
pub enum GenOutcome {
    /// A new config file was written at this path.
    Created(PathBuf),
    /// A config file already existed at this path and was left untouched.
    Existed(PathBuf),
}

/// Create a default config file, unless one already exists (never overwrites).
pub fn write_default() -> Result<GenOutcome> {
    let path = config_path()
        .ok_or_else(|| anyhow::anyhow!("no config directory is available on this platform"))?;
    if path.exists() {
        return Ok(GenOutcome::Existed(path));
    }
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)
            .map_err(|e| anyhow::anyhow!("could not create {}: {e}", dir.display()))?;
    }
    std::fs::write(&path, DEFAULT_CONFIG)
        .map_err(|e| anyhow::anyhow!("could not write {}: {e}", path.display()))?;
    Ok(GenOutcome::Created(path))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_config_sets_nothing() {
        let cfg = Config::parse("");
        assert!(cfg.theme.is_none() && cfg.sharpness.is_none() && cfg.fit.is_none());
    }

    #[test]
    fn parses_all_three_settings() {
        let cfg = Config::parse("theme = nord\nsharpness = high\nfit = width\n");
        assert_eq!(cfg.theme, Some(Theme::Nord));
        assert_eq!(cfg.sharpness, Some(Sharpness::High));
        assert_eq!(cfg.fit, Some(Fit::Width));
    }

    #[test]
    fn ignores_comments_blanks_and_unknown_keys() {
        let text = "# a comment\n\n  theme = off  \nbogus = 1\nwidth = 2\n";
        let cfg = Config::parse(text);
        assert_eq!(cfg.theme, Some(Theme::Off));
        assert!(cfg.sharpness.is_none() && cfg.fit.is_none());
    }

    #[test]
    fn strips_quotes_and_is_case_insensitive() {
        let cfg = Config::parse("THEME = \"Nord\"\nFit = 'Page'\n");
        assert_eq!(cfg.theme, Some(Theme::Nord));
        assert_eq!(cfg.fit, Some(Fit::Page));
    }

    #[test]
    fn fit_rejects_zoom_levels() {
        // Zoom is intentionally not configurable.
        assert_eq!(Config::parse("fit = zoom 1.5\n").fit, None);
        assert_eq!(Config::parse("fit = 150%\n").fit, None);
    }

    #[test]
    fn invalid_values_are_ignored() {
        let cfg = Config::parse("theme = lavender\nsharpness = ultra\n");
        assert!(cfg.theme.is_none() && cfg.sharpness.is_none());
    }

    #[test]
    fn default_template_changes_nothing_until_edited() {
        // Every line in the generated template is commented, so a freshly written
        // config must parse to all-`None` (no behavior change until the user edits).
        let cfg = Config::parse(DEFAULT_CONFIG);
        assert!(
            cfg.theme.is_none()
                && cfg.sharpness.is_none()
                && cfg.fit.is_none()
                && cfg.latex_engine.is_none()
        );
    }

    #[test]
    fn macos_config_root_uses_xdg_dot_config_not_library() {
        use std::ffi::OsString;
        let home = Some(PathBuf::from("/Users/me"));
        // Unset XDG_CONFIG_HOME -> ~/.config (not ~/Library/Application Support).
        assert_eq!(
            xdg_config_root(None, home.clone()),
            Some(PathBuf::from("/Users/me/.config"))
        );
        // An absolute XDG_CONFIG_HOME wins, just like on Linux.
        assert_eq!(
            xdg_config_root(Some(OsString::from("/custom/cfg")), home.clone()),
            Some(PathBuf::from("/custom/cfg"))
        );
        // A relative (spec-invalid) XDG value is ignored, falling back to ~/.config.
        assert_eq!(
            xdg_config_root(Some(OsString::from("relative/cfg")), home),
            Some(PathBuf::from("/Users/me/.config"))
        );
        // No home and no usable XDG value -> nothing.
        assert_eq!(xdg_config_root(None, None), None);
    }

    #[test]
    fn parses_latex_engine() {
        assert_eq!(
            Config::parse("latex_engine = tectonic\n").latex_engine.as_deref(),
            Some("tectonic")
        );
        assert!(Config::parse("").latex_engine.is_none());
    }

    #[test]
    fn default_template_examples_are_valid_when_uncommented() {
        // Uncommenting the documented example lines must yield valid settings.
        let uncommented = DEFAULT_CONFIG.replace("# theme", "theme");
        assert_eq!(Config::parse(&uncommented).theme, Some(Theme::Off));
    }
}
