//! indexer: SQLite catalog over als-core snapshots.
//!
//! Design (see ai/ARCHITECTURE.md):
//! - SQLite (bundled — no system dependency) with FTS5 for name search.
//! - Incremental: a set whose (file_size, mtime) is unchanged is never
//!   re-parsed. Re-ingest replaces all child rows for that set.
//! - The index lives OUTSIDE user project folders (app data dir by default).

use std::collections::HashSet;
use std::path::Path;

use anyhow::{Context, Result};
use rusqlite::{params, Connection};

use als_core::{BackupEntry, SetSnapshot, TrackKind, TrackRef};

const SCHEMA: &str = r#"
CREATE TABLE IF NOT EXISTS projects (
    id           INTEGER PRIMARY KEY,
    folder_path  TEXT UNIQUE NOT NULL,
    name         TEXT NOT NULL,
    last_scanned TEXT NOT NULL DEFAULT ''
);
CREATE TABLE IF NOT EXISTS sets (
    id             INTEGER PRIMARY KEY,
    project_id     INTEGER NOT NULL REFERENCES projects(id) ON DELETE CASCADE,
    als_path       TEXT UNIQUE NOT NULL,
    file_size      INTEGER NOT NULL,
    mtime          TEXT NOT NULL,
    content_hash   TEXT NOT NULL,
    live_version   TEXT,
    schema_version TEXT,
    tempo          REAL,
    time_signature TEXT,
    warnings       TEXT NOT NULL DEFAULT '[]'   -- JSON array
);
CREATE TABLE IF NOT EXISTS tracks (
    id      INTEGER PRIMARY KEY,
    set_id  INTEGER NOT NULL REFERENCES sets(id) ON DELETE CASCADE,
    idx     INTEGER NOT NULL,
    kind    TEXT NOT NULL,
    name    TEXT,
    color   INTEGER
);
CREATE TABLE IF NOT EXISTS devices (
    id           INTEGER PRIMARY KEY,
    set_id       INTEGER NOT NULL REFERENCES sets(id) ON DELETE CASCADE,
    track_ref    TEXT,            -- track index as text, 'master', or NULL
    kind         TEXT NOT NULL,   -- native | au | vst | vst3
    name         TEXT,
    manufacturer TEXT
);
CREATE TABLE IF NOT EXISTS samples (
    id             INTEGER PRIMARY KEY,
    set_id         INTEGER NOT NULL REFERENCES sets(id) ON DELETE CASCADE,
    path           TEXT NOT NULL,
    in_project     INTEGER NOT NULL,
    exists_on_disk INTEGER NOT NULL
);
CREATE TABLE IF NOT EXISTS locators (
    id     INTEGER PRIMARY KEY,
    set_id INTEGER NOT NULL REFERENCES sets(id) ON DELETE CASCADE,
    name   TEXT,
    time   REAL
);
CREATE TABLE IF NOT EXISTS backups (
    id         INTEGER PRIMARY KEY,
    project_id INTEGER NOT NULL REFERENCES projects(id) ON DELETE CASCADE,
    file       TEXT NOT NULL,
    size       INTEGER NOT NULL,
    mtime      TEXT NOT NULL
);
CREATE VIRTUAL TABLE IF NOT EXISTS search USING fts5(
    set_id UNINDEXED, project_name, set_name, track_names, device_names, sample_names
);
CREATE INDEX IF NOT EXISTS idx_sets_project   ON sets(project_id);
CREATE INDEX IF NOT EXISTS idx_tracks_set     ON tracks(set_id);
CREATE INDEX IF NOT EXISTS idx_devices_set    ON devices(set_id);
CREATE INDEX IF NOT EXISTS idx_devices_name   ON devices(name);
CREATE INDEX IF NOT EXISTS idx_samples_set    ON samples(set_id);
CREATE INDEX IF NOT EXISTS idx_samples_path   ON samples(path);
CREATE INDEX IF NOT EXISTS idx_backups_proj   ON backups(project_id);
"#;

/// Open (creating if needed) the index database.
pub fn open(db_path: &Path) -> Result<Connection> {
    if let Some(dir) = db_path.parent() {
        std::fs::create_dir_all(dir)
            .with_context(|| format!("creating {}", dir.display()))?;
    }
    let conn = Connection::open(db_path)
        .with_context(|| format!("opening {}", db_path.display()))?;
    let _mode: String = conn.query_row("PRAGMA journal_mode=WAL", [], |r| r.get(0))?;
    conn.execute_batch("PRAGMA foreign_keys=ON;")?;
    conn.execute_batch(SCHEMA)?;
    Ok(conn)
}

