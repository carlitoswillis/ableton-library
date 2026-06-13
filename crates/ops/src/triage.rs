//! M4a renderability triage: how well will a set bounce on THIS machine?
//!
//! Field findings that motivated this (2026-06-12): old projects render
//! slowly and incompletely — missing plugins = silent tracks, iCloud-evicted
//! samples stall bounces, moved samples force Live's slow relocate scan.
//! Triage lets the export worker do easy sets first and stamp every render
//! with an honest fidelity report.

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::Result;
use rusqlite::Connection;

use previews::matching::{normalize, score};

/// Normalized names of plugins installed on this machine, from recursive
/// bundle-filename scans of the plugin roots and vendor app folders.
#[derive(Debug, Default, Clone, serde::Serialize, serde::Deserialize)]
pub struct InstalledPlugins {
    pub names: Vec<String>, // normalized
}

/// An inventory smaller than this is treated as a failed build: plugin
/// checks then abstain rather than mass-report missing.
pub const MIN_PLAUSIBLE_INVENTORY: usize = 10;

/// The standard macOS plugin roots, scanned recursively (user suggestion
/// 2026-06-12: just take all of /Library/Audio/Plug-Ins, system + user).
fn plugin_file_dirs() -> Vec<PathBuf> {
    let mut dirs: Vec<PathBuf> = vec![PathBuf::from("/Library/Audio/Plug-Ins")];
    if let Some(home) = dirs::home_dir() {
        dirs.push(home.join("Library/Audio/Plug-Ins"));
    }
    dirs
}

/// Recursively collect plugin bundle stems. RECURSIVE because vendors love
/// subfolders (/VST3/iZotope/..., vendor dirs inside Components) — a flat
/// read_dir missed those and caused false "missing plugin" reports
/// (user-confirmed 2026-06-12).
fn scan_plugin_bundles(names: &mut HashSet<String>) {
    const BUNDLE_EXTS: &[&str] = &["component", "vst3", "vst", "bundle", "clap"];
    // Vendor app folders that carry per-plugin bundles or helpfully named
    // content (user-confirmed vendors first: Waves, Soundtoys, KORG).
    // Deliberately NOT all of /Applications — walking every .app is slow.
    const VENDOR_DIRS: &[&str] = &[
        "/Applications/Waves",
        "/Applications/Soundtoys",
        "/Applications/KORG",
        "/Applications/Native Instruments",
        "/Applications/iZotope",
        "/Applications/Plugin Alliance",
        "/Applications/Arturia",
        "/Applications/FabFilter",
        "/Applications/Eventide",
        "/Applications/Universal Audio",
    ];
    let mut roots = plugin_file_dirs();
    roots.extend(VENDOR_DIRS.iter().map(PathBuf::from));
    for root in roots {
        for entry in walkdir::WalkDir::new(&root)
            .max_depth(4)
            .into_iter()
            .filter_map(|e| e.ok())
        {
            let p = entry.path();
            if let Some(ext) = p.extension().map(|e| e.to_string_lossy().to_lowercase()) {
                if BUNDLE_EXTS.contains(&ext.as_str()) {
                    if let Some(stem) = p.file_stem() {
                        names.insert(normalize(&stem.to_string_lossy()));
                    }
                }
            }
        }
    }
}

/// Build the inventory: recursive bundle-stem scan of the macOS plugin
/// roots + vendor app folders. Milliseconds, no auval. (auval was removed
/// 2026-06-12: it loads/validates every plugin and proved a black hole on
/// the user's machine — filenames + vendor dirs cover what triage needs.)
pub fn installed_plugins() -> InstalledPlugins {
    let mut names: HashSet<String> = HashSet::new();
    scan_plugin_bundles(&mut names);
    InstalledPlugins { names: names.into_iter().filter(|n| !n.is_empty()).collect() }
}

/// Back-compat alias (quick IS the inventory now).
pub fn installed_plugins_quick() -> InstalledPlugins {
    installed_plugins()
}

