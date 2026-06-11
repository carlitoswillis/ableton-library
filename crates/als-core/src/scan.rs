//! Library discovery: find project folders and their .als files.
//!
//! Lives in als-core (not the CLI) so the indexer and the future Tauri app
//! share one definition of "what is a project".

use std::path::{Path, PathBuf};

use chrono::{DateTime, Utc};
use walkdir::WalkDir;

use crate::model::BackupEntry;

/// A project folder found on disk. Sets are NOT parsed yet — callers decide
/// what needs parsing (e.g. the indexer skips unchanged files).
#[derive(Debug, Clone)]
pub struct DiscoveredProject {
    pub dir: PathBuf,
    pub name: String,
    /// Top-level .als files (sorted). Backup/ contents are never included.
    pub als_files: Vec<PathBuf>,
    pub backups: Vec<BackupEntry>,
}

/// File mtime as ISO-8601 UTC — the single canonical format used in
/// snapshots, backup lineage, and the index freshness check.
pub fn iso_mtime(path: &Path) -> std::io::Result<String> {
    let t: DateTime<Utc> = std::fs::metadata(path)?.modified()?.into();
    Ok(t.format("%Y-%m-%dT%H:%M:%S+00:00").to_string())
}

fn backups(project_dir: &Path) -> std::io::Result<Vec<BackupEntry>> {
    let dir = project_dir.join("Backup");
    let mut out = Vec::new();
    if dir.is_dir() {
        let mut paths: Vec<PathBuf> = std::fs::read_dir(&dir)?
            .filter_map(|e| e.ok().map(|e| e.path()))
            .filter(|p| p.extension().map_or(false, |x| x == "als"))
            .collect();
        paths.sort();
        for p in paths {
            out.push(BackupEntry {
                file: p.file_name().unwrap().to_string_lossy().into_owned(),
                size: std::fs::metadata(&p)?.len(),
                mtime: iso_mtime(&p)?,
            });
        }
    }
    Ok(out)
}

/// Recursively find projects: any directory directly containing .als files.
/// Recurses to any depth (years/months/artists nesting is fine).
pub fn discover(root: &Path) -> std::io::Result<Vec<DiscoveredProject>> {
    use std::collections::BTreeMap;
    let mut by_dir: BTreeMap<PathBuf, Vec<PathBuf>> = BTreeMap::new();
    for entry in WalkDir::new(root)
        .into_iter()
        .filter_entry(|e| e.file_name() != "Backup")
        .filter_map(|e| e.ok())
    {
        let p = entry.path();
        if p.extension().map_or(false, |x| x == "als") {
            by_dir
                .entry(p.parent().unwrap().to_path_buf())
                .or_default()
                .push(p.to_path_buf());
        }
    }
    let mut out = Vec::new();
    for (dir, mut als_files) in by_dir {
        als_files.sort();
        out.push(DiscoveredProject {
            name: dir
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_default(),
            backups: backups(&dir)?,
            dir,
            als_files,
        });
    }
    Ok(out)
}
