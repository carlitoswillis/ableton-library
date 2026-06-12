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

/// v2: previews. set_id NULL = project-level (ambiguous) preview.
const SCHEMA_V2: &str = r#"
CREATE TABLE IF NOT EXISTS previews (
    id         INTEGER PRIMARY KEY,
    set_id     INTEGER REFERENCES sets(id) ON DELETE CASCADE,
    project_id INTEGER REFERENCES projects(id) ON DELETE CASCADE,
    audio_path TEXT NOT NULL,
    source     TEXT NOT NULL,              -- discovered | worker | manual
    confidence REAL NOT NULL,
    mtime      TEXT NOT NULL,
    size       INTEGER NOT NULL,
    duration   REAL,
    peaks      TEXT,                       -- JSON array of 0..1 floats
    is_primary INTEGER NOT NULL DEFAULT 0
);
CREATE INDEX IF NOT EXISTS idx_previews_set  ON previews(set_id);
CREATE INDEX IF NOT EXISTS idx_previews_proj ON previews(project_id);
CREATE UNIQUE INDEX IF NOT EXISTS idx_previews_set_path ON previews(set_id, audio_path);
"#;

/// v3: export automation queue.
const SCHEMA_V3: &str = r#"
CREATE TABLE IF NOT EXISTS export_jobs (
    id           INTEGER PRIMARY KEY,
    set_id       INTEGER NOT NULL UNIQUE REFERENCES sets(id) ON DELETE CASCADE,
    status       TEXT NOT NULL,              -- pending | processing | completed | failed
    error        TEXT,
    created_at   TEXT NOT NULL,
    started_at   TEXT,
    completed_at TEXT
);
CREATE INDEX IF NOT EXISTS idx_export_jobs_status ON export_jobs(status);
"#;

/// Current schema version. Migrations upgrade older catalogs in place;
/// catalogs NEWER than this build are refused.
pub const SCHEMA_VERSION: i32 = 3;

