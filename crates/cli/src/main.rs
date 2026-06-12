//! ableton-scan: catalog Ableton Live projects from the filesystem.
//!
//! Subcommands:
//!   json <root>      one-shot JSON dump (oracle-compatible; diff vs tools/reference_extract.py)
//!   scan <root>      incremental index into the SQLite catalog
//!   search [TEXT]    query the catalog (FTS + tempo/plugin filters)
//!   inspect <SET>    full detail for one set (by id or path fragment)
//!   stats            catalog row counts
//!
//! Default db: <app data dir>/ableton-library/library.db (override with --db).

use std::collections::HashSet;
use std::path::{Path, PathBuf};

use anyhow::{bail, Context, Result};
use clap::{Parser, Subcommand};

use als_core::{discover, parse_set, scan::iso_mtime, ProjectSnapshot, SetSnapshot};

#[derive(Parser)]
#[command(name = "ableton-scan", about = "Index Ableton Live projects from the filesystem")]
struct Args {
    #[command(subcommand)]
    cmd: Cmd,
}

#[derive(Subcommand)]
enum Cmd {
    /// Parse everything and print JSON (no database). Matches the Python oracle.
    Json {
        root: PathBuf,
        #[arg(long)]
        pretty: bool,
    },
    /// Incrementally index a library into the SQLite catalog.
    /// Renders found inside project folders are harvested as previews
    /// automatically (disable with --no-previews).
    Scan {
        root: PathBuf,
        #[arg(long)]
        db: Option<PathBuf>,
        /// Re-parse and re-ingest every set, ignoring the freshness check.
        #[arg(long)]
        force: bool,
        /// Skip the in-folder render harvest (e.g. to avoid iCloud downloads).
        #[arg(long)]
        no_previews: bool,
    },
    /// Search the catalog. TEXT uses FTS5 over project/set/track/device/sample names.
    Search {
        text: Option<String>,
        #[arg(long)]
        min_bpm: Option<f64>,
        #[arg(long)]
        max_bpm: Option<f64>,
        /// Substring match on device/plugin name.
        #[arg(long)]
        plugin: Option<String>,
        #[arg(long)]
        db: Option<PathBuf>,
    },
    /// Show full stored detail for one set (id, exact path, or path fragment).
    Inspect {
        set: String,
        #[arg(long)]
        db: Option<PathBuf>,
    },
    /// Catalog row counts.
    Stats {
        #[arg(long)]
        db: Option<PathBuf>,
    },
    /// Hunt folders for exported renders and match them to indexed sets.
    /// Files are never moved — only referenced. Matched files get waveform
    /// peaks extracted (this reads the audio, so iCloud may download them).
    Previews {
        /// Folders where bounces accumulate (Desktop, Downloads, ...)
        roots: Vec<PathBuf>,
        /// Minimum match confidence (0..1).
        #[arg(long, default_value_t = 0.6)]
        threshold: f64,
        /// List unmatched files too.
        #[arg(long)]
        verbose: bool,
        #[arg(long)]
        db: Option<PathBuf>,
    },
    /// Delete the catalog database. Safe: it is fully rebuildable by rescanning.
    Reset {
        /// Actually delete (without this, just shows what would be removed).
        #[arg(long)]
        yes: bool,
        #[arg(long)]
        db: Option<PathBuf>,
    },
    /// Manually attach an audio file to a set (confidence 1.0).
    Attach {
        /// Set id, exact path, or path fragment.
        set: String,
        /// Audio file to attach.
        audio: PathBuf,
        #[arg(long)]
        db: Option<PathBuf>,
    },
}

fn db_path(opt: Option<PathBuf>) -> Result<PathBuf> {
    if let Some(p) = opt {
        return Ok(p);
    }
    let base = dirs::data_dir().context("no app data dir on this platform")?;
    Ok(base.join("ableton-library").join("library.db"))
}

fn main() -> Result<()> {
    match Args::parse().cmd {
        Cmd::Json { root, pretty } => cmd_json(&root, pretty),
        Cmd::Scan { root, db, force, no_previews } => cmd_scan(&root, db, force, no_previews),
        Cmd::Search { text, min_bpm, max_bpm, plugin, db } => {
            cmd_search(text, min_bpm, max_bpm, plugin, db)
        }
        Cmd::Inspect { set, db } => cmd_inspect(&set, db),
        Cmd::Stats { db } => cmd_stats(db),
        Cmd::Previews { roots, threshold, verbose, db } => {
            cmd_previews(&roots, threshold, verbose, db)
        }
        Cmd::Attach { set, audio, db } => cmd_attach(&set, &audio, db),
        Cmd::Reset { yes, db } => cmd_reset(yes, db),
    }
}

