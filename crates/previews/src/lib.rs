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
pub mod sketch;

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

/// Hunt one or more roots for candidate render files.
pub fn discover_renders(roots: &[PathBuf], max_depth: Option<usize>) -> std::io::Result<Vec<RenderFile>> {
    let mut out = Vec::new();
    for root in roots {
        let mut walk = WalkDir::new(root);
        if let Some(depth) = max_depth {
            walk = walk.max_depth(depth);
        }
        for entry in walk
            .into_iter()
            .filter_entry(|e| {
                let name = e.file_name().to_string_lossy();
                !name.starts_with('.') && !EXCLUDED_DIRS.contains(&name.as_ref())
            })
            .filter_map(|e| e.ok())
        {
            let p = entry.path();
            if !entry.file_type().is_file() || !is_audio(p) {
                continue;
            }
            let size = entry.metadata().map(|m| m.len()).unwrap_or(0);
            let ext = p.extension()
                .map(|e| e.to_string_lossy().to_lowercase())
                .unwrap_or_default();
            let min_bytes = if ext == "mp3" || ext == "m4a" {
                100_000 // 100 KB for compressed formats
            } else {
                MIN_RENDER_BYTES // 1 MB for uncompressed formats (wav, aiff, etc.)
            };
            if size < min_bytes {
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
