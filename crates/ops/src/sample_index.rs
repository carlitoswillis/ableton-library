//! SampleIndex: one-pass, fully recursive audio-file index over search roots
//! (Ableton Places + project folders), with Live-style relaxed lookup.
//!
//! Replaces the old per-missing-file depth-5 walks (backlog item 2026-06-13:
//! "strict exact filename match only, 5-depth limit — fails on moved files
//! that Live's browser finds via fuzzy search").
//!
//! Lookup tiers (best wins, mirroring Live's missing-media search):
//!   1. exact filename (stem + extension)
//!   2. same stem, different audio extension ("alternative file type" —
//!      the missing .aif was re-rendered as .wav)
//!   3. fuzzy stem: normalized/squashed containment or similarity >= 0.75
//!      ("Kick 808 " vs "kick_808", "Vocal Take 3 edit" vs "Vocal Take 3")

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use previews::matching::{normalize, score};

/// Directories never worth indexing for samples.
const SKIP_DIRS: &[&str] = &["Backup", "Ableton Project Info", "node_modules"];

fn audio_ext(p: &Path) -> Option<String> {
    let ext = p.extension()?.to_string_lossy().to_lowercase();
    if previews::AUDIO_EXTS.contains(&ext.as_str()) {
        Some(ext)
    } else {
        None
    }
}

fn squash(s: &str) -> String {
    s.replace(' ', "")
}

#[derive(Debug, Clone)]
struct Entry {
    path: PathBuf,
    ext: String,
    mtime: u64,
}

#[derive(Debug, Default)]
pub struct SampleIndex {
    /// normalized stem -> entries (cheap exact/alt-ext lookups)
    by_stem: HashMap<String, Vec<Entry>>,
}

/// Hard budgets: a Place can be enormous (or an iCloud tree where directory
/// enumeration itself crawls). The index must NEVER hang the render path —
/// better a truncated index than a stuck app (lesson re-learned 2026-06-13).
const MAX_WALK_ENTRIES: usize = 250_000;
const MAX_WALK_TIME: std::time::Duration = std::time::Duration::from_secs(20);

