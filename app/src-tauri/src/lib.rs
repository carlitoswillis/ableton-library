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
async fn search(
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
async fn inspect(set_id: i64) -> Result<Value, String> {
    let conn = indexer::open(&db_path()?).map_err(|e| e.to_string())?;
    indexer::set_detail(&conn, set_id).map_err(|e| e.to_string())
}

#[tauri::command(rename_all = "snake_case")]
async fn stats() -> Result<indexer::Stats, String> {
    let conn = indexer::open(&db_path()?).map_err(|e| e.to_string())?;
    indexer::stats(&conn).map_err(|e| e.to_string())
}

#[derive(serde::Serialize)]
struct PreviewInfo {
    audio_path: String,
    duration: Option<f64>,
    peaks: serde_json::Value,
    confidence: f64,
    source: String,
}

/// Primary preview (audio path + waveform peaks) for a set, if any.
#[tauri::command(rename_all = "snake_case")]
async fn preview(set_id: i64) -> Result<Option<PreviewInfo>, String> {
    let conn = indexer::open(&db_path()?).map_err(|e| e.to_string())?;
    let row = indexer::primary_preview(&conn, set_id).map_err(|e| e.to_string())?;
    Ok(row.map(|(audio_path, duration, peaks_json, confidence, source)| PreviewInfo {
        audio_path,
        duration,
        peaks: peaks_json
            .and_then(|j| serde_json::from_str(&j).ok())
            .unwrap_or(serde_json::Value::Array(vec![])),
        confidence,
        source,
    }))
}

use tauri::Emitter;

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use tauri::State;

#[derive(Default)]
struct ScanState {
    cancel: Arc<AtomicBool>,
}

#[tauri::command(rename_all = "snake_case")]
async fn cancel_scan(state: State<'_, ScanState>) -> Result<(), String> {
    state.cancel.store(true, Ordering::Relaxed);
    Ok(())
}

/// Index a folder of Ableton projects (incremental; harvests in-folder
/// renders as previews). Same engine as `ableton-scan scan`.
///
/// MUST be async + spawn_blocking: synchronous Tauri commands run on the
/// MAIN thread and freeze the whole window (the beach ball incident).
#[tauri::command(rename_all = "snake_case")]
async fn scan_folder(
    app: tauri::AppHandle,
    state: State<'_, ScanState>,
    root: String,
) -> Result<ops::ScanSummary, String> {
    state.cancel.store(false, Ordering::Relaxed);
    let cancel_flag = state.cancel.clone();
    tauri::async_runtime::spawn_blocking(move || {
        let conn = indexer::open(&db_path()?).map_err(|e| e.to_string())?;
        let app_clone = app.clone();
        let mut log = move |line: String| {
            let _ = app_clone.emit("scan-progress", line);
        };
        ops::scan_library(
            &conn,
            std::path::Path::new(&root),
            false,
            true,
            Some(&cancel_flag),
            &mut log,
        )
        .map_err(|e| e.to_string())
    })
    .await
    .map_err(|e| e.to_string())?
}

/// Open the set in Ableton Live (default .als handler), or reveal it in Finder.
/// Only ever opens paths stored in the catalog — never arbitrary input.
#[tauri::command(rename_all = "snake_case")]
async fn open_set(set_id: i64, reveal: bool) -> Result<(), String> {
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
        .manage(ScanState::default())
        .plugin(tauri_plugin_dialog::init())
        .invoke_handler(tauri::generate_handler![
            search, inspect, stats, open_set, preview, scan_folder, cancel_scan
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