/// Open (creating if needed) the index database, migrating if needed.
pub fn open(db_path: &Path) -> Result<Connection> {
    if let Some(dir) = db_path.parent() {
        std::fs::create_dir_all(dir)
            .with_context(|| format!("creating {}", dir.display()))?;
    }
    let conn = Connection::open(db_path)
        .with_context(|| format!("opening {}", db_path.display()))?;
    let _mode: String = conn.query_row("PRAGMA journal_mode=WAL", [], |r| r.get(0))?;
    conn.execute_batch("PRAGMA foreign_keys=ON;")?;

    let mut version: i32 = conn.query_row("PRAGMA user_version", [], |r| r.get(0))?;
    if version > SCHEMA_VERSION {
        anyhow::bail!(
            "catalog {} has schema v{version}, this build only knows v{SCHEMA_VERSION} — \
             update the app, or delete the db and rescan (it is rebuildable from your .als files)",
            db_path.display()
        );
    }
    if version == 0 {
        conn.execute_batch(SCHEMA)?;
        conn.execute_batch(SCHEMA_V2)?;
        conn.execute_batch(SCHEMA_V3)?;
        conn.execute_batch(&format!("PRAGMA user_version = {SCHEMA_VERSION}"))?;
        return Ok(conn);
    }
    // In-place migrations (each block is additive and idempotent).
    if version == 1 {
        conn.execute_batch(SCHEMA_V2)?;
        version = 2;
    }
    if version == 2 {
        conn.execute_batch(SCHEMA_V3)?;
        version = 3;
        conn.execute_batch(&format!("PRAGMA user_version = {version}"))?;
    }
    debug_assert_eq!(version, SCHEMA_VERSION);
    conn.execute_batch(SCHEMA)?; // idempotent (IF NOT EXISTS)
    conn.execute_batch(SCHEMA_V2)?;
    conn.execute_batch(SCHEMA_V3)?;
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

/// A render attached to a set (or project, when ambiguous).
pub struct PreviewRow {
    pub set_id: Option<i64>,
    pub project_id: Option<i64>,
    pub audio_path: String,
    pub source: String,
    pub confidence: f64,
    pub mtime: String,
    pub size: u64,
    pub duration: Option<f64>,
    /// JSON array of 0..1 floats (waveform bins), already serialized.
    pub peaks_json: Option<String>,
}

/// True if a preview row for (set_id, audio_path) exists with same size+mtime.
pub fn preview_is_fresh(
    conn: &Connection,
    set_id: Option<i64>,
    audio_path: &str,
    size: u64,
    mtime: &str,
) -> Result<bool> {
    let row: Option<(u64, String)> = conn
        .query_row(
            "SELECT size, mtime FROM previews WHERE audio_path = ?2
             AND ((?1 IS NULL AND set_id IS NULL) OR set_id = ?1)",
            params![set_id, audio_path],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .map(Some)
        .or_else(|e| match e {
            rusqlite::Error::QueryReturnedNoRows => Ok(None),
            e => Err(e),
        })?;
    Ok(row.map_or(false, |(s, m)| s == size && m == mtime))
}

/// Insert or replace a preview row, then recompute the set's primary.
pub fn upsert_preview(conn: &Connection, p: &PreviewRow) -> Result<()> {
    conn.execute(
        "DELETE FROM previews WHERE audio_path = ?2
         AND ((?1 IS NULL AND set_id IS NULL) OR set_id = ?1)",
        params![p.set_id, p.audio_path],
    )?;
    conn.execute(
        "INSERT INTO previews (set_id, project_id, audio_path, source, confidence,
                               mtime, size, duration, peaks, is_primary)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, 0)",
        params![
            p.set_id, p.project_id, p.audio_path, p.source, p.confidence,
            p.mtime, p.size, p.duration, p.peaks_json
        ],
    )?;
    if let Some(set_id) = p.set_id {
        recompute_primary(conn, set_id)?;
    }
    Ok(())
}

/// Primary = highest confidence, newest file.
pub fn recompute_primary(conn: &Connection, set_id: i64) -> Result<()> {
    conn.execute("UPDATE previews SET is_primary = 0 WHERE set_id = ?1", params![set_id])?;
    conn.execute(
        "UPDATE previews SET is_primary = 1 WHERE id =
           (SELECT id FROM previews WHERE set_id = ?1
            ORDER BY confidence DESC, mtime DESC LIMIT 1)",
        params![set_id],
    )?;
    Ok(())
}

/// Primary preview for a set: (audio_path, duration, peaks_json, confidence, source).
pub fn primary_preview(
    conn: &Connection,
    set_id: i64,
) -> Result<Option<(String, Option<f64>, Option<String>, f64, String)>> {
    conn.query_row(
        "SELECT audio_path, duration, peaks, confidence, source
         FROM previews WHERE set_id = ?1 AND is_primary = 1",
        params![set_id],
        |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?)),
    )
    .map(Some)
    .or_else(|e| match e {
        rusqlite::Error::QueryReturnedNoRows => Ok(None),
        e => Err(e.into()),
    })
}

pub struct SearchOpts {
    pub text: Option<String>,
    pub min_bpm: Option<f64>,
    pub max_bpm: Option<f64>,
    pub plugin: Option<String>,
}

#[derive(serde::Serialize)]
pub struct SearchHit {
    pub set_id: i64,
    pub project: String,
    pub als_path: String,
    pub tempo: Option<f64>,
    pub time_signature: Option<String>,
    pub live_version: Option<String>,
    pub has_preview: bool,
    pub preview_duration: Option<f64>,
}