/// Is a plugin (by .als display name + manufacturer) present on this machine?
/// Exact normalized hit, containment, or fuzzy score >= 0.7 — generous on
/// purpose: a FALSE "missing" wrongly deprioritizes a renderable set.
pub fn plugin_installed(installed: &InstalledPlugins, name: &str, manufacturer: Option<&str>) -> bool {
    // Apple's stock Audio Units ship with macOS itself (no scannable bundle)
    // — they are always present. Catch via manufacturer or the AUXxx naming.
    if manufacturer.map_or(false, |m| m.eq_ignore_ascii_case("apple")) {
        return true;
    }
    let mut chars = name.chars();
    if name.starts_with("AU")
        && chars.nth(2).map_or(false, |c| c.is_ascii_uppercase() || c.is_ascii_lowercase())
        && !name.contains(' ')
    {
        return true; // AUDelay, AUGraphicEQ, ...
    }

    let n = normalize(name);
    if n.is_empty() {
        return true; // unidentifiable -> don't penalize
    }
    // SPACE-SQUASHED comparison: vendors concatenate bundle names
    // ("LittlePlate.bundle", "SieQ.bundle") while the .als spells them out
    // ("Little Plate", "Sie-Q"). Caused user-confirmed false missings.
    let squash = |s: &str| s.replace(' ', "");
    let n_sq = squash(&n);
    // Also try "manufacturer name" — inventory stems often carry the vendor
    // prefix ("FabFilter Pro-Q 3.vst3").
    let with_manu = manufacturer.map(|m| normalize(&format!("{m} {name}")));
    installed.names.iter().any(|have| {
        let have_sq = squash(have);
        have == &n
            || have.contains(&n)
            || n.contains(have.as_str())
            || have_sq == n_sq
            || have_sq.contains(&n_sq)
            || n_sq.contains(&have_sq)
            || score(have, &n) >= 0.7
            || with_manu
                .as_ref()
                .map_or(false, |wm| have == wm || have.contains(wm.as_str()) || score(have, wm) >= 0.7)
    })
}

/// One sample's disk state.
#[derive(Debug, PartialEq, serde::Serialize)]
pub enum SampleState {
    Present,
    /// iCloud placeholder exists (".name.icloud") — downloadable.
    Evicted,
    Missing,
}

/// Evicted vs missing: an evicted file's dir contains ".{filename}.icloud".
pub fn sample_state(path: &str) -> SampleState {
    let p = Path::new(path);
    if p.exists() {
        return SampleState::Present;
    }
    if let (Some(dir), Some(name)) = (p.parent(), p.file_name()) {
        let placeholder = dir.join(format!(".{}.icloud", name.to_string_lossy()));
        if placeholder.exists() {
            return SampleState::Evicted;
        }
    }
    SampleState::Missing
}

/// The fidelity report stored (as JSON) on jobs and worker previews.
#[derive(Debug, Default, serde::Serialize)]
pub struct Renderability {
    /// 0..1 — 1.0 renders perfectly as far as we can tell.
    pub score: f64,
    pub plugins_total: usize,
    pub missing_plugins: Vec<String>,
    pub samples_total: usize,
    pub samples_missing: usize,
    pub samples_evicted: usize,
}

/// Score a set from catalog data + this machine's plugin inventory.
/// Penalties: missing plugin -0.12 each (cap 0.6) — silent tracks are the
/// worst failure; missing sample -0.04 each (cap 0.3); evicted sample -0.01
/// each (cap 0.1) — recoverable via pre-flight download.
pub fn renderability(
    conn: &Connection,
    set_id: i64,
    installed: &InstalledPlugins,
) -> Result<Renderability> {
    let (plugins, samples_raw) = indexer::set_render_inputs(conn, set_id)?;
    let samples: Vec<String> = samples_raw.into_iter().map(|(p, _)| p).collect();

    let mut r = Renderability {
        score: 1.0,
        plugins_total: plugins.len(),
        samples_total: samples.len(),
        ..Default::default()
    };

    // Abstain if the inventory build failed: a bogus tiny inventory would
    // mass-report every plugin missing and tank all scores.
    let inventory_usable = installed.names.len() >= MIN_PLAUSIBLE_INVENTORY;
    let mut seen_missing: HashSet<String> = HashSet::new();
    if inventory_usable {
        for (_kind, name, manu) in &plugins {
            let display = name.clone().unwrap_or_default();
            if display.is_empty() {
                continue;
            }
            if !plugin_installed(installed, &display, manu.as_deref())
                && seen_missing.insert(display.clone())
            {
                r.missing_plugins.push(display);
            }
        }
    }
    for s in &samples {
        match sample_state(s) {
            SampleState::Present => {}
            SampleState::Evicted => r.samples_evicted += 1,
            SampleState::Missing => r.samples_missing += 1,
        }
    }

    let plugin_penalty = (r.missing_plugins.len() as f64 * 0.12).min(0.6);
    let missing_penalty = (r.samples_missing as f64 * 0.04).min(0.3);
    let evicted_penalty = (r.samples_evicted as f64 * 0.01).min(0.1);
    r.score = (1.0 - plugin_penalty - missing_penalty - evicted_penalty).clamp(0.0, 1.0);
    Ok(r)
}