fn cmd_reset(yes: bool, db: Option<PathBuf>) -> Result<()> {
    let db = db_path(db)?;
    let targets: Vec<PathBuf> = ["", "-wal", "-shm"]
        .iter()
        .map(|sfx| PathBuf::from(format!("{}{}", db.display(), sfx)))
        .filter(|p| p.exists())
        .collect();
    if targets.is_empty() {
        eprintln!("nothing to delete — no catalog at {}", db.display());
        return Ok(());
    }
    if !yes {
        eprintln!("would delete:");
        for t in &targets {
            eprintln!("  {}", t.display());
        }
        eprintln!("rerun with --yes to confirm (the catalog is rebuildable by rescanning)");
        return Ok(());
    }
    for t in &targets {
        std::fs::remove_file(t)?;
        eprintln!("deleted {}", t.display());
    }
    Ok(())
}

/// Load matcher candidates from the catalog.
fn set_candidates(conn: &rusqlite::Connection) -> Result<Vec<previews::matching::SetCandidate>> {
    use previews::matching::normalize;
    let mut out = Vec::new();
    for (set_id, project_id, als_path, project_name, count) in
        indexer::set_match_candidates(conn)?
    {
        let stem = Path::new(&als_path)
            .file_stem()
            .map(|s| s.to_string_lossy().into_owned())
            .unwrap_or_default();
        let proj = project_name.trim_end_matches(" Project");
        out.push(previews::matching::SetCandidate {
            set_id,
            project_id,
            norm_stem: normalize(&stem),
            norm_project: normalize(proj),
            project_set_count: count as usize,
        });
    }
    Ok(out)
}

/// Extract peaks and build a PreviewRow (shared by previews + attach).
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
/// has exactly one set -> that set at 0.7; otherwise project-level at 0.5.
fn harvest_folder_renders(
    conn: &rusqlite::Connection,
    dir: &Path,
    project_name: &str,
    pid: i64,
    known_samples: &HashSet<String>,
) -> Result<usize> {
    use previews::matching::{best_match, normalize, MatchTarget, SetCandidate};
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
                    .map(|s| s.to_string_lossy().into_owned())
                    .unwrap_or_default(),
            ),
            norm_project: norm_project.clone(),
            project_set_count: sets.len(),
        })
        .collect();

    let mut count = 0usize;
    for r in previews::discover_renders(&[dir.to_path_buf()])? {
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
                // No name resemblance, but a render in a single-set project
                // folder can only belong to that set.
                None if sets.len() == 1 => (Some(sets[0].0), Some(pid), 0.7),
                None => continue,
            };
        let mtime = iso_mtime(&r.path)?;
        if indexer::preview_is_fresh(&conn, set_id, &abs, r.size, &mtime)? {
            continue;
        }
        match build_preview_row(&r.path, set_id, project_id, "discovered", confidence) {
            Ok(row) => {
                indexer::upsert_preview(conn, &row)?;
                count += 1;
                eprintln!("  preview ({confidence:.2}) {}", r.path.display());
            }
            Err(e) => eprintln!("  ERROR decoding {}: {e}", r.path.display()),
        }
    }
    Ok(count)
}

