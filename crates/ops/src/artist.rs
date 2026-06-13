//! Derive a project's "artist" from where it sits on disk.
//!
//! The library is organized inconsistently (the user's words): most of it is
//! split by year, but some of it is split by artist. The scanner has no other
//! signal for artist (it is NOT in the `.als` file), so we read it from the
//! path, in two passes over the project's FULL absolute path:
//!
//! 1. **Marker convention (primary).** An `artists/` (or `artist/`) folder
//!    anywhere in the path means the very next folder is the artist —
//!    `…/artists/deebo/dahbby Project/` → **deebo**. We scan the whole path,
//!    not just the part below the scan root, so this fires no matter where you
//!    point the scan: at the library root, at `…/artists/`, or directly at the
//!    artist folder `…/artists/deebo/`.
//!
//! 2. **Positional fallback.** With no marker, walk the folders BETWEEN the
//!    scan root and the project, skip the obviously-temporal/generic ones
//!    (years, months, buckets like "Projects"), and take the first survivor —
//!    so `2024/march/Burial/song/` → **Burial**, while a pure `2024/march/song/`
//!    → `None` (correct — not filed under an artist).
//!
//! Best-effort by design: the scanner's explicit `--artist` override always
//! wins when the path can't be trusted.

use std::path::{Component, Path};

/// Folder names that introduce an artist directory (the NEXT segment is the
/// artist). Compared case-insensitively.
fn is_artist_marker(s: &str) -> bool {
    matches!(s.to_ascii_lowercase().as_str(), "artists" | "artist")
}

/// 4-digit calendar year (1900-2099) — a "2024"-style folder.
fn is_year(s: &str) -> bool {
    s.len() == 4
        && s.chars().all(|c| c.is_ascii_digit())
        && s.parse::<u32>().map_or(false, |y| (1900..=2099).contains(&y))
}

/// Month folder: an English month name/abbreviation, or a numeric month
/// (`1`..`12` / `01`..`12`). Numeric months are capped at 2 digits so a
/// 4-digit year never slips through here.
fn is_month(s: &str) -> bool {
    let l = s.to_ascii_lowercase();
    const MONTHS: [&str; 23] = [
        "january", "february", "march", "april", "may", "june", "july",
        "august", "september", "october", "november", "december",
        "jan", "feb", "mar", "apr", "jun", "jul", "aug", "sep", "oct", "nov", "dec",
    ];
    if MONTHS.contains(&l.as_str()) {
        return true;
    }
    if l.len() <= 2 {
        if let Ok(n) = l.parse::<u32>() {
            return (1..=12).contains(&n);
        }
    }
    false
}

/// Generic organizational folders that are never an artist name.
fn is_bucket(s: &str) -> bool {
    let l = s.to_ascii_lowercase();
    const BUCKETS: [&str; 26] = [
        "projects", "project", "ableton", "live", "music", "sets", "tracks",
        "ideas", "idea", "sketches", "beats", "loops", "stems", "samples",
        "bounces", "exports", "renders", "wip", "drafts", "draft", "misc",
        "unsorted", "new", "old", "random", "untitled",
    ];
    BUCKETS.contains(&l.as_str())
}

fn normal_segments(p: &Path) -> Vec<String> {
    p.components()
        .filter_map(|c| match c {
            Component::Normal(s) => Some(s.to_string_lossy().into_owned()),
            _ => None,
        })
        .collect()
}

/// Marker-only derivation over a full path: the segment right after an
/// `artists/` (or `artist/`) folder. Needs no scan root, so it works straight
/// from the project paths already stored in the catalog — this is what the
/// no-scan `reindex-artists` backfill uses. Returns `None` when there's no
/// marker (year-filed projects have no artist, or need a real scan whose root
/// gives the positional fallback something to measure against).
pub fn artist_from_full_path(project_dir: &Path) -> Option<String> {
    let comps = normal_segments(project_dir);
    if comps.is_empty() {
        return None;
    }
    // The final segment is the project folder itself — never the artist.
    let last = comps.len() - 1;
    for i in 0..last {
        if is_artist_marker(&comps[i]) && i + 1 < last {
            let cand = comps[i + 1].trim();
            if !cand.is_empty() && !is_year(cand) && !is_month(cand) && !is_bucket(cand) {
                return Some(cand.to_string());
            }
        }
    }
    None
}