impl SampleIndex {
    /// Walk every root ONCE, fully recursively but BUDGETED (entry count +
    /// wall clock). Logs each root before walking so a slow one is
    /// identifiable in the log, not a silent hang.
    pub fn build(roots: &[PathBuf], log: &mut dyn FnMut(String)) -> Self {
        let mut idx = SampleIndex::default();
        let deadline = std::time::Instant::now() + MAX_WALK_TIME;
        let mut visited = 0usize;
        'roots: for root in roots {
            let t0 = std::time::Instant::now();
            log(format!("indexing {} …", root.display()));
            for entry in walkdir::WalkDir::new(root)
                .follow_links(false)
                .into_iter()
                .filter_entry(|e| {
                    let name = e.file_name().to_string_lossy();
                    !name.starts_with('.') && !SKIP_DIRS.contains(&name.as_ref())
                })
                .filter_map(|e| e.ok())
            {
                visited += 1;
                if visited % 2048 == 0 && std::time::Instant::now() > deadline {
                    log(format!(
                        "index budget hit ({} entries / {:?}) — truncating at {}",
                        visited, MAX_WALK_TIME, root.display()
                    ));
                    break 'roots;
                }
                if visited > MAX_WALK_ENTRIES {
                    log(format!(
                        "index entry cap hit ({MAX_WALK_ENTRIES}) — truncating at {}",
                        root.display()
                    ));
                    break 'roots;
                }
                let p = entry.path();
                if !entry.file_type().is_file() {
                    continue;
                }
                let Some(ext) = audio_ext(p) else { continue };
                let Some(stem) = p.file_stem().map(|s| s.to_string_lossy().into_owned())
                else {
                    continue;
                };
                let norm = normalize(&stem);
                if norm.is_empty() {
                    continue;
                }
                idx.by_stem.entry(norm).or_default().push(Entry {
                    path: p.to_path_buf(),
                    ext,
                    mtime: entry
                        .metadata()
                        .ok()
                        .and_then(|m| m.modified().ok())
                        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
                        .map(|d| d.as_secs())
                        .unwrap_or(0),
                });
            }
            log(format!("  …done in {:.1}s", t0.elapsed().as_secs_f32()));
        }
        idx
    }

    pub fn len(&self) -> usize {
        self.by_stem.values().map(|v| v.len()).sum()
    }

    pub fn is_empty(&self) -> bool {
        self.by_stem.is_empty()
    }

    /// Find candidates for a missing filename, best tier first, newest first
    /// within a tier. Returned paths all exist (indexed from disk).
    pub fn find(&self, missing_filename: &str) -> Vec<PathBuf> {
        let missing = Path::new(missing_filename);
        let want_ext = missing
            .extension()
            .map(|e| e.to_string_lossy().to_lowercase())
            .unwrap_or_default();
        let stem = missing
            .file_stem()
            .map(|s| s.to_string_lossy().into_owned())
            .unwrap_or_else(|| missing_filename.to_string());
        let norm = normalize(&stem);
        let norm_sq = squash(&norm);

        // (tier, Reverse(mtime)) sort key; tier 0 best.
        let mut hits: Vec<(u8, u64, PathBuf)> = Vec::new();

        // Tiers 0/1: exact normalized stem.
        if let Some(entries) = self.by_stem.get(&norm) {
            for e in entries {
                let tier = if e.ext == want_ext { 0 } else { 1 };
                hits.push((tier, e.mtime, e.path.clone()));
            }
        }

        // Tier 2: fuzzy — squashed equality/containment or similarity >= 0.75.
        // Only when the strict tiers found nothing (Live behaves the same:
        // exact match wins outright).
        if hits.is_empty() && !norm.is_empty() {
            for (key, entries) in &self.by_stem {
                let key_sq = squash(key);
                let close = key_sq == norm_sq
                    || (norm_sq.len() >= 6
                        && (key_sq.contains(&norm_sq) || norm_sq.contains(&key_sq)))
                    || score(key, &norm) >= 0.75;
                if close {
                    for e in entries {
                        hits.push((2, e.mtime, e.path.clone()));
                    }
                }
            }
        }

        hits.sort_by_key(|(tier, mtime, _)| (*tier, std::cmp::Reverse(*mtime)));
        hits.into_iter().map(|(_, _, p)| p).collect()
    }

    /// All living parent dirs containing something matching `filename`
    /// (for folder-move voting).
    pub fn parents_of(&self, filename: &str) -> Vec<PathBuf> {
        self.find(filename)
            .into_iter()
            .filter_map(|p| p.parent().map(|d| d.to_path_buf()))
            .collect()
    }
}

/// Convenience: index built from Ableton Places + extra roots, deduped and
/// sanity-filtered (a Place that is "/", a volume root, or the home dir
/// would mean walking the world — skipped with a log line).
pub fn build_search_index(extra_roots: &[PathBuf], log: &mut dyn FnMut(String)) -> SampleIndex {
    let mut roots = crate::places::get_ableton_places();
    roots.extend(extra_roots.iter().cloned());
    roots.sort();
    roots.dedup();
    // Drop roots nested inside another root (avoid double indexing).
    let roots_c = roots.clone();
    roots.retain(|r| !roots_c.iter().any(|o| o != r && r.starts_with(o)));
    // Drop absurdly broad roots.
    let home = dirs::home_dir();
    roots.retain(|r| {
        let too_broad = r == Path::new("/")
            || r.components().count() <= 2
            || home.as_deref() == Some(r.as_path());
        if too_broad {
            log(format!("skipping over-broad search root {}", r.display()));
        }
        !too_broad
    });
    let t0 = std::time::Instant::now();
    let idx = SampleIndex::build(&roots, log);
    log(format!(
        "sample index: {} audio files across {} root(s) in {:.1}s",
        idx.len(),
        roots.len(),
        t0.elapsed().as_secs_f32()
    ));
    idx
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tiered_lookup_exact_altext_fuzzy() {
        let dir = std::env::temp_dir().join("alib_sample_index_test");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("kick_808.wav"), b"x").unwrap();
        std::fs::write(dir.join("Snare Hit.aif"), b"x").unwrap();
        let mut log = |_s: String| {};
        let idx = SampleIndex::build(&[dir.clone()], &mut log);
        assert!(!idx.find("kick_808.wav").is_empty(), "exact");
        assert!(!idx.find("Snare Hit.wav").is_empty(), "alternative extension");
        assert!(!idx.find("Kick 808.wav").is_empty(), "fuzzy underscore/space");
        assert!(idx.find("totally unrelated thing.wav").is_empty());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