/// Score every pending job that hasn't been scored yet. Returns how many
/// were scored. One set failing must never strand the rest unscored —
/// per-job errors are reported via `log` and skipped.
pub fn score_pending_jobs(
    conn: &Connection,
    installed: &InstalledPlugins,
    log: &mut dyn FnMut(String),
) -> Result<usize> {
    let mut stmt = conn.prepare(
        "SELECT id, set_id FROM export_jobs WHERE status = 'pending' AND score IS NULL",
    )?;
    let jobs: Vec<(i64, i64)> = stmt
        .query_map([], |r| Ok((r.get(0)?, r.get(1)?)))?
        .collect::<rusqlite::Result<_>>()?;
    let mut scored = 0usize;
    for (job_id, set_id) in jobs {
        let result = renderability(conn, set_id, installed).and_then(|r| {
            indexer::set_export_job_triage(conn, job_id, r.score, &serde_json::to_string(&r)?)
        });
        match result {
            Ok(()) => scored += 1,
            Err(e) => log(format!("triage failed for set {set_id}: {e}")),
        }
    }
    Ok(scored)
}

/// Resolve MISSING samples before a render by using the catalog as the
/// search index (the "Live auto-search, but we already know" move,
/// user idea 2026-06-12): if a dead path's filename exists at a live path
/// referenced by any indexed set, symlink old -> new so Live finds it
/// without a relocate scan. Returns (linked, unresolved).
pub fn relink_missing_samples(
    conn: &Connection,
    set_id: i64,
    log: &mut dyn FnMut(String),
) -> Result<(usize, usize)> {
    use std::collections::HashMap;
    let (_, samples_raw) = indexer::set_render_inputs(conn, set_id)?;
    let samples: Vec<String> = samples_raw.into_iter().map(|(p, _)| p).collect();

    // Group missing samples by their (dead) parent dir: when a whole folder
    // moved (sample packs!), ONE dir symlink beats N file links (user
    // request 2026-06-12).
    let mut groups: HashMap<PathBuf, Vec<String>> = HashMap::new();
    for p in &samples {
        if sample_state(p) != SampleState::Missing {
            continue;
        }
        let pb = Path::new(p);
        if let (Some(parent), Some(name)) = (pb.parent(), pb.file_name()) {
            groups
                .entry(parent.to_path_buf())
                .or_default()
                .push(name.to_string_lossy().into_owned());
        }
    }

    let mtime_secs = |c: &Path| {
        std::fs::metadata(c)
            .and_then(|m| m.modified())
            .ok()
            .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
            .map(|d| d.as_secs())
            .unwrap_or(0)
    };

    let (mut linked, mut unresolved) = (0usize, 0usize);
    let places = crate::places::get_ableton_places();
    for (dead_parent, basenames) in groups {
        // FOLDER-level attempt: find a living dir that contains ALL of this
        // group's filenames (vote via the catalog, verify on disk).
        let mut votes: HashMap<PathBuf, usize> = HashMap::new();
        for name in &basenames {
            for c in indexer::sample_paths_by_basename(conn, name)? {
                let cp = Path::new(&c);
                if cp.exists() {
                    if let Some(par) = cp.parent() {
                        if par != dead_parent.as_path() {
                            *votes.entry(par.to_path_buf()).or_default() += 1;
                        }
                    }
                }
            }
        }
        let mut ranked: Vec<(PathBuf, usize)> = votes.into_iter().collect();
        ranked.sort_by_key(|(_, v)| std::cmp::Reverse(*v));

        let mut full_match = ranked
            .iter()
            .map(|(cand, _)| cand)
            .find(|cand| basenames.iter().all(|n| cand.join(n).exists()))
            .cloned();

        if full_match.is_none() {
            // Search in Places
            for place in &places {
                for entry in walkdir::WalkDir::new(place)
                    .max_depth(5)
                    .into_iter()
                    .filter_map(|e| e.ok())
                {
                    if entry.file_type().is_dir() {
                        let cand = entry.path();
                        if basenames.iter().all(|n| cand.join(n).exists()) {
                            full_match = Some(cand.to_path_buf());
                            break;
                        }
                    }
                }
                if full_match.is_some() {
                    break;
                }
            }
        }

        // Folder symlink only works if the dead path doesn't exist as a real
        // dir (and we must NOT create_dir_all it — that would block this).
        if let Some(target) = &full_match {
            if !dead_parent.exists() {
                let ok = dead_parent
                    .parent()
                    .map_or(false, |gp| std::fs::create_dir_all(gp).is_ok());
                if ok && std::os::unix::fs::symlink(target, &dead_parent).is_ok() {
                    linked += basenames.len();
                    log(format!(
                        "relinked FOLDER {} -> {} ({} samples, one link)",
                        dead_parent.display(),
                        target.display(),
                        basenames.len()
                    ));
                    continue;
                }
            }
        }

        // Per-file fallback (dead dir really exists, or no folder fully matches).
        for name in &basenames {
            let mut cands: Vec<PathBuf> = indexer::sample_paths_by_basename(conn, name)?
                .into_iter()
                .map(PathBuf::from)
                .filter(|cp| cp.exists() && cp.parent() != Some(dead_parent.as_path()))
                .collect();

            // From Places
            for place in &places {
                for entry in walkdir::WalkDir::new(place)
                    .max_depth(5)
                    .into_iter()
                    .filter_map(|e| e.ok())
                {
                    if entry.file_name().to_string_lossy() == *name {
                        let cp = entry.path().to_path_buf();
                        if cp.parent() != Some(dead_parent.as_path()) {
                            cands.push(cp);
                        }
                    }
                }
            }

            cands.sort_by_key(|c| std::cmp::Reverse(mtime_secs(c)));
            match cands.first() {
                Some(src) => {
                    let dst = dead_parent.join(name);
                    let ok = std::fs::create_dir_all(&dead_parent).is_ok();
                    if ok && std::os::unix::fs::symlink(src, &dst).is_ok() {
                        linked += 1;
                        log(format!("relinked {name} -> {}", src.display()));
                    } else {
                        unresolved += 1;
                        log(format!("relink failed for {name}"));
                    }
                }
                None => {
                    unresolved += 1;
                    log(format!("relink failed for {name}: no candidates found"));
                }
            }
        }
        }
        Ok((linked, unresolved))
        }
