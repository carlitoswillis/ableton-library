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

/// v4: watch folders and ignored matches.
const SCHEMA_V4: &str = r#"
CREATE TABLE IF NOT EXISTS watch_folders (
    id   INTEGER PRIMARY KEY,
    path TEXT UNIQUE NOT NULL
);
CREATE TABLE IF NOT EXISTS ignored_matches (
    set_id     INTEGER NOT NULL REFERENCES sets(id) ON DELETE CASCADE,
    audio_path TEXT NOT NULL,
    PRIMARY KEY (set_id, audio_path)
);
"#;

/// v5: tempos json list
const SCHEMA_V5: &str = r#"
ALTER TABLE sets ADD COLUMN tempos_json TEXT NOT NULL DEFAULT '[]';
"#;

/// v6: renderability triage (M4a) — score + fidelity report on jobs, and
/// fidelity carried onto worker-generated previews.
const SCHEMA_V6: &str = r#"
ALTER TABLE export_jobs ADD COLUMN score REAL;
ALTER TABLE export_jobs ADD COLUMN fidelity TEXT;
ALTER TABLE previews ADD COLUMN fidelity TEXT;
"#;

/// v7: artist as a first-class project attribute. Derived from the folder
/// path at scan time (or set explicitly via the scanner's override), so the
/// catalog can be filtered/grouped by artist. NULL = unknown (e.g. pure
/// year/month layouts). Existing rows stay NULL until the next rescan
/// re-derives them (upsert_project runs on every scan).
const SCHEMA_V7: &str = r#"
ALTER TABLE projects ADD COLUMN artist TEXT;
CREATE INDEX IF NOT EXISTS idx_projects_artist ON projects(artist);
"#;

/// v8: per-set artist override. A manual tag on one set; the set's effective
/// artist is `COALESCE(sets.artist_override, projects.artist)` — so auto/
/// project-level derivation stays the default and a hand-tag overrides just
/// that set. Untouched by scan/reindex (those only write projects.artist), so
/// manual per-set tags survive a rescan.
const SCHEMA_V8: &str = r#"
ALTER TABLE sets ADD COLUMN artist_override TEXT;
CREATE INDEX IF NOT EXISTS idx_sets_artist_override ON sets(artist_override);
"#;

/// v9: user-curated lists (favorites + named collections). Many-to-many: a set
/// can be in many lists. Membership is keyed by `als_path` (the set's stable
/// identity), NOT set_id — so lists SURVIVE re-ingest, which deletes+reinserts
/// the set row (and would otherwise orphan a row-id FK). Deleting a list
/// cascades its items; pruned sets just leave harmless orphan rows that
/// reattach if the set returns.
const SCHEMA_V9: &str = r#"
CREATE TABLE IF NOT EXISTS lists (
    id         INTEGER PRIMARY KEY,
    name       TEXT NOT NULL,
    created_at TEXT NOT NULL
);
CREATE UNIQUE INDEX IF NOT EXISTS idx_lists_name ON lists(name COLLATE NOCASE);
CREATE TABLE IF NOT EXISTS list_items (
    list_id  INTEGER NOT NULL REFERENCES lists(id) ON DELETE CASCADE,
    als_path TEXT NOT NULL,
    added_at TEXT NOT NULL,
    PRIMARY KEY (list_id, als_path)
);
CREATE INDEX IF NOT EXISTS idx_list_items_path ON list_items(als_path);
"#;

/// Current schema version. Migrations upgrade older catalogs in place;
/// catalogs NEWER than this build are refused.
pub const SCHEMA_VERSION: i32 = 9;

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
        conn.execute_batch(SCHEMA_V4)?;
        // v5..v8 are ALTER TABLEs; run them after base creation.
        conn.execute_batch(SCHEMA_V5)?;
        conn.execute_batch(SCHEMA_V6)?;
        conn.execute_batch(SCHEMA_V7)?;
        conn.execute_batch(SCHEMA_V8)?;
        conn.execute_batch(SCHEMA_V9)?;
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
    }
    if version == 3 {
        conn.execute_batch(SCHEMA_V4)?;
        version = 4;
    }
    if version == 4 {
        conn.execute_batch(SCHEMA_V5)?;
        version = 5;
    }
    if version == 5 {
        conn.execute_batch(SCHEMA_V6)?;
        version = 6;
    }
    if version == 6 {
        conn.execute_batch(SCHEMA_V7)?;
        version = 7;
    }
    if version == 7 {
        conn.execute_batch(SCHEMA_V8)?;
        version = 8;
    }
    if version == 8 {
        conn.execute_batch(SCHEMA_V9)?;
        version = 9;
    }
    conn.execute_batch(&format!("PRAGMA user_version = {version}"))?;
    debug_assert_eq!(version, SCHEMA_VERSION);
    conn.execute_batch(SCHEMA)?; // idempotent (IF NOT EXISTS)
    conn.execute_batch(SCHEMA_V2)?;
    conn.execute_batch(SCHEMA_V3)?;
    conn.execute_batch(SCHEMA_V4)?;
    conn.execute_batch(SCHEMA_V9)?; // CREATE TABLE IF NOT EXISTS — safe to re-run
    // v5..v8 are ALTER TABLEs, not idempotent, so we don't re-run them blindly.
    Ok(conn)
}

