//! Name-similarity matching: loose render filenames -> catalog sets.
//!
//! Scoring (highest wins):
//!   1.00  normalized stems identical
//!   0.85  one normalized stem is a word-boundary prefix of the other
//!   0..1  token Jaccard overlap (only if >= threshold)
//! A render may also match a PROJECT name; that attaches to the project's
//! single set (x0.9) or, if multi-set, at project level (low confidence).

use std::collections::HashSet;

/// Noise tokens that say nothing about which song a bounce is.
const STOPWORDS: &[&str] = &[
    "final", "finals", "master", "mastered", "mix", "mixdown", "bounce",
    "bounced", "export", "exported", "render", "demo", "rough", "draft",
    "edit", "full", "song", "track", "new", "old", "copy",
];

/// Normalize a name for comparison: lowercase, strip bracketed chunks,
/// punctuation -> spaces, drop stopwords / vN / bpm / key-ish tokens.
pub fn normalize(s: &str) -> String {
    let lower = s.to_lowercase();
    // strip (...) and [...] chunks (often "(prod. x)" "[2026-05-29 1153]")
    let mut cleaned = String::with_capacity(lower.len());
    let mut depth = 0i32;
    for c in lower.chars() {
        match c {
            '(' | '[' | '{' => depth += 1,
            ')' | ']' | '}' => depth = (depth - 1).max(0),
            _ if depth == 0 => {
                cleaned.push(if c.is_alphanumeric() { c } else { ' ' })
            }
            _ => {}
        }
    }
    let tokens: Vec<&str> = cleaned
        .split_whitespace()
        .filter(|t| {
            if STOPWORDS.contains(t) {
                return false;
            }
            // v2, v10 ...
            if t.len() >= 2
                && t.starts_with('v')
                && t[1..].chars().all(|c| c.is_ascii_digit())
            {
                return false;
            }
            // 163bpm / bpm
            if *t == "bpm" || (t.ends_with("bpm") && t[..t.len() - 3].chars().all(|c| c.is_ascii_digit())) {
                return false;
            }
            true
        })
        .collect();
    tokens.join(" ")
}

fn tokens(s: &str) -> HashSet<&str> {
    s.split_whitespace().collect()
}

/// Similarity of two ALREADY-normalized names, 0..1.
pub fn score(a: &str, b: &str) -> f64 {
    if a.is_empty() || b.is_empty() {
        return 0.0;
    }
    if a == b {
        return 1.0;
    }
    let (short, long) = if a.len() <= b.len() { (a, b) } else { (b, a) };
    if long.starts_with(short)
        && long[short.len()..].starts_with(' ')
    {
        return 0.85;
    }
    let ta = tokens(a);
    let tb = tokens(b);
    let inter = ta.intersection(&tb).count() as f64;
    let union = ta.union(&tb).count() as f64;
    if union == 0.0 {
        0.0
    } else {
        inter / union
    }
}

/// A catalog set the matcher can target.
pub struct SetCandidate {
    pub set_id: i64,
    pub project_id: i64,
    /// normalized .als file stem
    pub norm_stem: String,
    /// normalized project name (without trailing "project" token)
    pub norm_project: String,
    /// how many sets the project has (for project-name fallback)
    pub project_set_count: usize,
}

pub enum MatchTarget {
    /// Attach to a specific set.
    Set { set_id: i64, project_id: i64 },
    /// Ambiguous: attach at project level.
    Project { project_id: i64 },
}

pub struct Match {
    pub target: MatchTarget,
    pub confidence: f64,
}

/// Best match for one render stem against all candidates, if any clears
/// `threshold`. Set-name matches beat project-name matches.
pub fn best_match(render_stem_norm: &str, cands: &[SetCandidate], threshold: f64) -> Option<Match> {
    let mut best_set: Option<(f64, &SetCandidate)> = None;
    let mut best_proj: Option<(f64, &SetCandidate)> = None;
    for c in cands {
        let s = score(render_stem_norm, &c.norm_stem);
        if best_set.map_or(true, |(b, _)| s > b) {
            best_set = Some((s, c));
        }
        let p = score(render_stem_norm, &c.norm_project);
        if best_proj.map_or(true, |(b, _)| p > b) {
            best_proj = Some((p, c));
        }
    }
    if let Some((s, c)) = best_set {
        if s >= threshold {
            return Some(Match {
                target: MatchTarget::Set { set_id: c.set_id, project_id: c.project_id },
                confidence: s,
            });
        }
    }
    if let Some((p, c)) = best_proj {
        if p >= threshold {
            return Some(if c.project_set_count == 1 {
                Match {
                    target: MatchTarget::Set { set_id: c.set_id, project_id: c.project_id },
                    confidence: p * 0.9,
                }
            } else {
                Match {
                    target: MatchTarget::Project { project_id: c.project_id },
                    confidence: p * 0.5,
                }
            });
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalizes_noise() {
        assert_eq!(normalize("wanna be your FINAL v2"), "wanna be your");
        assert_eq!(normalize("cult (prod. barlitxs) 145bpm"), "cult");
        assert_eq!(normalize("522 idea"), "522 idea");
    }

    #[test]
    fn exact_beats_prefix() {
        // "wanna be your" must match the set "wanna be your", not "wanna be your2"
        assert!(score("wanna be your", "wanna be your") > score("wanna be your", "wanna be your2"));
    }
}
