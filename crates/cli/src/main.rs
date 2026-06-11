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
    Scan {
        root: PathBuf,
        #[arg(long)]
        db: Option<PathBuf>,
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
        Cmd::Scan { root, db } => cmd_scan(&root, db),
        Cmd::Search { text, min_bpm, max_bpm, plugin, db } => {
            cmd_search(text, min_bpm, max_bpm, plugin, db)
        }
        Cmd::Inspect { set, db } => cmd_inspect(&set, db),
        Cmd::Stats { db } => cmd_stats(db),
    }
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

fn cmd_scan(root: &Path, db: Option<PathBuf>) -> Result<()> {
    let db = db_path(db)?;
    let conn = indexer::open(&db)?;
    let root_abs = std::path::absolute(root)?;
    let now = chrono::Utc::now().format("%Y-%m-%dT%H:%M:%S+00:00").to_string();

    let (mut parsed, mut fresh, mut errors) = (0usize, 0usize, 0usize);
    let mut seen: HashSet<String> = HashSet::new();

    conn.execute_batch("BEGIN")?;
    for proj in discover(&root_abs)? {
        let folder = std::path::absolute(&proj.dir)?.to_string_lossy().into_owned();
        let pid = indexer::upsert_project(&conn, &folder, &proj.name, &now)?;
        indexer::replace_backups(&conn, pid, &proj.backups)?;
        for als in &proj.als_files {
            let als_abs = std::path::absolute(als)?.to_string_lossy().into_owned();
            seen.insert(als_abs.clone());
            let size = std::fs::metadata(als)?.len();
            let mtime = iso_mtime(als)?;
            if indexer::set_is_fresh(&conn, &als_abs, size, &mtime)? {
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

    let st = indexer::stats(&conn)?;
    eprintln!(
        "scan done: {parsed} indexed, {fresh} unchanged, {errors} errors, {removed} pruned"
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
    // Resolve: numeric id, exact path, then path fragment.
    let set_id: i64 = if let Ok(id) = set.parse::<i64>() {
        id
    } else {
        conn.query_row(
            "SELECT id FROM sets WHERE als_path = ?1
             OR als_path LIKE '%' || ?1 || '%' LIMIT 1",
            rusqlite::params![set],
            |r| r.get(0),
        )
        .with_context(|| format!("no set matching '{set}'"))?
    };

    let mut out = serde_json::Map::new();
    conn.query_row(
        "SELECT s.als_path, s.live_version, s.tempo, s.time_signature, s.warnings, p.name
         FROM sets s JOIN projects p ON p.id = s.project_id WHERE s.id = ?1",
        rusqlite::params![set_id],
        |r| {
            out.insert("set_id".into(), set_id.into());
            out.insert("project".into(), r.get::<_, String>(5)?.into());
            out.insert("als_path".into(), r.get::<_, String>(0)?.into());
            out.insert("live_version".into(), r.get::<_, Option<String>>(1)?.into());
            out.insert("tempo".into(), r.get::<_, Option<f64>>(2)?.into());
            out.insert("time_signature".into(), r.get::<_, Option<String>>(3)?.into());
            let w: String = r.get(4)?;
            out.insert(
                "warnings".into(),
                serde_json::from_str(&w).unwrap_or(serde_json::Value::Null),
            );
            Ok(())
        },
    )
    .with_context(|| format!("no set with id {set_id}"))?;

    let list = |sql: &str, cols: &[&str]| -> Result<serde_json::Value> {
        let mut stmt = conn.prepare(sql)?;
        let mut rows = stmt.query(rusqlite::params![set_id])?;
        let mut arr = Vec::new();
        while let Some(row) = rows.next()? {
            let mut obj = serde_json::Map::new();
            for (i, c) in cols.iter().enumerate() {
                let v: rusqlite::types::Value = row.get(i)?;
                obj.insert((*c).into(), match v {
                    rusqlite::types::Value::Null => serde_json::Value::Null,
                    rusqlite::types::Value::Integer(n) => n.into(),
                    rusqlite::types::Value::Real(f) => f.into(),
                    rusqlite::types::Value::Text(s) => s.into(),
                    rusqlite::types::Value::Blob(_) => serde_json::Value::Null,
                });
            }
            arr.push(serde_json::Value::Object(obj));
        }
        Ok(serde_json::Value::Array(arr))
    };
    out.insert(
        "tracks".into(),
        list("SELECT idx, kind, name, color FROM tracks WHERE set_id = ?1 ORDER BY idx",
             &["idx", "kind", "name", "color"])?,
    );
    out.insert(
        "devices".into(),
        list("SELECT track_ref, kind, name, manufacturer FROM devices WHERE set_id = ?1",
             &["track", "kind", "name", "manufacturer"])?,
    );
    out.insert(
        "samples".into(),
        list("SELECT path, in_project, exists_on_disk FROM samples WHERE set_id = ?1",
             &["path", "in_project", "exists_on_disk"])?,
    );
    out.insert(
        "locators".into(),
        list("SELECT name, time FROM locators WHERE set_id = ?1", &["name", "time"])?,
    );
    println!("{}", serde_json::to_string_pretty(&serde_json::Value::Object(out))?);
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