/// Re-evaluate fidelity stamps on all worker-generated previews with the
/// CURRENT inventory (old stamps may come from buggier matching logic).
pub fn restamp_worker_previews(
    conn: &Connection,
    installed: &InstalledPlugins,
    log: &mut dyn FnMut(String),
) -> Result<usize> {
    let mut n = 0usize;
    for set_id in indexer::worker_preview_set_ids(conn)? {
        match renderability(conn, set_id, installed) {
            Ok(r) => {
                let json = if r.missing_plugins.is_empty()
                    && r.samples_missing == 0
                    && r.samples_evicted == 0
                {
                    None
                } else {
                    Some(serde_json::to_string(&r)?)
                };
                indexer::update_worker_preview_fidelity(conn, set_id, json.as_deref())?;
                n += 1;
            }
            Err(e) => log(format!("restamp failed for set {set_id}: {e}")),
        }
    }
    Ok(n)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn inv(names: &[&str]) -> InstalledPlugins {
        InstalledPlugins {
            names: names.iter().map(|s| s.to_string()).collect(),
        }
    }

    #[test]
    fn squashed_bundle_names_match() {
        // Real case (2026-06-12): Soundtoys bundles are concatenated,
        // .als names are spelled out. Padded so the inventory clears
        // MIN_PLAUSIBLE_INVENTORY and checks don't abstain.
        let i = inv(&[
            "littleplate", "sieq", "echoboy", "decapitator", "crystallizer",
            "tremolator", "phasemistress", "primaltap", "microshift",
            "littlealterboy", "panman", "filterfreak 1",
        ]);
        assert!(plugin_installed(&i, "Little Plate", Some("Soundtoys")));
        assert!(plugin_installed(&i, "Sie-Q", Some("Soundtoys")));
        assert!(plugin_installed(&i, "EchoBoy", Some("Soundtoys")));
        assert!(!plugin_installed(&i, "Serum", Some("Xfer Records")));
    }

    #[test]
    fn apple_stock_aus_always_present() {
        let i = inv(&[
            "a", "b", "c", "d", "e", "f", "g", "h", "i", "j", "k", "l",
        ]);
        assert!(plugin_installed(&i, "AUDelay", Some("Apple")));
        assert!(plugin_installed(&i, "AUGraphicEQ", None));
    }
}

/// Pre-flight: ask iCloud to materialize evicted samples of a set, then wait
/// (bounded) for them to land. Returns (downloaded, still_evicted).
pub fn materialize_icloud_samples(
    sample_paths: &[String],
    timeout: std::time::Duration,
) -> (usize, Vec<String>) {
    let evicted: Vec<&String> = sample_paths
        .iter()
        .filter(|p| sample_state(p) == SampleState::Evicted)
        .collect();
    if evicted.is_empty() {
        return (0, Vec::new());
    }
    for p in &evicted {
        // brctl asks fileproviderd to download; ignore per-file failures here,
        // the wait loop below is the ground truth.
        let _ = Command::new("brctl").arg("download").arg(p.as_str()).output();
    }
    let requested = evicted.len();
    let deadline = std::time::Instant::now() + timeout;
    let mut pending: Vec<&String> = evicted;
    while !pending.is_empty() && std::time::Instant::now() < deadline {
        std::thread::sleep(std::time::Duration::from_millis(500));
        pending.retain(|p| !Path::new(p.as_str()).exists());
    }
    let still: Vec<String> = pending.iter().map(|p| (*p).clone()).collect();
    (requested - still.len(), still)
}