fn cmd_previews(
    roots: &[PathBuf],
    threshold: f64,
    verbose: bool,
    db: Option<PathBuf>,
) -> Result<()> {
    use previews::matching::{best_match, normalize, MatchTarget};
    if roots.is_empty() {
        bail!("give at least one folder to hunt for renders, e.g. ~/Desktop ~/Downloads");
    }
    let conn = indexer::open(&db_path(db)?)?;
    let cands = set_candidates(&conn)?;
    if cands.is_empty() {
        bail!("catalog is empty — run `ableton-scan scan <root>` first");
    }
    let known_samples = indexer::all_sample_paths(&conn)?;
    let renders = previews::discover_renders(roots)?;
    eprintln!("{} candidate audio file(s) found, matching against {} set(s)…",
        renders.len(), cands.len());

    let (mut matched, mut fresh, mut ambiguous, mut unmatched, mut errors, mut samples_skipped) =
        (0usize, 0usize, 0usize, 0usize, 0usize, 0usize);
    for r in &renders {
        // Never attach a file the catalog knows as a SAMPLE of some set.
        let abs_check = std::path::absolute(&r.path)?.to_string_lossy().into_owned();
        if known_samples.contains(&abs_check) {
            samples_skipped += 1;
            if verbose {
                eprintln!("  skipped (known sample): {}", r.path.display());
            }
            continue;
        }
        let norm = normalize(&r.stem);
        match best_match(&norm, &cands, threshold) {
            Some(m) => {
                let (set_id, project_id) = match m.target {
                    MatchTarget::Set { set_id, project_id } => (Some(set_id), Some(project_id)),
                    MatchTarget::Project { project_id } => {
                        ambiguous += 1;
                        (None, Some(project_id))
                    }
                };
                let abs = std::path::absolute(&r.path)?.to_string_lossy().into_owned();
                let mtime = iso_mtime(&r.path)?;
                if indexer::preview_is_fresh(&conn, set_id, &abs, r.size, &mtime)? {
                    fresh += 1;
                    continue;
                }
                match build_preview_row(&r.path, set_id, project_id, "discovered", m.confidence) {
                    Ok(row) => {
                        indexer::upsert_preview(&conn, &row)?;
                        matched += 1;
                        eprintln!(
                            "  matched ({:.2}) {} -> set {:?}",
                            m.confidence,
                            r.path.display(),
                            set_id
                        );
                    }
                    Err(e) => {
                        errors += 1;
                        eprintln!("  ERROR decoding {}: {e}", r.path.display());
                    }
                }
            }
            None => {
                unmatched += 1;
                if verbose {
                    eprintln!("  unmatched: {}", r.path.display());
                }
            }
        }
    }
    eprintln!(
        "previews done: {matched} matched, {fresh} unchanged, {ambiguous} project-level (ambiguous), {unmatched} unmatched, {samples_skipped} known samples skipped, {errors} errors"
    );
    if unmatched > 0 && !verbose {
        eprintln!("(rerun with --verbose to list unmatched files; use `attach` for manual fixes)");
    }
    Ok(())
}

fn cmd_attach(set: &str, audio: &Path, db: Option<PathBuf>) -> Result<()> {
    let conn = indexer::open(&db_path(db)?)?;
    let set_id = indexer::resolve_set(&conn, set)?;
    let project_id = indexer::set_project_id(&conn, set_id)?;
    let row = build_preview_row(audio, Some(set_id), Some(project_id), "manual", 1.0)?;
    indexer::upsert_preview(&conn, &row)?;
    eprintln!("attached {} to set {set_id} (primary recomputed)", audio.display());
    Ok(())
}

fn cmd_json(root: &Path, pretty: bool) -> Result<()> {
    let mut library = Vec::new();
    for proj in discover(root)? {
        let mut sets = Vec::new();
        for als in &proj.als_files {
            match parse_set(als, &proj.dir) {
                Ok(snap) => {
                    summarize(&snap);
                    sets.push(snap);
                }
                Err(e) => eprintln!("  ERROR {}: {e}", als.display()),
            }
        }
        eprintln!("{}: {} set(s), {} backup(s)", proj.name, sets.len(), proj.backups.len());
        library.push(ProjectSnapshot {
            folder_path: std::path::absolute(&proj.dir)?.to_string_lossy().into_owned(),
            name: proj.name,
            sets,
            backups: proj.backups,
        });
    }
    let json = if pretty {
        serde_json::to_string_pretty(&library)?
    } else {
        serde_json::to_string(&library)?
    };
    println!("{json}");
    Ok(())
}