pub fn search(conn: &Connection, o: &SearchOpts) -> Result<Vec<SearchHit>> {
    // With text: rank by weighted bm25 so name matches beat content matches.
    // Column weights:        set_id, project, set, tracks, devices, samples
    // (set/project names matter most; a plugin or sample hit still surfaces,
    // just far below sets that match by name.)
    let sql = if o.text.is_some() {
        "SELECT s.id, p.name, s.als_path, s.tempo, s.time_signature, s.live_version,
                pv.audio_path, pv.duration
         FROM search f
         JOIN sets s ON s.id = f.set_id
         JOIN projects p ON p.id = s.project_id
         LEFT JOIN previews pv ON pv.set_id = s.id AND pv.is_primary = 1
         WHERE f.search MATCH ?1
           AND (?2 IS NULL OR s.tempo >= ?2)
           AND (?3 IS NULL OR s.tempo <= ?3)
           AND (?4 IS NULL OR EXISTS (SELECT 1 FROM devices d
                                      WHERE d.set_id = s.id
                                        AND d.name LIKE '%' || ?4 || '%'))
         ORDER BY bm25(f.search, 0.0, 8.0, 10.0, 4.0, 1.0, 0.5), p.name, s.als_path"
    } else {
        "SELECT s.id, p.name, s.als_path, s.tempo, s.time_signature, s.live_version,
                pv.audio_path, pv.duration
         FROM sets s
         JOIN projects p ON p.id = s.project_id
         LEFT JOIN previews pv ON pv.set_id = s.id AND pv.is_primary = 1
         WHERE ?1 IS NULL
           AND (?2 IS NULL OR s.tempo >= ?2)
           AND (?3 IS NULL OR s.tempo <= ?3)
           AND (?4 IS NULL OR EXISTS (SELECT 1 FROM devices d
                                      WHERE d.set_id = s.id
                                        AND d.name LIKE '%' || ?4 || '%'))
         ORDER BY p.name, s.als_path"
    };
    let mut stmt = conn.prepare(sql)?;
    let rows = stmt.query_map(
        params![o.text, o.min_bpm, o.max_bpm, o.plugin],
        |r| {
            let preview_path: Option<String> = r.get(6)?;
            Ok(SearchHit {
                set_id: r.get(0)?,
                project: r.get(1)?,
                als_path: r.get(2)?,
                tempo: r.get(3)?,
                time_signature: r.get(4)?,
                live_version: r.get(5)?,
                has_preview: preview_path.is_some(),
                preview_duration: r.get(7)?,
            })
        },
    )?;
    Ok(rows.collect::<rusqlite::Result<_>>()?)
}

#[derive(serde::Serialize)]
pub struct Stats {
    pub projects: i64,
    pub sets: i64,
    pub tracks: i64,
    pub devices: i64,
    pub samples: i64,
    pub backups: i64,
    pub previews: i64,
    pub export_jobs: i64,
}

/// Resolve a set reference: numeric id, exact als_path, or path fragment.
pub fn resolve_set(conn: &Connection, query: &str) -> Result<i64> {
    if let Ok(id) = query.parse::<i64>() {
        return Ok(id);
    }
    Ok(conn
        .query_row(
            "SELECT id FROM sets WHERE als_path = ?1
             OR als_path LIKE '%' || ?1 || '%' LIMIT 1",
            params![query],
            |r| r.get(0),
        )
        .with_context(|| format!("no set matching '{query}'"))?)
}

/// Every sample path referenced by any indexed set — used by render discovery
/// to guarantee a known sample is never mistaken for a bounce.
pub fn all_sample_paths(conn: &Connection) -> Result<std::collections::HashSet<String>> {
    let mut stmt = conn.prepare("SELECT DISTINCT path FROM samples")?;
    let rows = stmt.query_map([], |r| r.get::<_, String>(0))?;
    Ok(rows.collect::<rusqlite::Result<_>>()?)
}

