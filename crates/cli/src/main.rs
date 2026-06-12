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

fn cmd_scan(root: &Path, db: Option<PathBuf>, force: bool, no_previews: bool) -> Result<()> {
    let db = db_path(db)?;
    let conn = indexer::open(&db)?;
    let mut log = |line: String| eprintln!("  {line}");
    let s = ops::scan_library(&conn, root, force, !no_previews, None, &mut log)?;
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