fn cmd_scan(root: &Path, db: Option<PathBuf>, force: bool, no_previews: bool) -> Result<()> {
    let db = db_path(db)?;
    let conn = indexer::open(&db)?;
    let root_abs = std::path::absolute(root)?;
    let now = chrono::Utc::now().format("%Y-%m-%dT%H:%M:%S+00:00").to_string();

    let (mut parsed, mut fresh, mut errors) = (0usize, 0usize, 0usize);
    let mut seen: HashSet<String> = HashSet::new();
    let mut harvest_targets: Vec<(PathBuf, String, i64)> = Vec::new();

    conn.execute_batch("BEGIN")?;
    for proj in discover(&root_abs)? {
        let folder = std::path::absolute(&proj.dir)?.to_string_lossy().into_owned();
        let pid = indexer::upsert_project(&conn, &folder, &proj.name, &now)?;
        harvest_targets.push((proj.dir.clone(), proj.name.clone(), pid));
        indexer::replace_backups(&conn, pid, &proj.backups)?;
        for als in &proj.als_files {
            let als_abs = std::path::absolute(als)?.to_string_lossy().into_owned();
            seen.insert(als_abs.clone());
            let size = std::fs::metadata(als)?.len();
            let mtime = iso_mtime(als)?;
            if !force && indexer::set_is_fresh(&conn, &als_abs, size, &mtime)? {
                fresh += 1;
                continue;
            }
            match parse_set(als, &proj.dir) {
                Ok(snap) => {
                    indexer::ingest_set(&conn, pid, &snap)?;
                    parsed += 1;
                    eprintln!("  indexed {}", als.display());
                }
                Err(e) => {
                    errors += 1;
                    eprintln!("  ERROR {}: {e}", als.display());
                }
            }
        }
    }
    let removed = indexer::prune_missing(&conn, &root_abs.to_string_lossy(), &seen)?;
    conn.execute_batch("COMMIT")?;

    // Harvest pass: renders sitting inside project folders are near-certain
    // matches (the folder placement is the signal). Runs after commit so the
    // samples cross-check sees everything just indexed.
    let mut harvested = 0usize;
    if !no_previews {
        let known_samples = indexer::all_sample_paths(&conn)?;
        for (dir, name, pid) in &harvest_targets {
            harvested += harvest_folder_renders(&conn, dir, name, *pid, &known_samples)
                .unwrap_or_else(|e| {
                    eprintln!("  preview harvest failed for {}: {e}", dir.display());
                    0
                });
        }
    }

    let st = indexer::stats(&conn)?;
    eprintln!(
        "scan done: {parsed} indexed, {fresh} unchanged, {errors} errors, {removed} pruned, {harvested} preview(s) harvested"
    );
    eprintln!(
        "catalog: {} projects, {} sets, {} tracks, {} devices, {} samples, {} backups ({})",
        st.projects, st.sets, st.tracks, st.devices, st.samples, st.backups, db.display()
    );
    Ok(())
}

fn cmd_search(
    text: Option<String>,
    min_bpm: Option<f64>,
    max_bpm: Option<f64>,
    plugin: Option<String>,
    db: Option<PathBuf>,
) -> Result<()> {
    let conn = indexer::open(&db_path(db)?)?;
    let hits = indexer::search(
        &conn,
        &indexer::SearchOpts { text, min_bpm, max_bpm, plugin },
    )?;
    for h in &hits {
        let file = Path::new(&h.als_path)
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default();
        println!(
            "[{:>5}] {:35.35} {:35.35} {:>6} bpm  {:>4}  {}",
            h.set_id,
            h.project,
            file,
            h.tempo.map(|t| format!("{t}")).unwrap_or_else(|| "?".into()),
            h.time_signature.clone().unwrap_or_else(|| "?".into()),
            h.live_version.clone().unwrap_or_default(),
        );
    }
    eprintln!("{} result(s)", hits.len());
    Ok(())
}

fn cmd_inspect(set: &str, db: Option<PathBuf>) -> Result<()> {
    let conn = indexer::open(&db_path(db)?)?;
    let set_id = indexer::resolve_set(&conn, set)?;
    let detail = indexer::set_detail(&conn, set_id)?;
    println!("{}", serde_json::to_string_pretty(&detail)?);
    Ok(())
}

fn cmd_stats(db: Option<PathBuf>) -> Result<()> {
    let db = db_path(db)?;
    if !db.exists() {
        bail!("no catalog at {} — run `ableton-scan scan <root>` first", db.display());
    }
    let conn = indexer::open(&db)?;
    let st = indexer::stats(&conn)?;
    println!("db:       {}", db.display());
    println!("projects: {}", st.projects);
    println!("sets:     {}", st.sets);
    println!("tracks:   {}", st.tracks);
    println!("devices:  {}", st.devices);
    println!("samples:  {}", st.samples);
    println!("backups:  {}", st.backups);
    println!("previews: {}", st.previews);
    Ok(())
}

fn summarize(s: &SetSnapshot) {
    use std::collections::BTreeMap;
    let mut kinds: BTreeMap<&str, usize> = BTreeMap::new();
    for t in &s.tracks {
        let k = match t.kind {
            als_core::TrackKind::Midi => "midi",
            als_core::TrackKind::Audio => "audio",
            als_core::TrackKind::Return => "return",
            als_core::TrackKind::Group => "group",
        };
        *kinds.entry(k).or_default() += 1;
    }
    let plugins = s
        .devices
        .iter()
        .filter(|d| d.kind != als_core::DeviceKind::Native)
        .count();
    let file = Path::new(&s.als_path)
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    eprintln!(
        "  {file:40} {:>6} bpm  {:>5}  tracks={kinds:?}  devices={} ({plugins} plugin)  samples={}  warnings={}",
        s.tempo.map(|t| t.to_string()).unwrap_or_else(|| "?".into()),
        s.time_signature.clone().unwrap_or_else(|| "?".into()),
        s.devices.len(),
        s.samples.len(),
        s.warnings.len(),
    );
}