/// Insert or refresh a project row. `artist` is the path-derived (or
/// explicitly-overridden) artist; `None` means "unknown / leave as-is".
/// On conflict we COALESCE — a later broad scan that can't infer an artist
/// won't wipe one that an earlier targeted scan (or `--artist`) established.
pub fn upsert_project(
    conn: &Connection,
    folder_path: &str,
    name: &str,
    artist: Option<&str>,
    now: &str,
) -> Result<i64> {
    conn.execute(
        "INSERT INTO projects (folder_path, name, artist, last_scanned) VALUES (?1, ?2, ?3, ?4)
         ON CONFLICT(folder_path) DO UPDATE SET
            name = ?2,
            artist = COALESCE(?3, artist),
            last_scanned = ?4",
        params![folder_path, name, artist, now],
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
                           live_version, schema_version, tempo, tempos_json, time_signature, warnings)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)",
        params![
            project_id,
            s.als_path,
            s.file_size,
            s.mtime,
            s.content_hash,
            s.live_version,
            s.schema_version,
            s.tempo,
            serde_json::to_string(&s.tempos)?,
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
    /// JSON renderability/fidelity report (worker renders of imperfect sets),
    /// e.g. {"missing_plugins":["Serum"],"samples_missing":3}. None = full fidelity assumed.
    pub fidelity_json: Option<String>,
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
            "SELECT size, mtime FROM previews WHERE audio_path = ?2 COLLATE NOCASE
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
                               mtime, size, duration, peaks, fidelity, is_primary)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, 0)",
        params![
            p.set_id, p.project_id, p.audio_path, p.source, p.confidence,
            p.mtime, p.size, p.duration, p.peaks_json, p.fidelity_json
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
) -> Result<Option<(String, Option<f64>, Option<String>, f64, String, Option<String>)>> {
    conn.query_row(
        "SELECT audio_path, duration, peaks, confidence, source, fidelity
         FROM previews WHERE set_id = ?1 AND is_primary = 1",
        params![set_id],
        |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?, r.get(5)?)),
    )
    .map(Some)
    .or_else(|e| match e {
        rusqlite::Error::QueryReturnedNoRows => Ok(None),
        e => Err(e.into()),
    })
}

/// Get just the confidence and mtime of the primary preview (fast check).
pub fn primary_preview_stats(conn: &Connection, set_id: i64) -> Result<Option<(f64, String)>> {
    conn.query_row(
        "SELECT confidence, mtime FROM previews WHERE set_id = ?1 AND is_primary = 1",
        params![set_id],
        |r| Ok((r.get(0)?, r.get(1)?)),
    )
    .map(Some)
    .or_else(|e| match e {
        rusqlite::Error::QueryReturnedNoRows => Ok(None),
        e => Err(e.into()),
    })
}

/// Remove the primary preview for a set, deleting its file from disk if it is a sketch.
/// Returns the path of the deleted file if successful.
pub fn remove_preview(conn: &Connection, set_id: i64) -> Result<Option<String>> {
    let preview: Option<(i64, String, String)> = conn.query_row(
        "SELECT id, audio_path, source FROM previews WHERE set_id = ?1 AND is_primary = 1",
        params![set_id],
        |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
    ).map(Some).or_else(|e| match e {
        rusqlite::Error::QueryReturnedNoRows => Ok(None),
        e => Err(e),
    })?;

    if let Some((id, audio_path, source)) = preview {
        // If it's a sketch, delete the file.
        if source == "sketch" {
            let _ = std::fs::remove_file(&audio_path);
        }
        
        conn.execute("DELETE FROM previews WHERE id = ?1", params![id])?;
        recompute_primary(conn, set_id)?;
        Ok(Some(audio_path))
    } else {
        Ok(None)
    }
}

/// Delete preview rows whose audio files no longer exist on disk.
/// Returns a list of deleted preview audio paths.
pub fn prune_stale_previews(conn: &Connection) -> Result<Vec<(i64, String)>> {
    let mut stmt = conn.prepare("SELECT id, set_id, audio_path FROM previews")?;
    let rows = stmt.query_map([], |r| {
        Ok((r.get::<_, i64>(0)?, r.get::<_, Option<i64>>(1)?, r.get::<_, String>(2)?))
    })?;

    let mut stale = Vec::new();
    let mut to_delete = Vec::new();
    for row in rows {
        let (id, set_id, audio_path) = row?;
        if !std::path::Path::new(&audio_path).exists() {
            to_delete.push(id);
            stale.push((set_id.unwrap_or(0), audio_path));
        }
    }

    for id in to_delete {
        conn.execute("DELETE FROM previews WHERE id = ?1", params![id])?;
    }

    let mut affected_sets = std::collections::HashSet::new();
    for &(set_id, _) in &stale {
        if set_id > 0 {
            affected_sets.insert(set_id);
        }
    }
    for set_id in affected_sets {
        recompute_primary(conn, set_id)?;
    }

    Ok(stale)
}


pub struct SearchOpts {
    pub text: Option<String>,
    pub min_bpm: Option<f64>,
    pub max_bpm: Option<f64>,
    pub plugin: Option<String>,
    /// Substring match (case-insensitive) on the project's derived artist.
    pub artist: Option<String>,
    /// Restrict to sets that are members of this list.
    pub list_id: Option<i64>,
    pub sort_by: Option<String>,
    pub date_modified: Option<String>,
    pub date_scanned: Option<String>,
    pub has_preview: Option<String>,
}

#[derive(serde::Serialize)]
pub struct SearchHit {
    pub set_id: i64,
    pub project: String,
    pub artist: Option<String>,
    pub als_path: String,
    pub tempo: Option<f64>,
    pub tempos: Vec<f64>,
    pub time_signature: Option<String>,
    pub live_version: Option<String>,
    pub has_preview: bool,
    pub preview_source: Option<String>,
    pub preview_duration: Option<f64>,
    /// True if this set belongs to at least one user list (drives the star).
    pub in_list: bool,
}

/// Turn raw user search text into a SAFE FTS5 MATCH query.
///
/// User input was previously fed straight to FTS5, where characters like
/// `.`, `-`, `(`, `:`, `*`, `^` and `"` are QUERY OPERATORS — so typing
/// `131.10` made FTS5 read `.` as a column filter and raise
/// `fts5: syntax error near "."`. Library names are full of these
/// (`be 131.10 bpm`, `2 113.10 bpm`, `tisa - taco bell`, `nasty (prod…)`).
///
/// Strategy: pull out alphanumeric tokens only (the unicode61 tokenizer already
/// splits on punctuation at INDEX time, so dropping it here matches what's
/// actually stored — `131.10` is indexed as the tokens `131` + `10`), wrap each
/// token in double quotes to neutralise every special char, and append `*` for
/// prefix matching (preserves the as-you-type feel). Tokens are space-joined,
/// i.e. implicit AND. Returns `None` when the input yields no usable token
/// (e.g. all punctuation) so the caller skips the text filter entirely instead
/// of issuing an empty or invalid MATCH.
fn fts_query(raw: &str) -> Option<String> {
    let mut terms: Vec<String> = Vec::new();
    let mut cur = String::new();
    for ch in raw.chars() {
        if ch.is_alphanumeric() {
            cur.push(ch);
        } else if !cur.is_empty() {
            terms.push(std::mem::take(&mut cur));
        }
    }
    if !cur.is_empty() {
        terms.push(cur);
    }
    if terms.is_empty() {
        return None;
    }
    Some(
        terms
            .iter()
            .map(|t| format!("\"{t}\"*"))
            .collect::<Vec<_>>()
            .join(" "),
    )
}

