//! LaTeX → PDF conversion by invoking an external TeX engine.
//!
//! **Opt-in only:** the viewer calls this exclusively when the user has set
//! `latex_engine` in the config file, so folio never spawns a subprocess on its
//! own. Known engines (tectonic, latexmk, pdflatex, xelatex, lualatex) get the
//! right arguments automatically; an unrecognized command is invoked with
//! pdflatex-compatible flags. Shell-escape is always disabled, and the document
//! is compiled into a temporary directory that is removed afterwards.

use std::ffi::{OsStr, OsString};
use std::io::ErrorKind;
use std::path::Path;
use std::process::Command;

use crate::Result;

/// Compile `path` to PDF bytes using `engine` (a command name or path).
pub fn latex_to_pdf(path: &Path, engine: &str) -> Result<Vec<u8>> {
    let path = path.canonicalize().unwrap_or_else(|_| path.to_path_buf());
    let parent = path.parent().unwrap_or_else(|| Path::new(".")).to_path_buf();
    let stem = path
        .file_stem()
        .and_then(|s| s.to_str())
        .ok_or_else(|| anyhow::anyhow!("invalid .tex file name: {}", path.display()))?
        .to_string();
    let input: OsString = path
        .file_name()
        .unwrap_or_else(|| OsStr::new("input.tex"))
        .to_os_string();

    let outdir = std::env::temp_dir().join(format!(
        "folio-latex-{}-{:08x}",
        std::process::id(),
        hash(path.as_os_str())
    ));
    std::fs::create_dir_all(&outdir)
        .map_err(|e| anyhow::anyhow!("could not create temp dir {}: {e}", outdir.display()))?;

    // Compile, then always clean up the temp directory.
    let result = compile(engine, &parent, &outdir, &input, &stem, &path);
    let _ = std::fs::remove_dir_all(&outdir);
    result
}

fn compile(engine: &str, cwd: &Path, outdir: &Path, input: &OsStr, stem: &str, src: &Path) -> Result<Vec<u8>> {
    let (args, runs) = build_args(engine, outdir, input);

    let mut captured = String::new();
    for _ in 0..runs {
        let output = Command::new(engine)
            .args(&args)
            .current_dir(cwd)
            .output()
            .map_err(|e| match e.kind() {
                ErrorKind::NotFound => anyhow::anyhow!(
                    "LaTeX engine '{engine}' not found — install it or fix `latex_engine` in your config"
                ),
                _ => anyhow::anyhow!("could not run '{engine}': {e}"),
            })?;
        captured = format!(
            "{}{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        // Stop early on a hard error (with -halt-on-error, warnings still exit 0).
        if !output.status.success() {
            break;
        }
    }

    let pdf = outdir.join(format!("{stem}.pdf"));
    match std::fs::read(&pdf) {
        Ok(bytes) if !bytes.is_empty() => Ok(bytes),
        _ => {
            let log = std::fs::read_to_string(outdir.join(format!("{stem}.log"))).unwrap_or_default();
            let detail = error_detail(&log, &captured);
            Err(anyhow::anyhow!(
                "'{engine}' did not produce a PDF for {}:\n\n{detail}",
                src.display()
            ))
        }
    }
}

/// Engine arguments and the number of compile passes. Output goes to `outdir`;
/// the input is named relative to its own directory (set as the working dir) so
/// `\input` / `\includegraphics` resolve. Shell-escape is disabled throughout.
fn build_args(engine: &str, outdir: &Path, input: &OsStr) -> (Vec<OsString>, u8) {
    let base = Path::new(engine)
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or(engine)
        .to_ascii_lowercase();

    let mut args: Vec<OsString> = Vec::new();
    match base.as_str() {
        "tectonic" => {
            // tectonic disables shell-escape by default.
            args.push("--outdir".into());
            args.push(outdir.as_os_str().to_os_string());
            args.push(input.to_os_string());
            (args, 1)
        }
        "latexmk" => {
            // latexmk iterates passes itself; no shell-escape by default.
            args.push("-pdf".into());
            args.push("-interaction=nonstopmode".into());
            args.push("-halt-on-error".into());
            let mut outflag = OsString::from("-output-directory=");
            outflag.push(outdir);
            args.push(outflag);
            args.push(input.to_os_string());
            (args, 1)
        }
        // pdflatex / xelatex / lualatex, and anything else assumed compatible.
        _ => {
            args.push("-interaction=nonstopmode".into());
            args.push("-halt-on-error".into());
            args.push("-no-shell-escape".into());
            let mut outflag = OsString::from("-output-directory=");
            outflag.push(outdir);
            args.push(outflag);
            args.push(input.to_os_string());
            (args, 2) // two passes resolve cross-references / table of contents
        }
    }
}

/// Pull a useful tail out of the TeX `.log` (from the first `!` error line), or
/// fall back to the captured stdout/stderr.
fn error_detail(log: &str, captured: &str) -> String {
    if let Some(pos) = log.find("\n!") {
        let snippet: String = log[pos + 1..].lines().take(15).collect::<Vec<_>>().join("\n");
        if !snippet.trim().is_empty() {
            return snippet;
        }
    }
    let source = if log.trim().is_empty() { captured } else { log };
    let mut tail: Vec<&str> = source.lines().rev().take(20).collect();
    tail.reverse();
    tail.join("\n")
}

/// Small FNV-1a hash for a unique temp-dir name (avoids a hashing dependency).
fn hash(s: &OsStr) -> u32 {
    let mut h: u32 = 0x811c_9dc5;
    for &b in s.as_encoded_bytes() {
        h ^= b as u32;
        h = h.wrapping_mul(0x0100_0193);
    }
    h
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn recipe_by_engine_name() {
        let out = Path::new("/tmp/o");
        let inp = OsStr::new("doc.tex");
        assert_eq!(build_args("tectonic", out, inp).1, 1);
        assert_eq!(build_args("/usr/bin/latexmk", out, inp).1, 1);
        assert_eq!(build_args("pdflatex", out, inp).1, 2);
        assert_eq!(build_args("xelatex", out, inp).1, 2);

        assert!(build_args("tectonic", out, inp).0.iter().any(|a| a == "--outdir"));
        assert!(build_args("pdflatex", out, inp).0.iter().any(|a| a == "-no-shell-escape"));
    }

    #[test]
    fn missing_engine_is_a_clean_error() {
        // A bogus engine must yield a clear "not found" error, never a panic.
        let tmp = std::env::temp_dir().join("folio-latex-missing-test.tex");
        std::fs::write(&tmp, b"\\documentclass{article}\\begin{document}hi\\end{document}").unwrap();
        let err = latex_to_pdf(&tmp, "folio-no-such-engine-zzz").unwrap_err();
        let _ = std::fs::remove_file(&tmp);
        assert!(err.to_string().contains("not found"), "got: {err}");
    }
}
