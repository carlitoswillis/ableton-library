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

pub fn run() {
    tauri::Builder::default()
        .invoke_handler(tauri::generate_handler![search, inspect, stats])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