pub fn search(conn: &Connection, o: &SearchOpts) -> Result<Vec<SearchHit>> {
    // Sanitize free-text into a safe FTS5 query up front; everything below keys
    // off `fts` (not the raw text) so punctuation-only input cleanly degrades to
    // "no text filter" instead of erroring.
    let fts = o.text.as_deref().and_then(fts_query);
    let order_by = match o.sort_by.as_deref() {
        Some("modified") => "s.mtime DESC, p.name, s.als_path",
        Some("bpm") => "s.tempo DESC, p.name, s.als_path",
        Some("artist") => "COALESCE(s.artist_override, p.artist) IS NULL, COALESCE(s.artist_override, p.artist), p.name, s.als_path",
        Some("previews") => "pv.audio_path IS NULL, p.name, s.als_path",
        _ => {
            if fts.is_some() {
                "bm25(f.search, 0.0, 8.0, 10.0, 4.0, 1.0, 0.5), p.name, s.als_path"
            } else {
                "p.name, s.als_path"
            }
        }
    };

    let modified_bound = match o.date_modified.as_deref() {
        Some("today") => Some(chrono::Utc::now() - chrono::Duration::days(1)),
        Some("yesterday") => Some(chrono::Utc::now() - chrono::Duration::days(2)),
        Some("week") => Some(chrono::Utc::now() - chrono::Duration::days(7)),
        Some("month") => Some(chrono::Utc::now() - chrono::Duration::days(30)),
        _ => None,
    };
    let modified_bound_str = modified_bound.map(|dt| dt.format("%Y-%m-%dT%H:%M:%S+00:00").to_string());

    let scanned_bound = match o.date_scanned.as_deref() {
        Some("today") => Some(chrono::Utc::now() - chrono::Duration::days(1)),
        Some("yesterday") => Some(chrono::Utc::now() - chrono::Duration::days(2)),
        Some("week") => Some(chrono::Utc::now() - chrono::Duration::days(7)),
        Some("month") => Some(chrono::Utc::now() - chrono::Duration::days(30)),
        _ => None,
    };
    let scanned_bound_str = scanned_bound.map(|dt| dt.format("%Y-%m-%dT%H:%M:%S+00:00").to_string());

    let sql = if fts.is_some() {
        format!(
            "SELECT s.id, p.name, s.als_path, s.tempo, s.tempos_json, s.time_signature, s.live_version,
                    pv.audio_path, pv.duration, COALESCE(s.artist_override, p.artist),
                    EXISTS (SELECT 1 FROM list_items li WHERE li.als_path = s.als_path),
                    pv.source
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
               AND (?5 IS NULL OR s.mtime >= ?5)
               AND (?6 IS NULL OR p.last_scanned >= ?6)
               AND (?7 IS NULL OR ?7 = 'all' OR (?7 = 'yes' AND pv.audio_path IS NOT NULL AND pv.source != 'sketch') OR (?7 = 'no' AND (pv.audio_path IS NULL OR pv.source = 'sketch')))
               AND (?8 IS NULL OR COALESCE(s.artist_override, p.artist) LIKE '%' || ?8 || '%')
               AND (?9 IS NULL OR EXISTS (SELECT 1 FROM list_items li2
                                          WHERE li2.als_path = s.als_path AND li2.list_id = ?9))
             ORDER BY {}",
            order_by
        )
    } else {
        format!(
            "SELECT s.id, p.name, s.als_path, s.tempo, s.tempos_json, s.time_signature, s.live_version,
                    pv.audio_path, pv.duration, COALESCE(s.artist_override, p.artist),
                    EXISTS (SELECT 1 FROM list_items li WHERE li.als_path = s.als_path),
                    pv.source
             FROM sets s
             JOIN projects p ON p.id = s.project_id
             LEFT JOIN previews pv ON pv.set_id = s.id AND pv.is_primary = 1
             WHERE ?1 IS NULL
               AND (?2 IS NULL OR s.tempo >= ?2)
               AND (?3 IS NULL OR s.tempo <= ?3)
               AND (?4 IS NULL OR EXISTS (SELECT 1 FROM devices d
                                          WHERE d.set_id = s.id
                                            AND d.name LIKE '%' || ?4 || '%'))
               AND (?5 IS NULL OR s.mtime >= ?5)
               AND (?6 IS NULL OR p.last_scanned >= ?6)
               AND (?7 IS NULL OR ?7 = 'all' OR (?7 = 'yes' AND pv.audio_path IS NOT NULL AND pv.source != 'sketch') OR (?7 = 'no' AND (pv.audio_path IS NULL OR pv.source = 'sketch')))
               AND (?8 IS NULL OR COALESCE(s.artist_override, p.artist) LIKE '%' || ?8 || '%')
               AND (?9 IS NULL OR EXISTS (SELECT 1 FROM list_items li2
                                          WHERE li2.als_path = s.als_path AND li2.list_id = ?9))
             ORDER BY {}",
            order_by
        )
    };
    let mut stmt = conn.prepare(&sql)?;
    let rows = stmt.query_map(
        params![
            fts,
            o.min_bpm,
            o.max_bpm,
            o.plugin,
            modified_bound_str,
            scanned_bound_str,
            o.has_preview,
            o.artist,
            o.list_id,
        ],
        |r| {
            let preview_path: Option<String> = r.get(7)?;
            let tempos_json: String = r.get(4)?;
            let tempos: Vec<f64> = serde_json::from_str(&tempos_json).unwrap_or_default();
            Ok(SearchHit {
                set_id: r.get(0)?,
                project: r.get(1)?,
                artist: r.get(9)?,
                als_path: r.get(2)?,
                tempo: r.get(3)?,
                tempos,
                time_signature: r.get(5)?,
                live_version: r.get(6)?,
                has_preview: preview_path.is_some(),
                preview_source: r.get(11)?,
                preview_duration: r.get(8)?,
                in_list: r.get::<_, i64>(10)? != 0,
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

/// All indexed sample paths sharing a basename — the catalog-as-search-index
/// behind missing-sample relinking (we know where files moved to because
/// some other set references them at their new home).
pub fn sample_paths_by_basename(conn: &Connection, basename: &str) -> Result<Vec<String>> {
    let mut stmt = conn.prepare(
        "SELECT DISTINCT path FROM samples WHERE path LIKE '%/' || ?1",
    )?;
    let rows = stmt.query_map(params![basename], |r| r.get(0))?;
    Ok(rows.collect::<rusqlite::Result<_>>()?)
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
        "SELECT s.als_path, s.live_version, s.tempo, s.tempos_json, s.time_signature, s.warnings,
                p.name, p.artist, s.artist_override
         FROM sets s JOIN projects p ON p.id = s.project_id WHERE s.id = ?1",
        params![set_id],
        |r| {
            let project_artist: Option<String> = r.get(7)?;
            let artist_override: Option<String> = r.get(8)?;
            out.insert("set_id".into(), set_id.into());
            out.insert("project".into(), r.get::<_, String>(6)?.into());
            // Effective artist = per-set override else the project's derived one.
            out.insert(
                "artist".into(),
                artist_override.clone().or_else(|| project_artist.clone()).into(),
            );
            out.insert("artist_override".into(), artist_override.into());
            out.insert("project_artist".into(), project_artist.into());
            out.insert("als_path".into(), r.get::<_, String>(0)?.into());
            out.insert("live_version".into(), r.get::<_, Option<String>>(1)?.into());
            out.insert("tempo".into(), r.get::<_, Option<f64>>(2)?.into());
            
            let t_json: String = r.get(3)?;
            out.insert(
                "tempos".into(),
                serde_json::from_str(&t_json).unwrap_or(serde_json::Value::Array(Vec::new()))
            );
            
            out.insert("time_signature".into(), r.get::<_, Option<String>>(4)?.into());
            let w: String = r.get(5)?;
            out.insert(
                "warnings".into(),
                serde_json::from_str(&w).unwrap_or(serde_json::Value::Null),
            );
            Ok(())
        },
    )
    .with_context(|| format!("no set with id {set_id}"))?;

    // Query preview details
    let preview: Option<(i64, String)> = conn
        .query_row(
            "SELECT id, audio_path FROM previews WHERE set_id = ?1 AND is_primary = 1",
            params![set_id],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .map(Some)
        .or_else(|e| match e {
            rusqlite::Error::QueryReturnedNoRows => Ok(None),
            e => Err(e),
        })?;

    let mut preview_missing = false;
    let mut has_preview = false;
    let mut preview_path = None;

    if let Some((id, path)) = preview {
        if std::path::Path::new(&path).exists() {
            has_preview = true;
            preview_path = Some(path);
        } else {
            // Preview is missing from disk! Delete it from the database and note it.
            conn.execute("DELETE FROM previews WHERE id = ?1", params![id])?;
            recompute_primary(conn, set_id)?;
            preview_missing = true;
        }
    }

    out.insert("has_preview".into(), has_preview.into());
    out.insert("preview_path".into(), preview_path.into());
    out.insert("preview_missing".into(), preview_missing.into());


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

/// (id, folder_path) for every indexed project — used by the no-scan artist
/// backfill, which re-derives artist from the path already on record.
pub fn all_projects(conn: &Connection) -> Result<Vec<(i64, String)>> {
    let mut stmt = conn.prepare("SELECT id, folder_path FROM projects")?;
    let rows = stmt.query_map([], |r| Ok((r.get(0)?, r.get(1)?)))?;
    Ok(rows.collect::<rusqlite::Result<_>>()?)
}

/// Set a project's artist directly (the reindex/backfill path). Unlike
/// `upsert_project` this overwrites unconditionally — the caller only calls it
/// when it has a derived value, so it won't clobber with NULL.
pub fn set_project_artist(conn: &Connection, project_id: i64, artist: &str) -> Result<()> {
    conn.execute(
        "UPDATE projects SET artist = ?2 WHERE id = ?1",
        params![project_id, artist],
    )?;
    Ok(())
}

/// Manually set or clear a project's artist (`None` writes NULL). This is the
/// user's explicit assignment — used when the path-deriver found nothing.
pub fn set_project_artist_opt(
    conn: &Connection,
    project_id: i64,
    artist: Option<&str>,
) -> Result<()> {
    conn.execute(
        "UPDATE projects SET artist = ?2 WHERE id = ?1",
        params![project_id, artist],
    )?;
    Ok(())
}

/// Distinct artists with how many SETS each has (by effective artist =
/// per-set override else project's), most-sets first. Sets with no artist at
/// all are grouped under a `None` key (listed last) for an "Unknown" bucket.
pub fn list_artists(conn: &Connection) -> Result<Vec<(Option<String>, i64)>> {
    let mut stmt = conn.prepare(
        "SELECT COALESCE(s.artist_override, p.artist) AS artist, COUNT(*) AS n
         FROM sets s JOIN projects p ON p.id = s.project_id
         GROUP BY artist
         ORDER BY (artist IS NULL) ASC, n DESC, artist COLLATE NOCASE ASC",
    )?;
    let rows = stmt.query_map([], |r| Ok((r.get(0)?, r.get(1)?)))?;
    Ok(rows.collect::<rusqlite::Result<_>>()?)
}

// ---- User lists (favorites + named collections) --------------------------

/// Create a list (or return the existing one with that name — case-insensitive).
/// Select-first instead of `ON CONFLICT`: an upsert's conflict target must match
/// the index's collation, and ours is `name COLLATE NOCASE`, which makes
/// `ON CONFLICT(name)` brittle across SQLite versions. This is unambiguous.
pub fn create_list(conn: &Connection, name: &str) -> Result<i64> {
    let existing: Option<i64> = conn
        .query_row(
            "SELECT id FROM lists WHERE name = ?1 COLLATE NOCASE",
            params![name],
            |r| r.get(0),
        )
        .map(Some)
        .or_else(|e| match e {
            rusqlite::Error::QueryReturnedNoRows => Ok(None),
            e => Err(e),
        })?;
    if let Some(id) = existing {
        return Ok(id);
    }
    let now = chrono::Utc::now().format("%Y-%m-%dT%H:%M:%S+00:00").to_string();
    conn.execute(
        "INSERT INTO lists (name, created_at) VALUES (?1, ?2)",
        params![name, now],
    )?;
    Ok(conn.last_insert_rowid())
}

/// Delete a list (its memberships cascade away).
pub fn delete_list(conn: &Connection, list_id: i64) -> Result<()> {
    conn.execute("DELETE FROM lists WHERE id = ?1", params![list_id])?;
    Ok(())
}

/// Rename a list.
pub fn rename_list(conn: &Connection, list_id: i64, name: &str) -> Result<()> {
    conn.execute("UPDATE lists SET name = ?2 WHERE id = ?1", params![list_id, name])?;
    Ok(())
}

/// All lists as (id, name, item_count), ordered by name (case-insensitive).
pub fn all_lists(conn: &Connection) -> Result<Vec<(i64, String, i64)>> {
    let mut stmt = conn.prepare(
        "SELECT l.id, l.name, (SELECT COUNT(*) FROM list_items li WHERE li.list_id = l.id)
         FROM lists l ORDER BY l.name COLLATE NOCASE ASC",
    )?;
    let rows = stmt.query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))?;
    Ok(rows.collect::<rusqlite::Result<_>>()?)
}

/// Add a set (by its stable als_path) to a list. Idempotent.
pub fn add_to_list(conn: &Connection, list_id: i64, als_path: &str) -> Result<()> {
    let now = chrono::Utc::now().format("%Y-%m-%dT%H:%M:%S+00:00").to_string();
    conn.execute(
        "INSERT INTO list_items (list_id, als_path, added_at) VALUES (?1, ?2, ?3)
         ON CONFLICT(list_id, als_path) DO NOTHING",
        params![list_id, als_path, now],
    )?;
    Ok(())
}

/// Remove a set from a list.
pub fn remove_from_list(conn: &Connection, list_id: i64, als_path: &str) -> Result<()> {
    conn.execute(
        "DELETE FROM list_items WHERE list_id = ?1 AND als_path = ?2",
        params![list_id, als_path],
    )?;
    Ok(())
}

/// The list ids a given set (by als_path) currently belongs to.
pub fn lists_for_path(conn: &Connection, als_path: &str) -> Result<Vec<i64>> {
    let mut stmt = conn.prepare("SELECT list_id FROM list_items WHERE als_path = ?1")?;
    let rows = stmt.query_map(params![als_path], |r| r.get(0))?;
    Ok(rows.collect::<rusqlite::Result<_>>()?)
}

/// Manually set or clear the per-set artist override (`None` clears it, so the
/// set falls back to its project's derived artist).
pub fn set_set_artist_override(
    conn: &Connection,
    set_id: i64,
    artist: Option<&str>,
) -> Result<()> {
    conn.execute(
        "UPDATE sets SET artist_override = ?2 WHERE id = ?1",
        params![set_id, artist],
    )?;
    Ok(())
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
    /// Renderability 0..1 computed at queue time (None = not yet scored).
    pub score: Option<f64>,
    /// JSON renderability report (missing plugins, missing/evicted samples).
    pub fidelity: Option<String>,
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

/// Queue many sets at once (bulk export). Each set is (re)queued as pending,
/// EXCEPT sets whose job is currently `processing` — an active render is
/// never clobbered. Returns how many jobs were actually (re)queued.
pub fn add_export_jobs_bulk(conn: &Connection, set_ids: &[i64]) -> Result<usize> {
    let now = chrono::Utc::now().format("%Y-%m-%dT%H:%M:%S+00:00").to_string();
    let mut stmt = conn.prepare(
        "INSERT INTO export_jobs (set_id, status, created_at)
         VALUES (?1, 'pending', ?2)
         ON CONFLICT(set_id) DO UPDATE SET
            status = 'pending',
            created_at = ?2,
            started_at = NULL,
            completed_at = NULL,
            error = NULL
         WHERE export_jobs.status != 'processing'",
    )?;
    let mut queued = 0usize;
    for set_id in set_ids {
        queued += stmt.execute(params![set_id, now])?;
    }
    Ok(queued)
}

pub fn get_pending_export_job(conn: &Connection) -> Result<Option<(i64, i64, String)>> {
    // Easy-first triage (M4a): highest renderability score first, unscored
    // jobs last, FIFO within ties.
    let mut stmt = conn.prepare(
        "SELECT j.id, j.set_id, s.als_path
         FROM export_jobs j
         JOIN sets s ON s.id = j.set_id
         WHERE j.status = 'pending'
         ORDER BY (j.score IS NULL) ASC, j.score DESC, j.id ASC LIMIT 1",
    )?;
    let row = stmt.query_row([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))
        .map(Some)
        .or_else(|e| match e {
            rusqlite::Error::QueryReturnedNoRows => Ok(None),
            e => Err(e),
        })?;
    Ok(row)
}

/// Store the triage result on a job (M4a).
pub fn set_export_job_triage(
    conn: &Connection,
    job_id: i64,
    score: f64,
    fidelity_json: &str,
) -> Result<()> {
    conn.execute(
        "UPDATE export_jobs SET score = ?1, fidelity = ?2 WHERE id = ?3",
        params![score, fidelity_json, job_id],
    )?;
    Ok(())
}

/// Triage inputs for one set: non-native devices (kind, name, manufacturer)
/// and all referenced sample paths.
pub fn set_render_inputs(
    conn: &Connection,
    set_id: i64,
) -> Result<(Vec<(String, Option<String>, Option<String>)>, Vec<(String, bool)>)> {
    let mut stmt = conn.prepare(
        "SELECT kind, name, manufacturer FROM devices
         WHERE set_id = ?1 AND kind != 'native'",
    )?;
    let plugins = stmt
        .query_map(params![set_id], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    let mut stmt = conn.prepare("SELECT path, in_project FROM samples WHERE set_id = ?1")?;
    let samples = stmt
        .query_map(params![set_id], |r| Ok((r.get(0)?, r.get::<_, i32>(1)? != 0)))?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    Ok((plugins, samples))
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
        "SELECT j.id, j.set_id, s.als_path, p.name, j.status, j.error, j.created_at, j.started_at, j.completed_at, j.score, j.fidelity
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
            score: r.get(9)?,
            fidelity: r.get(10)?,
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

/// Empty the queue. A job currently `processing` is kept — its Live render
/// is mid-flight and the worker must be able to record the outcome.
pub fn clear_all_export_jobs(conn: &Connection) -> Result<usize> {
    Ok(conn.execute(
        "DELETE FROM export_jobs WHERE status != 'processing'",
        [],
    )?)
}

pub fn retry_failed_export_jobs(conn: &Connection) -> Result<()> {
    let now = chrono::Utc::now().format("%Y-%m-%dT%H:%M:%S+00:00").to_string();
    conn.execute(
        "UPDATE export_jobs SET status = 'pending', error = NULL, started_at = NULL, completed_at = NULL, created_at = ?1 WHERE status = 'failed'",
        params![now],
    )?;
    Ok(())
}

pub fn remove_export_job(conn: &Connection, job_id: i64) -> Result<()> {
    conn.execute("DELETE FROM export_jobs WHERE id = ?1", params![job_id])?;
    Ok(())
}

/// Wipe scores on pending jobs so they get re-scored (e.g. after the plugin
/// inventory upgrades from quick to full).
pub fn clear_pending_job_scores(conn: &Connection) -> Result<()> {
    conn.execute(
        "UPDATE export_jobs SET score = NULL, fidelity = NULL WHERE status = 'pending'",
        [],
    )?;
    Ok(())
}

/// Wipe stale score/fidelity from finished jobs (they may have been stamped
/// by older, buggier matching logic).
pub fn clear_finished_job_fidelity(conn: &Connection) -> Result<()> {
    conn.execute(
        "UPDATE export_jobs SET score = NULL, fidelity = NULL
         WHERE status IN ('completed', 'failed')",
        [],
    )?;
    Ok(())
}

/// Sets that have a worker-generated preview (for fidelity restamping).
pub fn worker_preview_set_ids(conn: &Connection) -> Result<Vec<i64>> {
    let mut stmt = conn.prepare(
        "SELECT DISTINCT set_id FROM previews WHERE source = 'worker' AND set_id IS NOT NULL",
    )?;
    let rows = stmt.query_map([], |r| r.get(0))?;
    Ok(rows.collect::<rusqlite::Result<_>>()?)
}

/// Replace the fidelity stamp on a set's worker preview(s).
pub fn update_worker_preview_fidelity(
    conn: &Connection,
    set_id: i64,
    fidelity_json: Option<&str>,
) -> Result<()> {
    conn.execute(
        "UPDATE previews SET fidelity = ?2 WHERE set_id = ?1 AND source = 'worker'",
        params![set_id, fidelity_json],
    )?;
    Ok(())
}

/// Pending jobs that haven't been triage-scored yet.
pub fn unscored_pending_jobs(conn: &Connection) -> Result<i64> {
    Ok(conn.query_row(
        "SELECT COUNT(*) FROM export_jobs WHERE status = 'pending' AND score IS NULL",
        [],
        |r| r.get(0),
    )?)
}

pub fn reset_stale_export_jobs(conn: &Connection) -> Result<()> {
    conn.execute(
        "UPDATE export_jobs SET status = 'failed', error = 'App closed or crashed during rendering' WHERE status = 'processing'",
        [],
    )?;
    Ok(())
}

pub fn add_watch_folder(conn: &Connection, path: &str) -> Result<()> {
    conn.execute(
        "INSERT INTO watch_folders (path) VALUES (?1) ON CONFLICT(path) DO NOTHING",
        params![path],
    )?;
    Ok(())
}

pub fn remove_watch_folder(conn: &Connection, id: i64) -> Result<()> {
    conn.execute("DELETE FROM watch_folders WHERE id = ?1", params![id])?;
    Ok(())
}

pub fn list_watch_folders(conn: &Connection) -> Result<Vec<(i64, String)>> {
    let mut stmt = conn.prepare("SELECT id, path FROM watch_folders ORDER BY path")?;
    let rows = stmt.query_map([], |r| Ok((r.get(0)?, r.get(1)?)))?;
    Ok(rows.collect::<rusqlite::Result<_>>()?)
}

pub fn add_ignored_match(conn: &Connection, set_id: i64, audio_path: &str) -> Result<()> {
    conn.execute(
        "INSERT INTO ignored_matches (set_id, audio_path) VALUES (?1, ?2) ON CONFLICT DO NOTHING",
        params![set_id, audio_path],
    )?;
    Ok(())
}

pub fn is_match_ignored(conn: &Connection, set_id: i64, audio_path: &str) -> Result<bool> {
    let count: i64 = conn.query_row(
        "SELECT COUNT(*) FROM ignored_matches WHERE set_id = ?1 AND audio_path = ?2 COLLATE NOCASE",
        params![set_id, audio_path],
        |r| r.get(0),
    )?;
    Ok(count > 0)
}


#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_remove_and_prune_previews() -> Result<()> {
        let conn = Connection::open_in_memory()?;
        conn.execute_batch(SCHEMA)?;
        conn.execute_batch(SCHEMA_V2)?;
        conn.execute_batch(SCHEMA_V3)?;
        conn.execute_batch(SCHEMA_V4)?;
        conn.execute_batch(SCHEMA_V5)?;
        conn.execute_batch(SCHEMA_V6)?;
        conn.execute_batch(SCHEMA_V7)?;
        conn.execute_batch(SCHEMA_V8)?;
        conn.execute_batch(SCHEMA_V9)?;

        // Insert project
        let project_id = upsert_project(&conn, "/path/to/project", "My Project", None, "2026-06-11T12:00:00+00:00")?;

        // Insert set
        conn.execute(
            "INSERT INTO sets (id, project_id, als_path, file_size, mtime, content_hash, warnings)
             VALUES (1, ?1, '/path/to/project/set.als', 100, '2026-06-11T12:00:00+00:00', 'hash', '[]')",
            params![project_id],
        )?;

        // Insert preview (which doesn't exist on disk)
        let row = PreviewRow {
            set_id: Some(1),
            project_id: Some(project_id),
            audio_path: "/path/to/nonexistent/preview.wav".into(),
            source: "manual".into(),
            confidence: 1.0,
            mtime: "2026-06-11T12:00:00+00:00".into(),
            size: 1000,
            duration: Some(10.0),
            peaks_json: Some("[]".into()),
            fidelity_json: None,
        };
        upsert_preview(&conn, &row)?;

        // Query primary preview -> should exist in db
        let p = primary_preview(&conn, 1)?;
        assert!(p.is_some());
        assert_eq!(p.unwrap().0, "/path/to/nonexistent/preview.wav");

        // Now prune stale previews (file doesn't exist on disk)
        let pruned = prune_stale_previews(&conn)?;
        assert_eq!(pruned.len(), 1);
        assert_eq!(pruned[0].1, "/path/to/nonexistent/preview.wav");

        // Primary preview should now be None
        let p = primary_preview(&conn, 1)?;
        assert!(p.is_none());

        Ok(())
    }

    fn fresh_db() -> Result<Connection> {
        let conn = Connection::open_in_memory()?;
        conn.execute_batch("PRAGMA foreign_keys=ON;")?;
        conn.execute_batch(SCHEMA)?;
        conn.execute_batch(SCHEMA_V2)?;
        conn.execute_batch(SCHEMA_V3)?;
        conn.execute_batch(SCHEMA_V4)?;
        conn.execute_batch(SCHEMA_V5)?;
        conn.execute_batch(SCHEMA_V6)?;
        conn.execute_batch(SCHEMA_V7)?;
        conn.execute_batch(SCHEMA_V8)?;
        conn.execute_batch(SCHEMA_V9)?;
        Ok(conn)
    }

    fn opts(artist: Option<&str>) -> SearchOpts {
        SearchOpts {
            text: None,
            min_bpm: None,
            max_bpm: None,
            plugin: None,
            artist: artist.map(|s| s.to_string()),
            list_id: None,
            sort_by: None,
            date_modified: None,
            date_scanned: None,
            has_preview: None,
        }
    }

    #[test]
    fn test_artist_column_search_and_coalesce() -> Result<()> {
        let conn = fresh_db()?;
        let now = "2026-06-13T00:00:00+00:00";

        // Project tagged with an artist, plus one with no artist.
        let pid_a = upsert_project(&conn, "/lib/burial/untrue", "untrue", Some("Burial"), now)?;
        let _pid_b = upsert_project(&conn, "/lib/2024/jan/sketch", "sketch", None, now)?;

        // A later broad scan that can't infer the artist must NOT wipe it.
        let pid_a2 = upsert_project(&conn, "/lib/burial/untrue", "untrue", None, now)?;
        assert_eq!(pid_a, pid_a2);
        let stored: Option<String> = conn.query_row(
            "SELECT artist FROM projects WHERE id = ?1", params![pid_a], |r| r.get(0))?;
        assert_eq!(stored.as_deref(), Some("Burial"));

        // Each project needs a set to appear in search results.
        for (i, pid) in [pid_a, _pid_b].iter().enumerate() {
            conn.execute(
                "INSERT INTO sets (project_id, als_path, file_size, mtime, content_hash, warnings)
                 VALUES (?1, ?2, 100, ?3, 'h', '[]')",
                params![pid, format!("/lib/set{i}.als"), now],
            )?;
        }

        // Filter by artist (substring, case-insensitive).
        let hits = search(&conn, &opts(Some("buri")))?;
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].artist.as_deref(), Some("Burial"));

        // No filter -> both sets; the artist rides along on the hit.
        let all = search(&conn, &opts(None))?;
        assert_eq!(all.len(), 2);

        // list_artists: known artist first, Unknown bucket last.
        let artists = list_artists(&conn)?;
        assert_eq!(artists.len(), 2);
        assert_eq!(artists[0], (Some("Burial".to_string()), 1));
        assert_eq!(artists[1], (None, 1));
        Ok(())
    }

    #[test]
    fn test_fts_query_sanitization() {
        // Punctuation that FTS5 treats as operators is neutralised; tokens get
        // quoted + prefixed.
        assert_eq!(fts_query("131.10").as_deref(), Some("\"131\"* \"10\"*"));
        assert_eq!(fts_query("tisa - taco").as_deref(), Some("\"tisa\"* \"taco\"*"));
        assert_eq!(fts_query("nasty (prod)").as_deref(), Some("\"nasty\"* \"prod\"*"));
        assert_eq!(fts_query("foo").as_deref(), Some("\"foo\"*"));
        // No usable tokens -> None (caller skips the text filter entirely).
        assert_eq!(fts_query("..."), None);
        assert_eq!(fts_query("   "), None);
        assert_eq!(fts_query(""), None);
    }

    #[test]
    fn test_search_with_period_in_query() -> Result<()> {
        let conn = fresh_db()?;
        let now = "2026-06-13T00:00:00+00:00";
        let pid = upsert_project(&conn, "/lib/2019/be", "be 131.10 bpm", None, now)?;
        conn.execute(
            "INSERT INTO sets (id, project_id, als_path, file_size, mtime, content_hash, warnings)
             VALUES (1, ?1, '/lib/2019/be/be.als', 100, ?2, 'h', '[]')",
            params![pid, now],
        )?;
        conn.execute(
            "INSERT INTO search (set_id, project_name, set_name, track_names, device_names, sample_names)
             VALUES (1, 'be 131.10 bpm', 'be 131.10 bpm', '', '', '')",
            [],
        )?;

        // Previously raised `fts5: syntax error near "."`.
        let mut o = opts(None);
        o.text = Some("131.10".to_string());
        let hits = search(&conn, &o)?;
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].project, "be 131.10 bpm");

        // A bare period (or any all-punctuation input) must not error and must
        // degrade to "no text filter" rather than matching nothing-by-syntax.
        o.text = Some(".".to_string());
        assert!(search(&conn, &o).is_ok());
        Ok(())
    }

    #[test]
    fn test_per_set_artist_override() -> Result<()> {
        let conn = fresh_db()?;
        let now = "2026-06-13T00:00:00+00:00";
        // One project (derived artist "Burial") with two sets.
        let pid = upsert_project(&conn, "/lib/burial/untrue", "untrue", Some("Burial"), now)?;
        for i in 0..2 {
            conn.execute(
                "INSERT INTO sets (id, project_id, als_path, file_size, mtime, content_hash, warnings)
                 VALUES (?1, ?2, ?3, 100, ?4, 'h', '[]')",
                params![i + 1, pid, format!("/lib/burial/untrue/s{i}.als"), now],
            )?;
        }
        // Both sets inherit the project artist.
        assert_eq!(search(&conn, &opts(Some("burial")))?.len(), 2);

        // Override just set 2 -> "Four Tet" (a collab). Effective artist flips.
        set_set_artist_override(&conn, 2, Some("Four Tet"))?;
        let burial = search(&conn, &opts(Some("burial")))?;
        assert_eq!(burial.len(), 1); // only set 1 still reads Burial
        assert_eq!(burial[0].set_id, 1);
        let ft = search(&conn, &opts(Some("four tet")))?;
        assert_eq!(ft.len(), 1);
        assert_eq!(ft[0].set_id, 2);
        assert_eq!(ft[0].artist.as_deref(), Some("Four Tet"));

        // list_artists counts SETS by effective artist.
        let artists = list_artists(&conn)?;
        assert!(artists.contains(&(Some("Burial".to_string()), 1)));
        assert!(artists.contains(&(Some("Four Tet".to_string()), 1)));

        // Clearing the override falls back to the project artist.
        set_set_artist_override(&conn, 2, None)?;
        assert_eq!(search(&conn, &opts(Some("burial")))?.len(), 2);
        Ok(())
    }

    #[test]
    fn test_lists_membership_and_filter() -> Result<()> {
        let conn = fresh_db()?;
        let now = "2026-06-13T00:00:00+00:00";
        let pid = upsert_project(&conn, "/lib/p", "p", None, now)?;
        // Two sets.
        for i in 0..2 {
            conn.execute(
                "INSERT INTO sets (id, project_id, als_path, file_size, mtime, content_hash, warnings)
                 VALUES (?1, ?2, ?3, 100, ?4, 'h', '[]')",
                params![i + 1, pid, format!("/lib/p/s{i}.als"), now],
            )?;
        }
        let p0 = "/lib/p/s0.als";
        let p1 = "/lib/p/s1.als";

        // Create lists (get-or-create is case-insensitive idempotent).
        let favs = create_list(&conn, "Favorites")?;
        assert_eq!(create_list(&conn, "favorites")?, favs);
        let mix = create_list(&conn, "to mix")?;

        // Multi-list membership: s0 in both, s1 in none.
        add_to_list(&conn, favs, p0)?;
        add_to_list(&conn, mix, p0)?;
        add_to_list(&conn, favs, p0)?; // idempotent

        let mut s0_lists = lists_for_path(&conn, p0)?;
        s0_lists.sort();
        let mut want = vec![favs, mix];
        want.sort();
        assert_eq!(s0_lists, want);
        assert!(lists_for_path(&conn, p1)?.is_empty());

        // in_list flag in search.
        let all = search(&conn, &opts(None))?;
        let s0 = all.iter().find(|h| h.set_id == 1).unwrap();
        let s1 = all.iter().find(|h| h.set_id == 2).unwrap();
        assert!(s0.in_list);
        assert!(!s1.in_list);

        // Filter by list.
        let mut o = opts(None);
        o.list_id = Some(mix);
        let in_mix = search(&conn, &o)?;
        assert_eq!(in_mix.len(), 1);
        assert_eq!(in_mix[0].set_id, 1);

        // Counts + remove + cascade on delete.
        let lists = all_lists(&conn)?;
        assert_eq!(lists.iter().find(|l| l.0 == favs).unwrap().2, 1);
        remove_from_list(&conn, mix, p0)?;
        assert_eq!(lists_for_path(&conn, p0)?, vec![favs]);
        delete_list(&conn, favs)?;
        assert!(lists_for_path(&conn, p0)?.is_empty());
        Ok(())
    }

    #[test]
    fn test_watch_folders_and_ignored_matches() -> Result<()> {
        let conn = Connection::open_in_memory()?;
        conn.execute_batch(SCHEMA)?;
        conn.execute_batch(SCHEMA_V2)?;
        conn.execute_batch(SCHEMA_V3)?;
        conn.execute_batch(SCHEMA_V4)?;
        conn.execute_batch(SCHEMA_V5)?;
        conn.execute_batch(SCHEMA_V6)?;
        conn.execute_batch(SCHEMA_V7)?;
        conn.execute_batch(SCHEMA_V8)?;
        conn.execute_batch(SCHEMA_V9)?;
        conn.execute_batch(SCHEMA_V4)?;

        // Insert project and set to satisfy foreign key constraint on ignored_matches
        let project_id = upsert_project(&conn, "/path/to/project", "My Project", None, "2026-06-11T12:00:00+00:00")?;
        conn.execute(
            "INSERT INTO sets (id, project_id, als_path, file_size, mtime, content_hash, warnings)
             VALUES (1, ?1, '/path/to/project/set.als', 100, '2026-06-11T12:00:00+00:00', 'hash', '[]')",
            params![project_id],
        )?;

        // Test watch folders
        add_watch_folder(&conn, "/path/to/watch1")?;
        add_watch_folder(&conn, "/path/to/watch2")?;
        add_watch_folder(&conn, "/path/to/watch1")?; // duplicate check

        let list = list_watch_folders(&conn)?;
        assert_eq!(list.len(), 2);
        assert_eq!(list[0].1, "/path/to/watch1");
        assert_eq!(list[1].1, "/path/to/watch2");

        remove_watch_folder(&conn, list[0].0)?;
        let list = list_watch_folders(&conn)?;
        assert_eq!(list.len(), 1);
        assert_eq!(list[0].1, "/path/to/watch2");

        // Test ignored matches
        assert!(!is_match_ignored(&conn, 1, "/path/to/audio.wav")?);
        add_ignored_match(&conn, 1, "/path/to/audio.wav")?;
        assert!(is_match_ignored(&conn, 1, "/path/to/audio.wav")?);

        Ok(())
    }
}