pub fn upsert_project(conn: &Connection, folder_path: &str, name: &str, now: &str) -> Result<i64> {
    conn.execute(
        "INSERT INTO projects (folder_path, name, last_scanned) VALUES (?1, ?2, ?3)
         ON CONFLICT(folder_path) DO UPDATE SET name = ?2, last_scanned = ?3",
        params![folder_path, name, now],
    )?;
    Ok(conn.query_row(
        "SELECT id FROM projects WHERE folder_path = ?1",
        params![folder_path],
        |r| r.get(0),
    )?)
}

pub fn replace_backups(conn: &Connection, project_id: i64, backups: &[BackupEntry]) -> Result<()> {
    conn.execute("DELETE FROM backups WHERE project_id = ?1", params![project_id])?;
    let mut stmt = conn.prepare(
        "INSERT INTO backups (project_id, file, size, mtime) VALUES (?1, ?2, ?3, ?4)",
    )?;
    for b in backups {
        stmt.execute(params![project_id, b.file, b.size, b.mtime])?;
    }
    Ok(())
}

/// True if the stored row for this path matches size+mtime (no re-parse needed).
pub fn set_is_fresh(conn: &Connection, als_path: &str, size: u64, mtime: &str) -> Result<bool> {
    let row: Option<(u64, String)> = conn
        .query_row(
            "SELECT file_size, mtime FROM sets WHERE als_path = ?1",
            params![als_path],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .map(Some)
        .or_else(|e| match e {
            rusqlite::Error::QueryReturnedNoRows => Ok(None),
            e => Err(e),
        })?;
    Ok(row.map_or(false, |(s, m)| s == size && m == mtime))
}

fn kind_str(k: TrackKind) -> &'static str {
    match k {
        TrackKind::Midi => "midi",
        TrackKind::Audio => "audio",
        TrackKind::Return => "return",
        TrackKind::Group => "group",
    }
}

fn track_ref_str(t: &Option<TrackRef>) -> Option<String> {
    match t {
        None => None,
        Some(TrackRef::Index(i)) => Some(i.to_string()),
        Some(TrackRef::Master(s)) => Some(s.clone()),
    }
}

/// Replace (or insert) one set and all its child rows + FTS entry.
pub fn ingest_set(conn: &Connection, project_id: i64, s: &SetSnapshot) -> Result<i64> {
    // Remove the previous version of this set (children cascade; FTS doesn't).
    if let Ok(old_id) = conn.query_row(
        "SELECT id FROM sets WHERE als_path = ?1",
        params![s.als_path],
        |r| r.get::<_, i64>(0),
    ) {
        conn.execute("DELETE FROM search WHERE set_id = ?1", params![old_id])?;
        conn.execute("DELETE FROM sets WHERE id = ?1", params![old_id])?;
    }

    conn.execute(
        "INSERT INTO sets (project_id, als_path, file_size, mtime, content_hash,
                           live_version, schema_version, tempo, time_signature, warnings)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)",
        params![
            project_id,
            s.als_path,
            s.file_size,
            s.mtime,
            s.content_hash,
            s.live_version,
            s.schema_version,
            s.tempo,
            s.time_signature,
            serde_json::to_string(&s.warnings)?,
        ],
    )?;
    let set_id = conn.last_insert_rowid();

    let mut t_stmt = conn.prepare(
        "INSERT INTO tracks (set_id, idx, kind, name, color) VALUES (?1, ?2, ?3, ?4, ?5)",
    )?;
    for (i, t) in s.tracks.iter().enumerate() {
        t_stmt.execute(params![set_id, i as i64, kind_str(t.kind), t.name, t.color])?;
    }
    let mut d_stmt = conn.prepare(
        "INSERT INTO devices (set_id, track_ref, kind, name, manufacturer)
         VALUES (?1, ?2, ?3, ?4, ?5)",
    )?;
    for d in &s.devices {
        let kind = match d.kind {
            als_core::DeviceKind::Native => "native",
            als_core::DeviceKind::Au => "au",
            als_core::DeviceKind::Vst => "vst",
            als_core::DeviceKind::Vst3 => "vst3",
        };
        d_stmt.execute(params![set_id, track_ref_str(&d.track), kind, d.name, d.manufacturer])?;
    }
    let mut s_stmt = conn.prepare(
        "INSERT INTO samples (set_id, path, in_project, exists_on_disk) VALUES (?1, ?2, ?3, ?4)",
    )?;
    for smp in &s.samples {
        s_stmt.execute(params![set_id, smp.path, smp.in_project, smp.exists])?;
    }
    let mut l_stmt =
        conn.prepare("INSERT INTO locators (set_id, name, time) VALUES (?1, ?2, ?3)")?;
    for l in &s.locators {
        l_stmt.execute(params![set_id, l.name, l.time])?;
    }

    // FTS row: searchable names, space-joined.
    let project_name: String = conn.query_row(
        "SELECT name FROM projects WHERE id = ?1",
        params![project_id],
        |r| r.get(0),
    )?;
    let set_name = Path::new(&s.als_path)
        .file_stem()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    let join = |it: Vec<String>| it.join(" ");
    let track_names = join(s.tracks.iter().filter_map(|t| t.name.clone()).collect());
    let device_names = join(s.devices.iter().filter_map(|d| d.name.clone()).collect());
    let sample_names = join(
        s.samples
            .iter()
            .filter_map(|x| {
                Path::new(&x.path)
                    .file_name()
                    .map(|n| n.to_string_lossy().into_owned())
            })
            .collect(),
    );
    conn.execute(
        "INSERT INTO search (set_id, project_name, set_name, track_names, device_names, sample_names)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
        params![set_id, project_name, set_name, track_names, device_names, sample_names],
    )?;
    Ok(set_id)
}

