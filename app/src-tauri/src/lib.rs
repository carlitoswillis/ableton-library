//! Tauri backend: thin command layer over the shared `indexer` crate.
//! All query logic lives in indexer — the CLI and this app are equals.

use std::path::PathBuf;

use serde_json::Value;

fn db_path() -> Result<PathBuf, String> {
    Ok(dirs::data_dir()
        .ok_or("no app data dir on this platform")?
        .join("ableton-library")
        .join("library.db"))
}

fn none_if_blank(s: Option<String>) -> Option<String> {
    s.filter(|v| !v.trim().is_empty())
}

#[tauri::command(rename_all = "snake_case")]
fn search(
    text: Option<String>,
    min_bpm: Option<f64>,
    max_bpm: Option<f64>,
    plugin: Option<String>,
) -> Result<Vec<indexer::SearchHit>, String> {
    let conn = indexer::open(&db_path()?).map_err(|e| e.to_string())?;
    indexer::search(
        &conn,
        &indexer::SearchOpts {
            text: none_if_blank(text),
            min_bpm,
            max_bpm,
            plugin: none_if_blank(plugin),
        },
    )
    .map_err(|e| e.to_string())
}

#[tauri::command(rename_all = "snake_case")]
fn inspect(set_id: i64) -> Result<Value, String> {
    let conn = indexer::open(&db_path()?).map_err(|e| e.to_string())?;
    indexer::set_detail(&conn, set_id).map_err(|e| e.to_string())
}

#[tauri::command(rename_all = "snake_case")]
fn stats() -> Result<indexer::Stats, String> {
    let conn = indexer::open(&db_path()?).map_err(|e| e.to_string())?;
    indexer::stats(&conn).map_err(|e| e.to_string())
}

/// Open the set in Ableton Live (default .als handler), or reveal it in Finder.
/// Only ever opens paths stored in the catalog — never arbitrary input.
#[tauri::command(rename_all = "snake_case")]
fn open_set(set_id: i64, reveal: bool) -> Result<(), String> {
    let conn = indexer::open(&db_path()?).map_err(|e| e.to_string())?;
    let path = indexer::set_path(&conn, set_id).map_err(|e| e.to_string())?;
    if !std::path::Path::new(&path).exists() {
        return Err(format!("File not found on disk: {path}"));
    }
    #[cfg(target_os = "macos")]
    {
        let mut cmd = std::process::Command::new("open");
        if reveal {
            cmd.arg("-R");
        }
        cmd.arg(&path).spawn().map_err(|e| e.to_string())?;
        Ok(())
    }
    #[cfg(not(target_os = "macos"))]
    {
        let _ = reveal;
        Err("open is only implemented on macOS so far".into())
    }
}

pub fn run() {
    tauri::Builder::default()
        .invoke_handler(tauri::generate_handler![search, inspect, stats, open_set])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
