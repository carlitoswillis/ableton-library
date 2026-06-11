//! ableton-scan: scan a folder of Ableton projects into JSON snapshots.
//!
//! Usage: ableton-scan <library-root> [--pretty]
//! JSON (array of ProjectSnapshot) on stdout, human summary on stderr.
//! Output shape matches tools/reference_extract.py (the test oracle):
//!   diff <(ableton-scan lib) <(python3 tools/reference_extract.py lib)

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use anyhow::Result;
use chrono::{DateTime, Utc};
use clap::Parser;
use serde::Serialize;
use walkdir::WalkDir;

use als_core::{parse_set, DeviceKind, SetSnapshot};

#[derive(Parser)]
#[command(name = "ableton-scan", about = "Index Ableton Live projects from the filesystem")]
struct Args {
    /// Root folder containing Ableton projects
    root: PathBuf,
    /// Pretty-print JSON output
    #[arg(long)]
    pretty: bool,
}

#[derive(Serialize)]
struct ProjectSnapshot {
    folder_path: String,
    name: String,
    sets: Vec<SetSnapshot>,
    backups: Vec<BackupEntry>,
}

/// Lineage-only record of a Backup/*.als (not parsed; see PROJECT_STATE.md).
#[derive(Serialize)]
struct BackupEntry {
    file: String,
    size: u64,
    mtime: String,
}

fn iso_mtime(path: &Path) -> Result<String> {
    let t: DateTime<Utc> = std::fs::metadata(path)?.modified()?.into();
    Ok(t.format("%Y-%m-%dT%H:%M:%S+00:00").to_string())
}

fn backups(project_dir: &Path) -> Result<Vec<BackupEntry>> {
    let dir = project_dir.join("Backup");
    let mut out = Vec::new();
    if dir.is_dir() {
        let mut names: Vec<PathBuf> = std::fs::read_dir(&dir)?
            .filter_map(|e| e.ok().map(|e| e.path()))
            .filter(|p| p.extension().map_or(false, |x| x == "als"))
            .collect();
        names.sort();
        for p in names {
            out.push(BackupEntry {
                file: p.file_name().unwrap().to_string_lossy().into_owned(),
                size: std::fs::metadata(&p)?.len(),
                mtime: iso_mtime(&p)?,
            });
        }
    }
    Ok(out)
}

fn main() -> Result<()> {
    let args = Args::parse();

    // A "project" is any directory directly containing .als files.
    // Backup/ directories hold timestamped lineage, indexed but not parsed.
    let mut projects: BTreeMap<PathBuf, Vec<PathBuf>> = BTreeMap::new();
    for entry in WalkDir::new(&args.root)
        .into_iter()
        .filter_entry(|e| e.file_name() != "Backup")
        .filter_map(|e| e.ok())
    {
        let p = entry.path();
        if p.extension().map_or(false, |x| x == "als") {
            projects
                .entry(p.parent().unwrap().to_path_buf())
                .or_default()
                .push(p.to_path_buf());
        }
    }

    let mut library = Vec::new();
    for (dir, mut als_files) in projects {
        als_files.sort();
        let name = dir
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default();
        let mut sets = Vec::new();
        for als in &als_files {
            match parse_set(als, &dir) {
                Ok(snap) => {
                    summarize(&snap);
                    sets.push(snap);
                }
                // Lenient at the catalog level too: one corrupt file must
                // not abort the scan.
                Err(e) => eprintln!("  ERROR {}: {e}", als.display()),
            }
        }
        let backups = backups(&dir)?;
        eprintln!("{name}: {} set(s), {} backup(s)", sets.len(), backups.len());
        library.push(ProjectSnapshot {
            folder_path: std::path::absolute(&dir)?.to_string_lossy().into_owned(),
            name,
            sets,
            backups,
        });
    }

    let json = if args.pretty {
        serde_json::to_string_pretty(&library)?
    } else {
        serde_json::to_string(&library)?
    };
    println!("{json}");
    Ok(())
}

fn summarize(s: &SetSnapshot) {
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
    let plugins = s.devices.iter().filter(|d| d.kind != DeviceKind::Native).count();
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
