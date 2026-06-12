//! ops: the operations layer shared by the CLI and the desktop app.
//!
//! Everything here used to live in the CLI binary; it moved so the app can
//! run the same scans. Layering: als-core (parse) + previews (renders/peaks)
//! -> indexer (storage) -> ops (workflows) -> cli / app (frontends).

use std::collections::HashSet;
use std::path::{Path, PathBuf};

use anyhow::Result;
use rusqlite::Connection;

use als_core::scan::iso_mtime;
use als_core::{discover, parse_set};
use previews::matching::{best_match, normalize, MatchTarget, SetCandidate};

/// Progress sink: cli prints to stderr, the app may ignore or forward.
pub type Log<'a> = &'a mut dyn FnMut(String);

#[derive(Debug, Default, serde::Serialize)]
pub struct ScanSummary {
    pub indexed: usize,
    pub unchanged: usize,
    pub errors: usize,
    pub pruned: usize,
    pub harvested: usize,
}

/// Index a library root (incremental), then harvest in-folder renders.
pub fn scan_library(
    conn: &Connection,
    root: &Path,
    force: bool,
    harvest: bool,
    cancel: Option<&std::sync::atomic::AtomicBool>,
    log: Log,
) -> Result<ScanSummary> {
    let root_abs = std::path::absolute(root)?;
    let now = chrono::Utc::now().format("%Y-%m-%dT%H:%M:%S+00:00").to_string();

    let mut s = ScanSummary::default();
    let mut seen: HashSet<String> = HashSet::new();
    let mut harvest_targets: Vec<(PathBuf, String, i64)> = Vec::new();

    conn.execute_batch("BEGIN")?;
    for proj in discover(&root_abs)? {
        if let Some(c) = cancel {
            if c.load(std::sync::atomic::Ordering::Relaxed) {
                anyhow::bail!("scan cancelled by user");
            }
        }
        let folder = std::path::absolute(&proj.dir)?.to_string_lossy().into_owned();
        let pid = indexer::upsert_project(conn, &folder, &proj.name, &now)?;
        harvest_targets.push((proj.dir.clone(), proj.name.clone(), pid));
        indexer::replace_backups(conn, pid, &proj.backups)?;
        for als in &proj.als_files {
            if let Some(c) = cancel {
                if c.load(std::sync::atomic::Ordering::Relaxed) {
                    anyhow::bail!("scan cancelled by user");
                }
            }
            let als_abs = std::path::absolute(als)?.to_string_lossy().into_owned();
            seen.insert(als_abs.clone());
            let size = std::fs::metadata(als)?.len();
            let mtime = iso_mtime(als)?;
            if !force && indexer::set_is_fresh(conn, &als_abs, size, &mtime)? {
                s.unchanged += 1;
                continue;
            }
            match parse_set(als, &proj.dir) {
                Ok(snap) => {
                    indexer::ingest_set(conn, pid, &snap)?;
                    s.indexed += 1;
                    log(format!("indexed {}", als.display()));
                }
                Err(e) => {
                    s.errors += 1;
                    log(format!("ERROR {}: {e}", als.display()));
                }
            }
        }
    }
    let stale_previews = indexer::prune_stale_previews(conn)?;
    for (_, path) in stale_previews {
        log(format!("preview removed (missing from disk): {}", path));
    }
    s.pruned = indexer::prune_missing(conn, &root_abs.to_string_lossy(), &seen)?;
    conn.execute_batch("COMMIT")?;


    // Harvest pass: renders sitting inside project folders are near-certain
    // matches (folder placement is the signal). Runs after commit so the
    // samples cross-check sees everything just indexed.
    if harvest {
        let known_samples = indexer::all_sample_paths(conn)?;
        for (dir, name, pid) in &harvest_targets {
            if let Some(c) = cancel {
                if c.load(std::sync::atomic::Ordering::Relaxed) {
                    anyhow::bail!("scan cancelled by user");
                }
            }
            match harvest_folder_renders(conn, dir, name, *pid, &known_samples, log) {
                Ok(n) => s.harvested += n,
                Err(e) => log(format!("preview harvest failed for {}: {e}", dir.display())),
            }
        }
    }
    Ok(s)
}

/// Extract peaks and build a PreviewRow (shared by harvest / hunt / attach).
fn build_preview_row(
    audio: &Path,
    set_id: Option<i64>,
    project_id: Option<i64>,
    source: &str,
    confidence: f64,
) -> Result<indexer::PreviewRow> {
    let meta = std::fs::metadata(audio)?;
    let pk = previews::peaks::extract(audio)?;
    Ok(indexer::PreviewRow {
        set_id,
        project_id,
        audio_path: std::path::absolute(audio)?.to_string_lossy().into_owned(),
        source: source.into(),
        confidence,
        mtime: iso_mtime(audio)?,
        size: meta.len(),
        duration: Some(pk.duration_secs),
        peaks_json: Some(previews::peaks::to_json(&pk.peaks)),
    })
}