/// Raw matcher inputs: (set_id, project_id, als_path, project_name, project_set_count).
pub fn set_match_candidates(conn: &Connection) -> Result<Vec<(i64, i64, String, String, i64)>> {
    let mut stmt = conn.prepare(
        "SELECT s.id, p.id, s.als_path, p.name,
                (SELECT COUNT(*) FROM sets s2 WHERE s2.project_id = p.id)
         FROM sets s JOIN projects p ON p.id = s.project_id",
    )?;
    let rows = stmt.query_map([], |r| {
        Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?))
    })?;
    Ok(rows.collect::<rusqlite::Result<_>>()?)
}

/// (set_id, als_path) for every set in a project.
pub fn project_sets(conn: &Connection, project_id: i64) -> Result<Vec<(i64, String)>> {
    let mut stmt =
        conn.prepare("SELECT id, als_path FROM sets WHERE project_id = ?1 ORDER BY als_path")?;
    let rows = stmt.query_map(params![project_id], |r| Ok((r.get(0)?, r.get(1)?)))?;
    Ok(rows.collect::<rusqlite::Result<_>>()?)
}

/// The project a set belongs to.
pub fn set_project_id(conn: &Connection, set_id: i64) -> Result<i64> {
    Ok(conn
        .query_row(
            "SELECT project_id FROM sets WHERE id = ?1",
            params![set_id],
            |r| r.get(0),
        )
        .with_context(|| format!("no set with id {set_id}"))?)
}

/// The stored als_path for one set.
pub fn set_path(conn: &Connection, set_id: i64) -> Result<String> {
    Ok(conn
        .query_row(
            "SELECT als_path FROM sets WHERE id = ?1",
            params![set_id],
            |r| r.get(0),
        )
        .with_context(|| format!("no set with id {set_id}"))?)
}

/// Full stored detail for one set as JSON (shared by CLI inspect and the app).
pub fn set_detail(conn: &Connection, set_id: i64) -> Result<serde_json::Value> {
    let mut out = serde_json::Map::new();
    conn.query_row(
        "SELECT s.als_path, s.live_version, s.tempo, s.time_signature, s.warnings, p.name
         FROM sets s JOIN projects p ON p.id = s.project_id WHERE s.id = ?1",
        params![set_id],
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
        let mut rows = stmt.query(params![set_id])?;
        let mut arr = Vec::new();
        while let Some(row) = rows.next()? {
            let mut obj = serde_json::Map::new();
            for (i, c) in cols.iter().enumerate() {
                let v: rusqlite::types::Value = row.get(i)?;
                obj.insert(
                    (*c).into(),
                    match v {
                        rusqlite::types::Value::Null => serde_json::Value::Null,
                        rusqlite::types::Value::Integer(n) => n.into(),
                        rusqlite::types::Value::Real(f) => f.into(),
                        rusqlite::types::Value::Text(s) => s.into(),
                        rusqlite::types::Value::Blob(_) => serde_json::Value::Null,
                    },
                );
            }
            arr.push(serde_json::Value::Object(obj));
        }
        Ok(serde_json::Value::Array(arr))
    };
    out.insert(
        "tracks".into(),
        list(
            "SELECT idx, kind, name, color FROM tracks WHERE set_id = ?1 ORDER BY idx",
            &["idx", "kind", "name", "color"],
        )?,
    );
    out.insert(
        "devices".into(),
        list(
            "SELECT track_ref, kind, name, manufacturer FROM devices WHERE set_id = ?1",
            &["track", "kind", "name", "manufacturer"],
        )?,
    );
    out.insert(
        "samples".into(),
        list(
            "SELECT path, in_project, exists_on_disk FROM samples WHERE set_id = ?1",
            &["path", "in_project", "exists_on_disk"],
        )?,
    );
    out.insert(
        "locators".into(),
        list("SELECT name, time FROM locators WHERE set_id = ?1", &["name", "time"])?,
    );
    Ok(serde_json::Value::Object(out))
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
        previews: count("SELECT COUNT(*) FROM previews")?,
        export_jobs: count("SELECT COUNT(*) FROM export_jobs")?,
    })
}

