//! Tauri backend: thin command layer over the shared `indexer` crate.
//! All query logic lives in indexer — the CLI and this app are equals.

use std::path::PathBuf;
use tauri::Manager;

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
    sort_by: Option<String>,
    date_modified: Option<String>,
    date_scanned: Option<String>,
    has_preview: Option<String>,
) -> Result<Vec<indexer::SearchHit>, String> {
    let conn = indexer::open(&db_path()?).map_err(|e| e.to_string())?;
    indexer::search(
        &conn,
        &indexer::SearchOpts {
            text: none_if_blank(text),
            min_bpm,
            max_bpm,
            plugin: none_if_blank(plugin),
            sort_by: none_if_blank(sort_by),
            date_modified: none_if_blank(date_modified),
            date_scanned: none_if_blank(date_scanned),
            has_preview: none_if_blank(has_preview),
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
    if let Some((audio_path, duration, peaks_json, confidence, source)) = row {
        if !std::path::Path::new(&audio_path).exists() {
            let _ = indexer::remove_preview(&conn, set_id);
            return Err("Preview file was missing from disk and has been removed from the database.".to_string());
        }
        Ok(Some(PreviewInfo {
            audio_path,
            duration,
            peaks: peaks_json
                .and_then(|j| serde_json::from_str(&j).ok())
                .unwrap_or(serde_json::Value::Array(vec![])),
            confidence,
            source,
        }))
    } else {
        Ok(None)
    }
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

struct ExportState {
    active: Arc<AtomicBool>,
}

impl Default for ExportState {
    fn default() -> Self {
        Self {
            active: Arc::new(AtomicBool::new(false)),
        }
    }
}

#[tauri::command(rename_all = "snake_case")]
async fn add_to_export_queue(set_id: i64) -> Result<(), String> {
    let conn = indexer::open(&db_path()?).map_err(|e| e.to_string())?;
    indexer::add_export_job(&conn, set_id).map_err(|e| e.to_string())
}

/// Bulk export: queue renders for many sets at once (multi-select in the UI).
/// Returns how many were actually queued (active renders are skipped).
#[tauri::command(rename_all = "snake_case")]
async fn add_to_export_queue_bulk(set_ids: Vec<i64>) -> Result<usize, String> {
    let conn = indexer::open(&db_path()?).map_err(|e| e.to_string())?;
    indexer::add_export_jobs_bulk(&conn, &set_ids).map_err(|e| e.to_string())
}

#[tauri::command(rename_all = "snake_case")]
async fn get_export_queue() -> Result<Vec<indexer::ExportJobInfo>, String> {
    let conn = indexer::open(&db_path()?).map_err(|e| e.to_string())?;
    indexer::get_export_queue(&conn).map_err(|e| e.to_string())
}

#[tauri::command(rename_all = "snake_case")]
async fn remove_from_export_queue(job_id: i64) -> Result<(), String> {
    let conn = indexer::open(&db_path()?).map_err(|e| e.to_string())?;
    indexer::remove_export_job(&conn, job_id).map_err(|e| e.to_string())
}

#[tauri::command(rename_all = "snake_case")]
async fn clear_completed_jobs() -> Result<(), String> {
    let conn = indexer::open(&db_path()?).map_err(|e| e.to_string())?;
    indexer::clear_completed_export_jobs(&conn).map_err(|e| e.to_string())
}

#[tauri::command(rename_all = "snake_case")]
async fn retry_failed_jobs() -> Result<(), String> {
    let conn = indexer::open(&db_path()?).map_err(|e| e.to_string())?;
    indexer::retry_failed_export_jobs(&conn).map_err(|e| e.to_string())
}

#[tauri::command(rename_all = "snake_case")]
async fn toggle_export_queue(state: State<'_, ExportState>, active: bool) -> Result<(), String> {
    state.active.store(active, Ordering::Relaxed);
    Ok(())
}

#[tauri::command(rename_all = "snake_case")]
async fn get_export_queue_active(state: State<'_, ExportState>) -> Result<bool, String> {
    Ok(state.active.load(Ordering::Relaxed))
}

async fn export_worker_loop(app: tauri::AppHandle) {
    loop {
        tokio::time::sleep(std::time::Duration::from_secs(3)).await;

        let active = {
            if let Some(state) = app.try_state::<ExportState>() {
                state.active.load(Ordering::Relaxed)
            } else {
                false
            }
        };

        if !active {
            continue;
        }

        let db = match db_path() {
            Ok(p) => p,
            Err(_) => continue,
        };
        let conn = match indexer::open(&db) {
            Ok(c) => c,
            Err(_) => continue,
        };

        let job = match indexer::get_pending_export_job(&conn) {
            Ok(Some(j)) => j,
            _ => continue,
        };

        let (job_id, set_id, als_path) = job;
        let set_stem = std::path::Path::new(&als_path)
            .file_stem()
            .map(|x| x.to_string_lossy().into_owned())
            .unwrap_or_default();

        // 1. Mark as processing
        if let Err(_) = indexer::update_export_job_status(&conn, job_id, "processing", None) {
            continue;
        }
        let _ = app.emit("export-queue-updated", ());

        // 2. Perform the export (saving inside the existing project folder next to the .als file)
        let als_parent = std::path::Path::new(&als_path).parent().unwrap().to_path_buf();

        let mut script_path = std::env::current_dir()
            .unwrap_or_default()
            .join("tools")
            .join("export_set.py");

        if !script_path.exists() {
            if let Ok(exe_path) = std::env::current_exe() {
                if let Some(parent) = exe_path.parent() {
                    script_path = parent.join("tools").join("export_set.py");
                    if !script_path.exists() {
                        if let Some(grandparent) = parent.parent() {
                            if let Some(grandgrandparent) = grandparent.parent() {
                                script_path = grandgrandparent.join("tools").join("export_set.py");
                            }
                        }
                    }
                }
            }
        }

        let mut live_app_path = std::path::PathBuf::from("/Applications/Ableton Live 11 Suite.app");
        if !live_app_path.exists() {
            if let Ok(entries) = std::fs::read_dir("/Applications") {
                for entry in entries.flatten() {
                    let path = entry.path();
                    let name = path.file_name().unwrap_or_default().to_string_lossy();
                    if name.contains("Ableton Live") && name.ends_with(".app") {
                        live_app_path = path;
                        break;
                    }
                }
            }
        }
        let live_app = live_app_path.to_string_lossy().into_owned();
        let output_dir_str = als_parent.to_string_lossy().into_owned();

        // Run process
        let status = tauri::async_runtime::spawn_blocking(move || {
            std::process::Command::new("python3")
                .arg(&script_path)
                .arg("--set-path")
                .arg(&als_path)
                .arg("--output-dir")
                .arg(&output_dir_str)
                .arg("--live-app")
                .arg(live_app)
                .output()
        })
        .await;

        let mut error_msg = None;
        let mut success = false;

        match status {
            Ok(Ok(output)) if output.status.success() => {
                success = true;
            }
            Ok(Ok(output)) => {
                let stderr = String::from_utf8_lossy(&output.stderr).into_owned();
                error_msg = Some(format!("Script failed: {stderr}"));
            }
            Ok(Err(e)) => {
                error_msg = Some(format!("Failed to execute python3 script: {e}"));
            }
            Err(e) => {
                error_msg = Some(format!("Thread panic during command execution: {e}"));
            }
        }

        if success {
            let audio_file = als_parent.join(format!("{}.wav", set_stem));

            let db_path_clone = db.clone();
            let ingest_result = tauri::async_runtime::spawn_blocking(move || {
                let conn = indexer::open(&db_path_clone)?;
                let meta = std::fs::metadata(&audio_file)?;
                let now = chrono::Utc::now().format("%Y-%m-%dT%H:%M:%S+00:00").to_string();
                let pk = previews::peaks::extract(&audio_file)?;
                let row = indexer::PreviewRow {
                    set_id: Some(set_id),
                    project_id: Some(indexer::set_project_id(&conn, set_id)?),
                    audio_path: std::path::absolute(&audio_file)?.to_string_lossy().into_owned(),
                    source: "worker".into(),
                    confidence: 1.0,
                    mtime: now,
                    size: meta.len(),
                    duration: Some(pk.duration_secs),
                    peaks_json: Some(previews::peaks::to_json(&pk.peaks)),
                };
                indexer::upsert_preview(&conn, &row)?;
                indexer::update_export_job_status(&conn, job_id, "completed", None)?;
                Ok::<(), anyhow::Error>(())
            })
            .await;

            if let Err(e) = ingest_result {
                let err_str = e.to_string();
                let _ = indexer::update_export_job_status(&conn, job_id, "failed", Some(&err_str));
            }
        } else {
            let err_str = error_msg.unwrap_or_else(|| "Unknown export error".into());
            let _ = indexer::update_export_job_status(&conn, job_id, "failed", Some(&err_str));
        }

        let _ = app.emit("export-queue-updated", ());
    }
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

#[tauri::command(rename_all = "snake_case")]
async fn bulk_preview_scan(
    app: tauri::AppHandle,
    state: State<'_, ScanState>,
) -> Result<ops::ScanSummary, String> {
    state.cancel.store(false, Ordering::Relaxed);
    let cancel_flag = state.cancel.clone();
    tauri::async_runtime::spawn_blocking(move || {
        let conn = indexer::open(&db_path()?).map_err(|e| e.to_string())?;
        let app_clone = app.clone();
        let mut log = move |line: String| {
            let _ = app_clone.emit("scan-progress", line);
        };
        
        let mut s = ops::ScanSummary::default();

        // 1. Harvest project folders of sets without previews
        let mut stmt = conn.prepare(
            "SELECT DISTINCT p.id, p.name, p.folder_path 
             FROM projects p 
             JOIN sets s ON s.project_id = p.id 
             LEFT JOIN previews pr ON pr.set_id = s.id 
             WHERE pr.audio_path IS NULL"
        ).map_err(|e| e.to_string())?;
        
        let projects: Vec<(i64, String, String)> = stmt.query_map([], |row| {
            Ok((row.get(0)?, row.get(1)?, row.get(2)?))
        }).map_err(|e| e.to_string())?
        .filter_map(Result::ok)
        .collect();
        
        let known_samples = indexer::all_sample_paths(&conn).map_err(|e| e.to_string())?;
        
        for (pid, name, dir) in projects {
            if cancel_flag.load(Ordering::Relaxed) {
                return Err("scan cancelled by user".to_string());
            }
            let path = std::path::PathBuf::from(dir);
            match ops::harvest_folder_renders(&conn, &path, &name, pid, &known_samples, Some(&cancel_flag), &mut log) {
                Ok(n) => s.harvested += n,
                Err(e) => log(format!("preview harvest failed for {}: {}", path.display(), e)),
            }
        }
        
        // 2. Hunt watch folders
        let watch_folders = indexer::list_watch_folders(&conn).map_err(|e| e.to_string())?;
        if !watch_folders.is_empty() {
            let roots: Vec<std::path::PathBuf> = watch_folders.into_iter().map(|(_, p)| std::path::PathBuf::from(p)).collect();
            match ops::hunt_renders(&conn, &roots, 0.6, false, &mut log) {
                Ok(hs) => {
                    s.harvested += hs.matched;
                    s.errors += hs.errors;
                    s.unchanged += hs.unchanged;
                }
                Err(e) => log(format!("watch folder hunt failed: {}", e)),
            }
        }
        
        Ok(s)
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

#[tauri::command(rename_all = "snake_case")]
async fn remove_preview(set_id: i64) -> Result<bool, String> {
    let conn = indexer::open(&db_path()?).map_err(|e| e.to_string())?;
    let deleted = indexer::remove_preview(&conn, set_id).map_err(|e| e.to_string())?;
    Ok(deleted.is_some())
}

#[tauri::command(rename_all = "snake_case")]
async fn add_watch_folder(path: String) -> Result<(), String> {
    let conn = indexer::open(&db_path()?).map_err(|e| e.to_string())?;
    indexer::add_watch_folder(&conn, &path).map_err(|e| e.to_string())
}

#[tauri::command(rename_all = "snake_case")]
async fn remove_watch_folder(id: i64) -> Result<(), String> {
    let conn = indexer::open(&db_path()?).map_err(|e| e.to_string())?;
    indexer::remove_watch_folder(&conn, id).map_err(|e| e.to_string())
}

#[tauri::command(rename_all = "snake_case")]
async fn list_watch_folders() -> Result<Vec<(i64, String)>, String> {
    let conn = indexer::open(&db_path()?).map_err(|e| e.to_string())?;
    indexer::list_watch_folders(&conn).map_err(|e| e.to_string())
}

#[tauri::command(rename_all = "snake_case")]
async fn get_watch_suggestions() -> Result<Vec<ops::Suggestion>, String> {
    let conn = indexer::open(&db_path()?).map_err(|e| e.to_string())?;
    ops::get_watch_suggestions(&conn).map_err(|e| e.to_string())
}

#[tauri::command(rename_all = "snake_case")]
async fn ignore_watch_suggestion(set_id: i64, audio_path: String) -> Result<(), String> {
    let conn = indexer::open(&db_path()?).map_err(|e| e.to_string())?;
    indexer::add_ignored_match(&conn, set_id, &audio_path).map_err(|e| e.to_string())
}

/// Link one suggested bounce match. spawn_blocking: moves a file (possibly
/// iCloud) and decodes audio — never block the async runtime with that.
#[tauri::command(rename_all = "snake_case")]
async fn link_watch_suggestion(set_id: i64, audio_path: String) -> Result<(), String> {
    tauri::async_runtime::spawn_blocking(move || {
        let conn = indexer::open(&db_path()?).map_err(|e| e.to_string())?;
        let mut last_err: Option<String> = None;
        let mut log = |line: String| last_err = Some(line);
        let mut progress = |_done: usize| {};
        let linked = ops::link_suggestions(
            &conn,
            &[(set_id, audio_path)],
            None,
            &mut progress,
            &mut log,
        )
        .map_err(|e| e.to_string())?;
        if linked == 1 {
            Ok(())
        } else {
            Err(last_err.unwrap_or_else(|| "link failed".into()))
        }
    })
    .await
    .map_err(|e| e.to_string())?
}

/// Bulk-link suggested bounce matches. Heavy (file moves + audio decodes for
/// potentially hundreds of files) → spawn_blocking, parallel decode in ops,
/// and "link-progress" events `(done, total)` so the UI can show progress.
#[tauri::command(rename_all = "snake_case")]
async fn link_watch_suggestions(
    app: tauri::AppHandle,
    state: State<'_, ScanState>,
    matches: Vec<(i64, String)>,
) -> Result<usize, String> {
    // Same background-job treatment as scan_folder / bulk_preview_scan:
    // shared cancel flag (the Cancel button works), scan-progress log events
    // for the progress modal, plus link-progress (done, total) for the button.
    state.cancel.store(false, Ordering::Relaxed);
    let cancel_flag = state.cancel.clone();
    tauri::async_runtime::spawn_blocking(move || {
        let conn = indexer::open(&db_path()?).map_err(|e| e.to_string())?;
        let total = matches.len();
        let app_log = app.clone();
        let mut log = move |line: String| {
            let _ = app_log.emit("scan-progress", line);
        };
        let app_prog = app.clone();
        let mut progress = move |done: usize| {
            let _ = app_prog.emit("link-progress", (done, total));
        };
        ops::link_suggestions(&conn, &matches, Some(&cancel_flag), &mut progress, &mut log)
            .map_err(|e| e.to_string())
    })
    .await
    .map_err(|e| e.to_string())?
}


pub fn run() {
    tauri::Builder::default()
        .manage(ScanState::default())
        .manage(ExportState::default())
        .plugin(tauri_plugin_dialog::init())
        .setup(|app| {
            if let Ok(db) = db_path() {
                if let Ok(conn) = indexer::open(&db) {
                    let _ = indexer::reset_stale_export_jobs(&conn);
                    let _ = indexer::prune_stale_previews(&conn);
                }
            }
            let app_handle = app.handle().clone();
            tauri::async_runtime::spawn(async move {
                export_worker_loop(app_handle).await;
            });
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            search, inspect, stats, open_set, preview, scan_folder, cancel_scan, bulk_preview_scan,
            add_to_export_queue, add_to_export_queue_bulk, get_export_queue, remove_from_export_queue,
            clear_completed_jobs, toggle_export_queue, get_export_queue_active,
            retry_failed_jobs, remove_preview,
            add_watch_folder, remove_watch_folder, list_watch_folders,
            get_watch_suggestions, ignore_watch_suggestion, link_watch_suggestion,
            link_watch_suggestions
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}