/// Attach renders found INSIDE one project folder to that project's sets.
/// Folder placement is strong evidence, so matching is local and generous:
/// name match -> that set (+0.05 folder bonus); no name match but the project
/// has exactly one set -> that set at 0.7; otherwise skipped.
fn harvest_folder_renders(
    conn: &Connection,
    dir: &Path,
    project_name: &str,
    pid: i64,
    known_samples: &HashSet<String>,
    log: Log,
) -> Result<usize> {
    let sets = indexer::project_sets(conn, pid)?;
    if sets.is_empty() {
        return Ok(0);
    }
    let norm_project = normalize(project_name.trim_end_matches(" Project"));
    let cands: Vec<SetCandidate> = sets
        .iter()
        .map(|(set_id, als_path)| SetCandidate {
            set_id: *set_id,
            project_id: pid,
            norm_stem: normalize(
                &Path::new(als_path)
                    .file_stem()
                    .map(|x| x.to_string_lossy().into_owned())
                    .unwrap_or_default(),
            ),
            norm_project: norm_project.clone(),
            project_set_count: sets.len(),
        })
        .collect();

    let mut count = 0usize;
    for r in previews::discover_renders(&[dir.to_path_buf()], Some(2))? {
        let abs = std::path::absolute(&r.path)?.to_string_lossy().into_owned();
        if known_samples.contains(&abs) {
            continue;
        }
        let (set_id, project_id, confidence) =
            match best_match(&normalize(&r.stem), &cands, 0.6) {
                Some(m) => match m.target {
                    MatchTarget::Set { set_id, project_id } => {
                        (Some(set_id), Some(project_id), (m.confidence + 0.05).min(1.0))
                    }
                    MatchTarget::Project { project_id } => (None, Some(project_id), m.confidence),
                },
                None if sets.len() == 1 => (Some(sets[0].0), Some(pid), 0.7),
                None => continue,
            };
        let mtime = iso_mtime(&r.path)?;
        if indexer::preview_is_fresh(conn, set_id, &abs, r.size, &mtime)? {
            continue;
        }
        match build_preview_row(&r.path, set_id, project_id, "discovered", confidence) {
            Ok(row) => {
                indexer::upsert_preview(conn, &row)?;
                count += 1;
                log(format!("preview ({confidence:.2}) {}", r.path.display()));
            }
            Err(e) => log(format!("ERROR decoding {}: {e}", r.path.display())),
        }
    }
    Ok(count)
}

#[derive(Debug, Default, serde::Serialize)]
pub struct HuntSummary {
    pub matched: usize,
    pub unchanged: usize,
    pub ambiguous: usize,
    pub unmatched: usize,
    pub samples_skipped: usize,
    pub errors: usize,
}

/// Hunt arbitrary folders for renders and match them against the whole
/// catalog by name (renders are scattered — user decision 2026-06-11).
pub fn hunt_renders(
    conn: &Connection,
    roots: &[PathBuf],
    threshold: f64,
    verbose: bool,
    log: Log,
) -> Result<HuntSummary> {
    let cands = catalog_candidates(conn)?;
    if cands.is_empty() {
        anyhow::bail!("catalog is empty — scan a projects folder first");
    }
    let known_samples = indexer::all_sample_paths(conn)?;
    let renders = previews::discover_renders(roots, None)?;
    log(format!(
        "{} candidate audio file(s), matching against {} set(s)",
        renders.len(),
        cands.len()
    ));

    let mut s = HuntSummary::default();
    for r in &renders {
        let abs = std::path::absolute(&r.path)?.to_string_lossy().into_owned();
        if known_samples.contains(&abs) {
            s.samples_skipped += 1;
            if verbose {
                log(format!("skipped (known sample): {}", r.path.display()));
            }
            continue;
        }
        match best_match(&normalize(&r.stem), &cands, threshold) {
            Some(m) => {
                let (set_id, project_id) = match m.target {
                    MatchTarget::Set { set_id, project_id } => (Some(set_id), Some(project_id)),
                    MatchTarget::Project { project_id } => {
                        s.ambiguous += 1;
                        (None, Some(project_id))
                    }
                };
                let mtime = iso_mtime(&r.path)?;
                if indexer::preview_is_fresh(conn, set_id, &abs, r.size, &mtime)? {
                    s.unchanged += 1;
                    continue;
                }
                match build_preview_row(&r.path, set_id, project_id, "discovered", m.confidence) {
                    Ok(row) => {
                        indexer::upsert_preview(conn, &row)?;
                        s.matched += 1;
                        log(format!(
                            "matched ({:.2}) {} -> set {:?}",
                            m.confidence,
                            r.path.display(),
                            set_id
                        ));
                    }
                    Err(e) => {
                        s.errors += 1;
                        log(format!("ERROR decoding {}: {e}", r.path.display()));
                    }
                }
            }
            None => {
                s.unmatched += 1;
                if verbose {
                    log(format!("unmatched: {}", r.path.display()));
                }
            }
        }
    }
    Ok(s)
}

/// Manually attach an audio file to a set (confidence 1.0).
pub fn attach(conn: &Connection, set_id: i64, audio: &Path) -> Result<()> {
    let project_id = indexer::set_project_id(conn, set_id)?;
    let row = build_preview_row(audio, Some(set_id), Some(project_id), "manual", 1.0)?;
    indexer::upsert_preview(conn, &row)?;
    Ok(())
}

/// Matcher candidates for the whole catalog.
fn catalog_candidates(conn: &Connection) -> Result<Vec<SetCandidate>> {
    let mut out = Vec::new();
    for (set_id, project_id, als_path, project_name, count) in
        indexer::set_match_candidates(conn)?
    {
        let stem = Path::new(&als_path)
            .file_stem()
            .map(|x| x.to_string_lossy().into_owned())
            .unwrap_or_default();
        out.push(SetCandidate {
            set_id,
            project_id,
            norm_stem: normalize(&stem),
            norm_project: normalize(project_name.trim_end_matches(" Project")),
            project_set_count: count as usize,
        });
    }
    Ok(out)
}