/// Remove sets under `root_prefix` that no longer exist on disk, then orphan
/// projects and stale FTS rows. `seen` holds als_paths found this scan.
pub fn prune_missing(conn: &Connection, root_prefix: &str, seen: &HashSet<String>) -> Result<usize> {
    let like = format!("{}%", root_prefix);
    let mut stmt = conn.prepare("SELECT id, als_path FROM sets WHERE als_path LIKE ?1")?;
    let rows: Vec<(i64, String)> = stmt
        .query_map(params![like], |r| Ok((r.get(0)?, r.get(1)?)))?
        .collect::<rusqlite::Result<_>>()?;
    let mut removed = 0;
    for (id, path) in rows {
        if !seen.contains(&path) {
            conn.execute("DELETE FROM search WHERE set_id = ?1", params![id])?;
            conn.execute("DELETE FROM sets WHERE id = ?1", params![id])?;
            removed += 1;
        }
    }
    conn.execute(
        "DELETE FROM projects WHERE folder_path LIKE ?1
         AND id NOT IN (SELECT DISTINCT project_id FROM sets)",
        params![like],
    )?;
    Ok(removed)
}

pub struct SearchOpts {
    pub text: Option<String>,
    pub min_bpm: Option<f64>,
    pub max_bpm: Option<f64>,
    pub plugin: Option<String>,
}

pub struct SearchHit {
    pub set_id: i64,
    pub project: String,
    pub als_path: String,
    pub tempo: Option<f64>,
    pub time_signature: Option<String>,
    pub live_version: Option<String>,
}

pub fn search(conn: &Connection, o: &SearchOpts) -> Result<Vec<SearchHit>> {
    let mut stmt = conn.prepare(
        "SELECT s.id, p.name, s.als_path, s.tempo, s.time_signature, s.live_version
         FROM sets s JOIN projects p ON p.id = s.project_id
         WHERE (?1 IS NULL OR s.id IN (SELECT set_id FROM search WHERE search MATCH ?1))
           AND (?2 IS NULL OR s.tempo >= ?2)
           AND (?3 IS NULL OR s.tempo <= ?3)
           AND (?4 IS NULL OR EXISTS (SELECT 1 FROM devices d
                                      WHERE d.set_id = s.id
                                        AND d.name LIKE '%' || ?4 || '%'))
         ORDER BY p.name, s.als_path",
    )?;
    let rows = stmt.query_map(
        params![o.text, o.min_bpm, o.max_bpm, o.plugin],
        |r| {
            Ok(SearchHit {
                set_id: r.get(0)?,
                project: r.get(1)?,
                als_path: r.get(2)?,
                tempo: r.get(3)?,
                time_signature: r.get(4)?,
                live_version: r.get(5)?,
            })
        },
    )?;
    Ok(rows.collect::<rusqlite::Result<_>>()?)
}

pub struct Stats {
    pub projects: i64,
    pub sets: i64,
    pub tracks: i64,
    pub devices: i64,
    pub samples: i64,
    pub backups: i64,
}

pub fn stats(conn: &Connection) -> Result<Stats> {
    let count = |sql: &str| -> Result<i64> { Ok(conn.query_row(sql, [], |r| r.get(0))?) };
    Ok(Stats {
        projects: count("SELECT COUNT(*) FROM projects")?,
        sets: count("SELECT COUNT(*) FROM sets")?,
        tracks: count("SELECT COUNT(*) FROM tracks")?,
        devices: count("SELECT COUNT(*) FROM devices")?,
        samples: count("SELECT COUNT(*) FROM samples")?,
        backups: count("SELECT COUNT(*) FROM backups")?,
    })
}