#[derive(serde::Serialize)]
pub struct ExportJobInfo {
    pub id: i64,
    pub set_id: i64,
    pub als_path: String,
    pub project_name: String,
    pub status: String,
    pub error: Option<String>,
    pub created_at: String,
    pub started_at: Option<String>,
    pub completed_at: Option<String>,
}

pub fn add_export_job(conn: &Connection, set_id: i64) -> Result<()> {
    let now = chrono::Utc::now().format("%Y-%m-%dT%H:%M:%S+00:00").to_string();
    conn.execute(
        "INSERT INTO export_jobs (set_id, status, created_at)
         VALUES (?1, 'pending', ?2)
         ON CONFLICT(set_id) DO UPDATE SET
            status = 'pending',
            created_at = ?2,
            started_at = NULL,
            completed_at = NULL,
            error = NULL",
        params![set_id, now],
    )?;
    Ok(())
}

pub fn get_pending_export_job(conn: &Connection) -> Result<Option<(i64, i64, String)>> {
    let mut stmt = conn.prepare(
        "SELECT j.id, j.set_id, s.als_path
         FROM export_jobs j
         JOIN sets s ON s.id = j.set_id
         WHERE j.status = 'pending'
         ORDER BY j.id ASC LIMIT 1",
    )?;
    let row = stmt.query_row([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))
        .map(Some)
        .or_else(|e| match e {
            rusqlite::Error::QueryReturnedNoRows => Ok(None),
            e => Err(e),
        })?;
    Ok(row)
}

pub fn update_export_job_status(
    conn: &Connection,
    job_id: i64,
    status: &str,
    error: Option<&str>,
) -> Result<()> {
    let now = chrono::Utc::now().format("%Y-%m-%dT%H:%M:%S+00:00").to_string();
    match status {
        "processing" => {
            conn.execute(
                "UPDATE export_jobs SET status = ?1, started_at = ?2, error = NULL WHERE id = ?3",
                params![status, now, job_id],
            )?;
        }
        "completed" | "failed" => {
            conn.execute(
                "UPDATE export_jobs SET status = ?1, completed_at = ?2, error = ?3 WHERE id = ?4",
                params![status, now, error, job_id],
            )?;
        }
        _ => {
            conn.execute(
                "UPDATE export_jobs SET status = ?1, error = ?2 WHERE id = ?3",
                params![status, error, job_id],
            )?;
        }
    }
    Ok(())
}

pub fn get_export_queue(conn: &Connection) -> Result<Vec<ExportJobInfo>> {
    let mut stmt = conn.prepare(
        "SELECT j.id, j.set_id, s.als_path, p.name, j.status, j.error, j.created_at, j.started_at, j.completed_at
         FROM export_jobs j
         JOIN sets s ON s.id = j.set_id
         JOIN projects p ON p.id = s.project_id
         ORDER BY j.id DESC",
    )?;
    let rows = stmt.query_map([], |r| {
        Ok(ExportJobInfo {
            id: r.get(0)?,
            set_id: r.get(1)?,
            als_path: r.get(2)?,
            project_name: r.get(3)?,
            status: r.get(4)?,
            error: r.get(5)?,
            created_at: r.get(6)?,
            started_at: r.get(7)?,
            completed_at: r.get(8)?,
        })
    })?;
    Ok(rows.collect::<rusqlite::Result<_>>()?)
}

pub fn clear_completed_export_jobs(conn: &Connection) -> Result<()> {
    conn.execute(
        "DELETE FROM export_jobs WHERE status IN ('completed', 'failed')",
        [],
    )?;
    Ok(())
}

pub fn remove_export_job(conn: &Connection, job_id: i64) -> Result<()> {
    conn.execute("DELETE FROM export_jobs WHERE id = ?1", params![job_id])?;
    Ok(())
}
