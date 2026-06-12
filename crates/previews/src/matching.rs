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

/// Normalize a name for comparison. Philosophy (user decision 2026-06-11):
/// bpm / key / "(prod. x)" are often PART of project names, so they are
/// distinguishing signal — keep them, normalize their FORM instead:
/// - lowercase; punctuation -> spaces (parenthesized content kept)
/// - [bracketed] chunks stripped (Live backup timestamps, never identity)
/// - digit/letter boundaries split ("145bpm" -> "145 bpm") so both sides
///   tokenize identically
/// - only true noise dropped: stopwords (final/master/...) and vN tokens
pub fn normalize(s: &str) -> String {
    let lower = s.to_lowercase();
    // strip only [...] chunks; keep (...) content (often "(prod. x)" = identity)
    let mut cleaned = String::with_capacity(lower.len());
    let mut depth = 0i32;
    let mut prev: Option<char> = None;
    for c in lower.chars() {
        match c {
            '[' | '{' => depth += 1,
            ']' | '}' => depth = (depth - 1).max(0),
            _ if depth == 0 => {
                let out = if c.is_alphanumeric() { c } else { ' ' };
                // split digit<->letter boundaries: 145bpm -> 145 bpm
                if let Some(p) = prev {
                    if (p.is_ascii_digit() && out.is_alphabetic())
                        || (p.is_alphabetic() && out.is_ascii_digit())
                    {
                        cleaned.push(' ');
                    }
                }
                cleaned.push(out);
                prev = if out == ' ' { None } else { Some(out) };
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
    fn normalizes_noise_keeps_identity() {
        assert_eq!(normalize("wanna be your FINAL v2"), "wanna be your");
        // bpm/prod/key are identity, kept; form normalized (145bpm -> 145 bpm)
        assert_eq!(normalize("cult (prod. barlitxs) 145bpm"), "cult prod barlitxs 145 bpm");
        assert_eq!(
            normalize("sky high 2020 (prod. barlitxs) 88 bpm Db minor"),
            "sky high 2020 prod barlitxs 88 bpm db minor"
        );
        // bracketed backup timestamps are never identity
        assert_eq!(normalize("king st [2026-05-29 202646]"), "king st");
        assert_eq!(normalize("522 idea"), "522 idea");
    }

    #[test]
    fn bpm_in_both_names_disambiguates() {
        // a render carrying the bpm should prefer the matching long-form name
        let with = score(
            &normalize("cult 145bpm"),
            &normalize("cult (prod. barlitxs) 145 bpm G minor"),
        );
        let without = score(&normalize("cult 145bpm"), &normalize("cult 152 bpm A minor"));
        assert!(with > without);
    }

    #[test]
    fn exact_beats_prefix() {
        // "wanna be your" must match the set "wanna be your", not "wanna be your2"
        assert!(score("wanna be your", "wanna be your") > score("wanna be your", "wanna be your2"));
    }
}
