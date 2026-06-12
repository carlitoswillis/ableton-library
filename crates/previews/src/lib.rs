//! previews: render discovery, name matching, and waveform peak extraction.
//!
//! Design (see ai/ARCHITECTURE.md, Preview Service):
//! - Renders are SCATTERED (user decision 2026-06-11: do not rely on project
//!   folders). Discovery hunts user-chosen roots and matches loose audio
//!   files to catalog sets by normalized name similarity. Files are never
//!   moved — only referenced.
//! - Every match carries a confidence; the export worker will later feed the
//!   same table at confidence 1.0.

pub mod matching;
pub mod peaks;

use std::path::{Path, PathBuf};

use walkdir::WalkDir;

pub const AUDIO_EXTS: &[&str] = &["wav", "mp3", "aif", "aiff", "flac", "m4a"];

/// Directories whose audio is never a render (samples, Live internals).
const EXCLUDED_DIRS: &[&str] = &["Samples", "Backup", "Ableton Project Info", "node_modules"];

/// Files smaller than this are presumed one-shots/samples, not bounces.
pub const MIN_RENDER_BYTES: u64 = 1_000_000;

#[derive(Debug, Clone)]
pub struct RenderFile {
    pub path: PathBuf,
    /// File stem, e.g. "wanna be your final v2".
    pub stem: String,
    pub size: u64,
}

fn is_audio(path: &Path) -> bool {
    path.extension()
        .map(|e| e.to_string_lossy().to_lowercase())
        .map_or(false, |e| AUDIO_EXTS.contains(&e.as_str()))
}

fn is_project_dir(path: &Path) -> bool {
    if let Ok(entries) = std::fs::read_dir(path) {
        for entry in entries.filter_map(|e| e.ok()) {
            if entry.path().extension().map_or(false, |ext| ext == "als") {
                return true;
            }
        }
    }
    false
}

/// Hunt one or more roots for candidate render files.
pub fn discover_renders(roots: &[PathBuf]) -> std::io::Result<Vec<RenderFile>> {
    let mut out = Vec::new();
    for root in roots {
        for entry in WalkDir::new(root)
            .into_iter()
            .filter_entry(|e| {
                let name = e.file_name().to_string_lossy();
                if name.starts_with('.') || EXCLUDED_DIRS.contains(&name.as_ref()) {
                    return false;
                }
                let p = e.path();
                if e.file_type().is_dir() && p != root && is_project_dir(&p) {
                    return false;
                }
                true
            })
            .filter_map(|e| e.ok())
        {
            let p = entry.path();
            if !entry.file_type().is_file() || !is_audio(p) {
                continue;
            }
            let size = entry.metadata().map(|m| m.len()).unwrap_or(0);
            if size < MIN_RENDER_BYTES {
                continue;
            }
            out.push(RenderFile {
                stem: p
                    .file_stem()
                    .map(|s| s.to_string_lossy().into_owned())
                    .unwrap_or_default(),
                path: p.to_path_buf(),
                size,
            });
        }
    }
    Ok(out)
}