/// Infer the artist for a project at `project_dir` (an absolute path)
/// discovered under `root`. Returns `None` when nothing in the path looks like
/// an artist (e.g. a pure `year/month/project` layout).
pub fn infer_artist(root: &Path, project_dir: &Path) -> Option<String> {
    // Pass 1 — marker convention over the FULL path.
    if let Some(a) = artist_from_full_path(project_dir) {
        return Some(a);
    }

    // Pass 2 — positional fallback over the segments below the scan root.
    if let Ok(rel) = project_dir.strip_prefix(root) {
        let mut segs = normal_segments(rel);
        segs.pop(); // drop the project folder
        for seg in segs {
            let t = seg.trim();
            if t.is_empty() || is_year(t) || is_month(t) || is_bucket(t) || is_artist_marker(t) {
                continue;
            }
            return Some(t.to_string());
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn infer(root: &str, dir: &str) -> Option<String> {
        infer_artist(&PathBuf::from(root), &PathBuf::from(dir))
    }

    #[test]
    fn marker_when_scanning_artist_folder_directly() {
        // The user's real case: scan points AT the artist folder, so the only
        // thing below the root is the project. The full-path marker still wins.
        let root = "/Users/c/Documents/Projects (icloud)/artists/deebo";
        let dir = "/Users/c/Documents/Projects (icloud)/artists/deebo/dahbby Project";
        assert_eq!(infer(root, dir), Some("deebo".to_string()));
    }

    #[test]
    fn marker_when_scanning_whole_library() {
        // Scan the library root: must pick `deebo`, NOT the `artists` folder.
        let root = "/Users/c/Documents/Projects (icloud)";
        let dir = "/Users/c/Documents/Projects (icloud)/artists/deebo/dahbby Project";
        assert_eq!(infer(root, dir), Some("deebo".to_string()));
    }

    #[test]
    fn marker_when_scanning_artists_parent() {
        let root = "/Users/c/Documents/Projects (icloud)/artists";
        let dir = "/Users/c/Documents/Projects (icloud)/artists/deebo/dahbby Project";
        assert_eq!(infer(root, dir), Some("deebo".to_string()));
    }

    #[test]
    fn marker_skips_date_after_artists_folder() {
        // `artists/2024/...` shouldn't call "2024" an artist; fall through.
        let root = "/lib";
        let dir = "/lib/artists/2024/song Project";
        assert_eq!(infer(root, dir), None);
    }

    #[test]
    fn artist_under_year_month() {
        // 2024/march/<artist>/<project>
        assert_eq!(
            infer("/lib", "/lib/2024/march/Burial/Untrue Project"),
            Some("Burial".to_string())
        );
    }

    #[test]
    fn artist_at_top_level() {
        assert_eq!(
            infer("/lib", "/lib/Burial/Untrue Project"),
            Some("Burial".to_string())
        );
    }

    #[test]
    fn pure_year_month_has_no_artist() {
        assert_eq!(infer("/lib", "/lib/2024/march/Untrue Project"), None);
    }

    #[test]
    fn project_directly_at_root_has_no_artist() {
        assert_eq!(infer("/lib", "/lib/Untrue Project"), None);
    }

    #[test]
    fn bucket_folders_are_skipped() {
        assert_eq!(
            infer("/lib", "/lib/Projects/Burial/Untrue Project"),
            Some("Burial".to_string())
        );
        // All temporal/bucket -> unknown.
        assert_eq!(infer("/lib", "/lib/2024/Projects/Untrue Project"), None);
    }

    #[test]
    fn numeric_month_skipped_but_year_is_not_a_month() {
        // "05" is a month folder, skipped; the artist follows.
        assert_eq!(
            infer("/lib", "/lib/2024/05/Aphex/Set"),
            Some("Aphex".to_string())
        );
    }

    #[test]
    fn root_mismatch_yields_none() {
        assert_eq!(infer("/other", "/lib/Burial/Set"), None);
    }
}
