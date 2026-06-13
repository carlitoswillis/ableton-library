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

use std::path::{Path, PathBuf};

use anyhow::{bail, Context, Result};
use clap::{Parser, Subcommand};

use als_core::{discover, parse_set, ProjectSnapshot, SetSnapshot};

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
        /// Tag every project under ROOT with this artist, overriding the
        /// path-based guess. Use when scanning an artist's folder directly.
        #[arg(long)]
        artist: Option<String>,
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
        /// Substring match on the project's (path-derived) artist.
        #[arg(long)]
        artist: Option<String>,
        #[arg(long)]
        db: Option<PathBuf>,
    },
    /// List every derived artist and how many projects each has.
    Artists {
        #[arg(long)]
        db: Option<PathBuf>,
    },
    /// Backfill artists for already-indexed projects from their stored paths
    /// (no scanning, no re-parsing). Use after upgrading or the path fix.
    ReindexArtists {
        #[arg(long)]
        db: Option<PathBuf>,
    },
    /// Manually set a set's artist (override). Pass an empty string to clear.
    /// With --project, sets the whole project's artist instead of one set.
    SetArtist {
        /// Set id, exact path, or path fragment.
        set: String,
        /// Artist name ("" clears).
        artist: String,
        /// Apply to the whole project (all its sets) instead of just this set.
        #[arg(long)]
        project: bool,
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
    /// Renderability report for a set: missing plugins, missing/evicted
    /// samples, and the 0..1 score the export worker uses for easy-first
    /// ordering. (Builds the installed-plugin inventory; first run is slow.)
    Triage {
        /// Set id, exact path, or path fragment.
        set: String,
        /// Also print every known installed-plugin name (for debugging
        /// false "missing plugin" reports).
        #[arg(long)]
        show_inventory: bool,
        #[arg(long)]
        db: Option<PathBuf>,
    },
    /// Relink a set's missing samples: symlink dead paths to live copies
    /// found via the catalog (same filename referenced by any indexed set).
    Relink {
        /// Set id, exact path, or path fragment.
        set: String,
        #[arg(long)]
        db: Option<PathBuf>,
    },
    /// Clear and recompute triage scores for all pending export jobs
    /// (fresh plugin inventory). Use after installing plugins or upgrading.
    Rescore {
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
    /// Create a transformed copy of a set with missing samples relinked in
    /// the app cache dir. (M4b relinking redesign).
    Proxy {
        /// Set id, exact path, or path fragment.
        set: String,
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
        Cmd::Scan { root, db, force, no_previews, artist } => {
            cmd_scan(&root, db, force, no_previews, artist)
        }
        Cmd::Search { text, min_bpm, max_bpm, plugin, artist, db } => {
            cmd_search(text, min_bpm, max_bpm, plugin, artist, db)
        }
        Cmd::Artists { db } => cmd_artists(db),
        Cmd::ReindexArtists { db } => cmd_reindex_artists(db),
        Cmd::SetArtist { set, artist, project, db } => cmd_set_artist(&set, &artist, project, db),
        Cmd::Inspect { set, db } => cmd_inspect(&set, db),
        Cmd::Stats { db } => cmd_stats(db),
        Cmd::Previews { roots, threshold, verbose, db } => {
            cmd_previews(&roots, threshold, verbose, db)
        }
        Cmd::Triage { set, show_inventory, db } => cmd_triage(&set, show_inventory, db),
        Cmd::Rescore { db } => cmd_rescore(db),
        Cmd::Relink { set, db } => cmd_relink(&set, db),
        Cmd::Attach { set, audio, db } => cmd_attach(&set, &audio, db),
        Cmd::Proxy { set, db } => cmd_proxy(&set, db),
        Cmd::Reset { yes, db } => cmd_reset(yes, db),
    }
}

fn cmd_proxy(set: &str, db: Option<PathBuf>) -> Result<()> {
    let conn = indexer::open(&db_path(db)?)?;
    let set_id = indexer::resolve_set(&conn, set)?;
    let mut log = |line: String| eprintln!("  {line}");
    let path = ops::proxy::create_proxy_set(&conn, set_id, &mut log)?;
    eprintln!("proxy set created: {}", path.display());
    Ok(())
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


fn cmd_previews(
    roots: &[PathBuf],
    threshold: f64,
    verbose: bool,
    db: Option<PathBuf>,
) -> Result<()> {
    if roots.is_empty() {
        bail!("give at least one folder to hunt for renders, e.g. ~/Desktop ~/Downloads");
    }
    let conn = indexer::open(&db_path(db)?)?;
    let mut log = |line: String| eprintln!("  {line}");
    let s = ops::hunt_renders(&conn, roots, threshold, verbose, &mut log)?;
    eprintln!(
        "previews done: {} matched, {} unchanged, {} project-level (ambiguous), {} unmatched, {} known samples skipped, {} errors",
        s.matched, s.unchanged, s.ambiguous, s.unmatched, s.samples_skipped, s.errors
    );
    if s.unmatched > 0 && !verbose {
        eprintln!("(rerun with --verbose to list unmatched files; use `attach` for manual fixes)");
    }
    Ok(())
}

fn cmd_triage(set: &str, show_inventory: bool, db: Option<PathBuf>) -> Result<()> {
    let db = db_path(db)?;
    let conn = indexer::open(&db)?;
    let set_id = indexer::resolve_set(&conn, set)?;
    // Quick inventory: same source the app uses for instant scoring, so this
    // command reproduces exactly what the badges claim.
    let installed = ops::triage::installed_plugins_quick();
    eprintln!("{} installed plugin names known (quick inventory)", installed.names.len());
    if show_inventory {
        let mut names = installed.names.clone();
        names.sort();
        for n in &names {
            println!("{n}");
        }
        println!("---");
    }
    let r = ops::triage::renderability(&conn, set_id, &installed)?;
    println!("{}", serde_json::to_string_pretty(&r)?);
    Ok(())
}

fn cmd_rescore(db: Option<PathBuf>) -> Result<()> {
    let conn = indexer::open(&db_path(db)?)?;
    indexer::clear_pending_job_scores(&conn)?;
    indexer::clear_finished_job_fidelity(&conn)?;
    let installed = ops::triage::installed_plugins();
    eprintln!("{} installed plugin names (folder scan)", installed.names.len());
    let mut log = |line: String| eprintln!("  {line}");
    let n = ops::triage::score_pending_jobs(&conn, &installed, &mut log)?;
    let m = ops::triage::restamp_worker_previews(&conn, &installed, &mut log)?;
    eprintln!("re-scored {n} pending job(s); restamped {m} worker preview set(s)");
    Ok(())
}

fn cmd_relink(set: &str, db: Option<PathBuf>) -> Result<()> {
    let conn = indexer::open(&db_path(db)?)?;
    let set_id = indexer::resolve_set(&conn, set)?;
    let mut log = |line: String| eprintln!("  {line}");
    let (linked, unresolved) = ops::triage::relink_missing_samples(&conn, set_id, &mut log)?;
    eprintln!("relink done: {linked} linked, {unresolved} unresolved");
    Ok(())
}

fn cmd_attach(set: &str, audio: &Path, db: Option<PathBuf>) -> Result<()> {
    let conn = indexer::open(&db_path(db)?)?;
    let set_id = indexer::resolve_set(&conn, set)?;
    ops::attach(&conn, set_id, audio)?;
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

fn cmd_scan(
    root: &Path,
    db: Option<PathBuf>,
    force: bool,
    no_previews: bool,
    artist: Option<String>,
) -> Result<()> {
    let db = db_path(db)?;
    let conn = indexer::open(&db)?;
    let mut log = |line: String| eprintln!("  {line}");
    let s = ops::scan_library(&conn, root, force, !no_previews, artist.as_deref(), None, &mut log)?;
    let st = indexer::stats(&conn)?;
    eprintln!(
        "scan done: {} indexed, {} unchanged, {} errors, {} pruned, {} preview(s) harvested",
        s.indexed, s.unchanged, s.errors, s.pruned, s.harvested
    );
    eprintln!(
        "catalog: {} projects, {} sets, {} tracks, {} devices, {} samples, {} backups, {} previews ({})",
        st.projects, st.sets, st.tracks, st.devices, st.samples, st.backups, st.previews,
        db.display()
    );
    Ok(())
}

fn cmd_search(
    text: Option<String>,
    min_bpm: Option<f64>,
    max_bpm: Option<f64>,
    plugin: Option<String>,
    artist: Option<String>,
    db: Option<PathBuf>,
) -> Result<()> {
    let conn = indexer::open(&db_path(db)?)?;
    let hits = indexer::search(
        &conn,
        &indexer::SearchOpts {
            text,
            min_bpm,
            max_bpm,
            plugin,
            artist,
            list_id: None,
            sort_by: None,
            date_modified: None,
            date_scanned: None,
            has_preview: None,
        },
    )?;
    for h in &hits {
        let file = Path::new(&h.als_path)
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default();
        println!(
            "[{:>5}] {:18.18} {:28.28} {:28.28} {:>6} bpm  {:>4}  {}",
            h.set_id,
            h.artist.clone().unwrap_or_default(),
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

fn cmd_set_artist(set: &str, artist: &str, project: bool, db: Option<PathBuf>) -> Result<()> {
    let conn = indexer::open(&db_path(db)?)?;
    let set_id = indexer::resolve_set(&conn, set)?;
    let value = if artist.trim().is_empty() { None } else { Some(artist.trim()) };
    if project {
        let pid = indexer::set_project_id(&conn, set_id)?;
        indexer::set_project_artist_opt(&conn, pid, value)?;
        eprintln!("project artist {} for set {set_id}", value.map(|v| format!("set to '{v}'")).unwrap_or_else(|| "cleared".into()));
    } else {
        indexer::set_set_artist_override(&conn, set_id, value)?;
        eprintln!("set artist {} for set {set_id}", value.map(|v| format!("set to '{v}'")).unwrap_or_else(|| "cleared".into()));
    }
    Ok(())
}

fn cmd_reindex_artists(db: Option<PathBuf>) -> Result<()> {
    let conn = indexer::open(&db_path(db)?)?;
    let n = ops::reindex_artists(&conn)?;
    eprintln!("reindexed artists from stored paths: {n} project(s) tagged");
    Ok(())
}

fn cmd_artists(db: Option<PathBuf>) -> Result<()> {
    let conn = indexer::open(&db_path(db)?)?;
    let artists = indexer::list_artists(&conn)?;
    for (name, count) in &artists {
        println!(
            "{:>4}  {}",
            count,
            name.clone().unwrap_or_else(|| "(unknown)".into())
        );
    }
    eprintln!("{} artist(s)", artists.len());
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
