//! Tauri backend: thin command layer over the shared `indexer` crate.
//! All query logic lives in indexer — the CLI and this app are equals.

use std::path::PathBuf;
use tauri::Manager;
use tokio::process::Command;

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
    artist: Option<String>,
    list_id: Option<i64>,
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
            artist: none_if_blank(artist),
            list_id,
            sort_by: none_if_blank(sort_by),
            date_modified: none_if_blank(date_modified),
            date_scanned: none_if_blank(date_scanned),
            has_preview: none_if_blank(has_preview),
        },
    )
    .map_err(|e| e.to_string())
}

// ---- User lists (favorites + collections) --------------------------------

/// All lists as (id, name, item_count).
#[tauri::command(rename_all = "snake_case")]
async fn get_lists() -> Result<Vec<(i64, String, i64)>, String> {
    let conn = indexer::open(&db_path()?).map_err(|e| e.to_string())?;
    indexer::all_lists(&conn).map_err(|e| e.to_string())
}

/// Create (or get existing) a list by name. Returns its id.
#[tauri::command(rename_all = "snake_case")]
async fn create_list(name: String) -> Result<i64, String> {
    let conn = indexer::open(&db_path()?).map_err(|e| e.to_string())?;
    let name = name.trim();
    if name.is_empty() {
        return Err("list name cannot be empty".into());
    }
    indexer::create_list(&conn, name).map_err(|e| e.to_string())
}

#[tauri::command(rename_all = "snake_case")]
async fn delete_list(list_id: i64) -> Result<(), String> {
    let conn = indexer::open(&db_path()?).map_err(|e| e.to_string())?;
    indexer::delete_list(&conn, list_id).map_err(|e| e.to_string())
}

#[tauri::command(rename_all = "snake_case")]
async fn rename_list(list_id: i64, name: String) -> Result<(), String> {
    let conn = indexer::open(&db_path()?).map_err(|e| e.to_string())?;
    indexer::rename_list(&conn, list_id, name.trim()).map_err(|e| e.to_string())
}

/// The list ids a set currently belongs to (for the star popover checkboxes).
#[tauri::command(rename_all = "snake_case")]
async fn lists_for_set(set_id: i64) -> Result<Vec<i64>, String> {
    let conn = indexer::open(&db_path()?).map_err(|e| e.to_string())?;
    let path = indexer::set_path(&conn, set_id).map_err(|e| e.to_string())?;
    indexer::lists_for_path(&conn, &path).map_err(|e| e.to_string())
}

#[tauri::command(rename_all = "snake_case")]
async fn add_set_to_list(list_id: i64, set_id: i64) -> Result<(), String> {
    let conn = indexer::open(&db_path()?).map_err(|e| e.to_string())?;
    let path = indexer::set_path(&conn, set_id).map_err(|e| e.to_string())?;
    indexer::add_to_list(&conn, list_id, &path).map_err(|e| e.to_string())
}

#[tauri::command(rename_all = "snake_case")]
async fn remove_set_from_list(list_id: i64, set_id: i64) -> Result<(), String> {
    let conn = indexer::open(&db_path()?).map_err(|e| e.to_string())?;
    let path = indexer::set_path(&conn, set_id).map_err(|e| e.to_string())?;
    indexer::remove_from_list(&conn, list_id, &path).map_err(|e| e.to_string())
}

/// Distinct artists with project counts (for the artist filter dropdown).
#[tauri::command(rename_all = "snake_case")]
async fn list_artists() -> Result<Vec<(Option<String>, i64)>, String> {
    let conn = indexer::open(&db_path()?).map_err(|e| e.to_string())?;
    indexer::list_artists(&conn).map_err(|e| e.to_string())
}

/// Backfill artists from stored project paths — no scan, no re-parse.
/// Returns how many projects were tagged.
#[tauri::command(rename_all = "snake_case")]
async fn reindex_artists() -> Result<usize, String> {
    tauri::async_runtime::spawn_blocking(|| {
        let conn = indexer::open(&db_path()?).map_err(|e| e.to_string())?;
        ops::reindex_artists(&conn).map_err(|e| e.to_string())
    })
    .await
    .map_err(|e| e.to_string())?
}

/// Manually set/clear ONE set's artist override (blank clears -> the set falls
/// back to its project's derived artist).
#[tauri::command(rename_all = "snake_case")]
async fn set_artist(set_id: i64, artist: Option<String>) -> Result<(), String> {
    let conn = indexer::open(&db_path()?).map_err(|e| e.to_string())?;
    indexer::set_set_artist_override(&conn, set_id, none_if_blank(artist).as_deref())
        .map_err(|e| e.to_string())
}

/// Manually set/clear the artist for the WHOLE project the set belongs to
/// (every set in that folder, unless a set has its own override).
#[tauri::command(rename_all = "snake_case")]
async fn set_project_artist(set_id: i64, artist: Option<String>) -> Result<(), String> {
    let conn = indexer::open(&db_path()?).map_err(|e| e.to_string())?;
    let pid = indexer::set_project_id(&conn, set_id).map_err(|e| e.to_string())?;
    indexer::set_project_artist_opt(&conn, pid, none_if_blank(artist).as_deref())
        .map_err(|e| e.to_string())
}

/// Set/clear the per-set artist override on many sets at once (bulk tagging
/// from the results selection). Returns how many sets were touched.
#[tauri::command(rename_all = "snake_case")]
async fn set_artist_bulk(set_ids: Vec<i64>, artist: Option<String>) -> Result<usize, String> {
    let conn = indexer::open(&db_path()?).map_err(|e| e.to_string())?;
    let a = none_if_blank(artist);
    for id in &set_ids {
        indexer::set_set_artist_override(&conn, *id, a.as_deref()).map_err(|e| e.to_string())?;
    }
    Ok(set_ids.len())
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
    /// Renderability report for worker renders of imperfect sets
    /// (missing plugins / samples). Null = full fidelity.
    fidelity: Option<serde_json::Value>,
}

/// Plugin inventory: a recursive bundle-filename scan (ms-fast; auval was
/// removed after proving a black hole). Built lazily, refreshable on demand
/// (Re-triage button rebuilds it so newly installed plugins are seen).
static INVENTORY: std::sync::OnceLock<
    std::sync::RwLock<std::sync::Arc<ops::triage::InstalledPlugins>>,
> = std::sync::OnceLock::new();

fn inventory_cell() -> &'static std::sync::RwLock<std::sync::Arc<ops::triage::InstalledPlugins>> {
    INVENTORY.get_or_init(|| {
        let inv = ops::triage::installed_plugins();
        eprintln!("[triage] plugin inventory: {} names (folder scan)", inv.names.len());
        std::sync::RwLock::new(std::sync::Arc::new(inv))
    })
}

fn plugin_inventory() -> std::sync::Arc<ops::triage::InstalledPlugins> {
    inventory_cell().read().unwrap().clone()
}

fn refresh_plugin_inventory() {
    let fresh = std::sync::Arc::new(ops::triage::installed_plugins());
    eprintln!("[triage] plugin inventory refreshed: {} names", fresh.names.len());
    *inventory_cell().write().unwrap() = fresh;
}

/// Primary preview (audio path + waveform peaks) for a set, if any.
#[tauri::command(rename_all = "snake_case")]
async fn preview(set_id: i64) -> Result<Option<PreviewInfo>, String> {
    let conn = indexer::open(&db_path()?).map_err(|e| e.to_string())?;
    let row = indexer::primary_preview(&conn, set_id).map_err(|e| e.to_string())?;
    if let Some((audio_path, duration, peaks_json, confidence, source, fidelity_json)) = row {
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
            fidelity: fidelity_json.and_then(|j| serde_json::from_str(&j).ok()),
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
    tx: tokio::sync::watch::Sender<bool>,
}

impl Default for ExportState {
    fn default() -> Self {
        let (tx, _) = tokio::sync::watch::channel(false);
        Self {
            active: Arc::new(AtomicBool::new(false)),
            tx,
        }
    }
}

/// Score unscored pending jobs in the background and refresh the UI when
/// done. Detached (never awaited by enqueue commands) as a standing rule:
/// nothing user-facing waits on enrichment work.
fn spawn_job_scoring(app: tauri::AppHandle, refresh_inventory: bool) {
    tauri::async_runtime::spawn_blocking(move || {
        let Ok(p) = db_path() else { return };
        let Ok(conn) = indexer::open(&p) else { return };
        if refresh_inventory {
            refresh_plugin_inventory();
        }
        let unscored = match indexer::unscored_pending_jobs(&conn) {
            Ok(n) if n > 0 => n,
            _ => return,
        };
        let installed = plugin_inventory();
        eprintln!(
            "[triage] {unscored} unscored job(s); scoring with {} known plugin names",
            installed.names.len()
        );
        let t1 = std::time::Instant::now();
        let mut log = |line: String| eprintln!("[triage] {line}");
        match ops::triage::score_pending_jobs(&conn, &installed, &mut log) {
            Ok(n) => {
                eprintln!(
                    "[triage] scored {n}/{unscored} job(s) in {:.1}s",
                    t1.elapsed().as_secs_f32()
                );
                if n > 0 {
                    let _ = app.emit("export-queue-updated", ());
                }
            }
            Err(e) => eprintln!("[triage] scoring pass failed: {e}"),
        }
        // Full re-triage also re-stamps worker previews so the player/queue
        // stop quoting fidelity computed by older logic.
        if refresh_inventory {
            let inv = plugin_inventory();
            let mut log2 = |line: String| eprintln!("[triage] {line}");
            match ops::triage::restamp_worker_previews(&conn, &inv, &mut log2) {
                Ok(n) => {
                    eprintln!("[triage] restamped {n} worker preview set(s)");
                    let _ = app.emit("export-queue-updated", ());
                }
                Err(e) => eprintln!("[triage] restamp failed: {e}"),
            }
        }
    });
}

#[tauri::command(rename_all = "snake_case")]
async fn add_to_export_queue(app: tauri::AppHandle, set_id: i64) -> Result<(), String> {
    let conn = indexer::open(&db_path()?).map_err(|e| e.to_string())?;
    indexer::add_export_job(&conn, set_id).map_err(|e| e.to_string())?;
    let _ = app.emit("export-queue-updated", ());
    spawn_job_scoring(app, false); // badges fill in when ready
    Ok(())
}

/// Bulk export: queue renders for many sets at once (multi-select in the UI).
/// Returns how many were actually queued (active renders are skipped).
#[tauri::command(rename_all = "snake_case")]
async fn add_to_export_queue_bulk(app: tauri::AppHandle, set_ids: Vec<i64>) -> Result<usize, String> {
    let conn = indexer::open(&db_path()?).map_err(|e| e.to_string())?;
    let queued = indexer::add_export_jobs_bulk(&conn, &set_ids).map_err(|e| e.to_string())?;
    let _ = app.emit("export-queue-updated", ());
    spawn_job_scoring(app, false);
    Ok(queued)
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

/// Clear + recompute triage scores for pending jobs (user-triggered, e.g.
/// after installing plugins or when scores look wrong).
#[tauri::command(rename_all = "snake_case")]
async fn retriage_jobs(app: tauri::AppHandle) -> Result<(), String> {
    let conn = indexer::open(&db_path()?).map_err(|e| e.to_string())?;
    indexer::clear_pending_job_scores(&conn).map_err(|e| e.to_string())?;
    indexer::clear_finished_job_fidelity(&conn).map_err(|e| e.to_string())?;
    let _ = app.emit("export-queue-updated", ()); // badges drop instantly…
    spawn_job_scoring(app, true); // …and refill with a FRESH inventory
    Ok(())
}

/// Clear the whole queue (a mid-render `processing` job survives).
#[tauri::command(rename_all = "snake_case")]
async fn clear_all_jobs() -> Result<usize, String> {
    let conn = indexer::open(&db_path()?).map_err(|e| e.to_string())?;
    indexer::clear_all_export_jobs(&conn).map_err(|e| e.to_string())
}

#[tauri::command(rename_all = "snake_case")]
async fn retry_failed_jobs() -> Result<(), String> {
    let conn = indexer::open(&db_path()?).map_err(|e| e.to_string())?;
    indexer::retry_failed_export_jobs(&conn).map_err(|e| e.to_string())
}

#[tauri::command(rename_all = "snake_case")]
async fn toggle_export_queue(state: State<'_, ExportState>, active: bool) -> Result<(), String> {
    state.active.store(active, Ordering::Relaxed);
    let _ = state.tx.send(active);
    Ok(())
}

#[tauri::command(rename_all = "snake_case")]
async fn get_export_queue_active(state: State<'_, ExportState>) -> Result<bool, String> {
    Ok(state.active.load(Ordering::Relaxed))
}

#[tauri::command(rename_all = "snake_case")]
async fn create_proxy_set(set_id: i64) -> Result<String, String> {
    tauri::async_runtime::spawn_blocking(move || {
        let conn = indexer::open(&db_path()?).map_err(|e| e.to_string())?;
        let mut log = |line: String| eprintln!("[proxy] {line}");
        let path = ops::proxy::create_proxy_set(&conn, set_id, &mut log).map_err(|e| e.to_string())?;
        Ok(path.to_string_lossy().into_owned())
    })
    .await
    .map_err(|e| e.to_string())?
}

async fn export_worker_loop(app: tauri::AppHandle) {
    let mut rx = {
        let state = app.state::<ExportState>();
        state.tx.subscribe()
    };

    loop {
        // 1. Wait for queue to be active
        if !*rx.borrow() {
            if rx.changed().await.is_err() {
                break;
            }
            if !*rx.borrow() {
                continue;
            }
        }

        tokio::time::sleep(std::time::Duration::from_secs(3)).await;

        // Re-check active after sleep
        if !*rx.borrow() {
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

        // Mark as processing
        if let Err(_) = indexer::update_export_job_status(&conn, job_id, "processing", None) {
            continue;
        }
        let _ = app.emit("export-queue-updated", ());

        let als_parent = std::path::Path::new(&als_path).parent().unwrap().to_path_buf();
        let expected_output = als_parent.join(format!("{}.wav", set_stem));
        let fresh_existing = (|| -> Option<bool> {
            let wav_m = std::fs::metadata(&expected_output).ok()?.modified().ok()?;
            let als_m = std::fs::metadata(&als_path).ok()?.modified().ok()?;
            Some(wav_m >= als_m)
        })()
        .unwrap_or(false);

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

        let log_output = std::sync::Arc::new(std::sync::Mutex::new(String::new()));
        let log = {
            let log_output = log_output.clone();
            move |line: String| {
                eprintln!("{line}");
                let mut out = log_output.lock().unwrap();
                out.push_str(&line);
                out.push('\n');
            }
        };

        let mut error_msg = None;
        let mut success = false;
        // Set when the export script reports a SYSTEMIC Accessibility-permission
        // failure (sentinel exit code 42, or the keystroke-1002 signature in the
        // logs). Such a failure is identical for every job, so we pause the whole
        // queue instead of grinding through and failing all of them.
        let mut permission_failure = false;

        if fresh_existing {
            log(format!("[worker] found fresh existing render: {}", expected_output.display()));
            success = true;
        } else {
            let proxy_res = {
                let db_p = db_path().unwrap();
                let log_output_clone = log_output.clone();
                tauri::async_runtime::spawn_blocking(move || {
                    let conn = indexer::open(&db_p).map_err(|e| e.to_string())?;
                    let mut l = |line: String| {
                        let msg = format!("[worker-proxy] {line}");
                        eprintln!("{msg}");
                        let mut out = log_output_clone.lock().unwrap();
                        out.push_str(&msg);
                        out.push('\n');
                    };
                    ops::proxy::create_proxy_set(&conn, set_id, &mut l).map_err(|e| e.to_string())
                }).await
            };

            let (render_path, is_proxy) = match proxy_res {
                Ok(Ok(p)) => (p, true),
                Ok(Err(e)) => {
                    log(format!("[worker] proxy creation failed: {e}; falling back to original"));
                    (als_path.into(), false)
                }
                Err(e) => {
                    log(format!("[worker] proxy creation panicked: {e}"));
                    (als_path.into(), false)
                }
            };

            let (_, samples_raw) = indexer::set_render_inputs(&conn, set_id).unwrap_or_default();
            let sample_paths: Vec<String> = samples_raw.into_iter().map(|(p, _)| p).collect();
            if !sample_paths.is_empty() {
                let sp = sample_paths.clone();
                log(format!("[worker] checking/materializing {} samples", sp.len()));
                let _ = tauri::async_runtime::spawn_blocking(move || {
                    ops::triage::materialize_icloud_samples(
                        &sp,
                        std::time::Duration::from_secs(180),
                    )
                })
                .await;
            }

            let output_name = set_stem.clone();
            use std::process::Stdio;
            let mut child = Command::new("python3")
                .arg(&script_path)
                .arg("--set-path")
                .arg(&render_path)
                .arg("--output-dir")
                .arg(&output_dir_str)
                .arg("--output-name")
                .arg(&output_name)
                .arg("--live-app")
                .arg(live_app)
                .stdout(Stdio::piped())
                .stderr(Stdio::piped())
                .spawn();

            match child.as_mut() {
                Ok(c) => {
                    let stdout = c.stdout.take().unwrap();
                    let stderr = c.stderr.take().unwrap();
                    let (log_tx, mut log_rx) = tokio::sync::mpsc::channel(100);

                    // Task to read stdout
                    let log_tx_stdout = log_tx.clone();
                    tokio::spawn(async move {
                        use tokio::io::AsyncBufReadExt;
                        let mut reader = tokio::io::BufReader::new(stdout).lines();
                        while let Ok(Some(line)) = reader.next_line().await {
                            let _ = log_tx_stdout.send(format!("[script] {line}")).await;
                        }
                    });

                    // Task to read stderr
                    let log_tx_stderr = log_tx.clone();
                    tokio::spawn(async move {
                        use tokio::io::AsyncBufReadExt;
                        let mut reader = tokio::io::BufReader::new(stderr).lines();
                        while let Ok(Some(line)) = reader.next_line().await {
                            let _ = log_tx_stderr.send(format!("[script-err] {line}")).await;
                        }
                    });

                    let timeout_dur = std::time::Duration::from_secs(720); // 12 mins
                    let timeout = tokio::time::sleep(timeout_dur);
                    tokio::pin!(timeout);

                    loop {
                        tokio::select! {
                            res = c.wait() => {
                                if let Ok(status) = res {
                                    if status.success() {
                                        success = true;
                                    } else if status.code() == Some(42) {
                                        permission_failure = true;
                                        error_msg = Some(PERMISSION_ERROR_MSG.into());
                                    } else {
                                        error_msg = Some(format!("Script failed with exit code {}", status.code().unwrap_or(-1)));
                                    }
                                } else {
                                    error_msg = Some("Failed to wait for script".into());
                                }
                                break;
                            }
                            Some(line) = log_rx.recv() => {
                                log(line);
                            }
                            _ = rx.changed() => {
                                if !*rx.borrow() {
                                    log("[worker] cancellation requested, killing export process".into());
                                    let _ = c.kill().await;
                                    error_msg = Some("Export cancelled by user".into());
                                    break;
                                }
                            }
                            _ = &mut timeout => {
                                log("[worker] export script timed out (12m)".into());
                                let _ = c.kill().await;
                                error_msg = Some("Export timed out after 12 minutes".into());
                                break;
                            }
                        }
                    }

                    // Drain remaining logs
                    while let Ok(line) = log_rx.try_recv() {
                        log(line);
                    }
                }
                Err(e) => {
                    error_msg = Some(format!("Failed to spawn export script: {e}"));
                }
            }

            // Belt-and-suspenders: even if the sentinel exit code didn't survive
            // (e.g. the script was killed), recognise the keystroke-permission
            // signature in the captured logs and treat it as systemic.
            if !success && !permission_failure {
                let logs = log_output.lock().unwrap().to_lowercase();
                if logs.contains("not allowed to send keystrokes")
                    || logs.contains("(1002)")
                    || logs.contains("-1719")
                {
                    permission_failure = true;
                    error_msg = Some(PERMISSION_ERROR_MSG.into());
                }
            }

            if is_proxy {
                let _ = std::fs::remove_file(&render_path);
                log(format!("[worker] deleted proxy set: {}", render_path.display()));
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
                let inv = plugin_inventory();
                let fidelity_json = ops::triage::renderability(&conn, set_id, &inv)
                    .ok()
                    .filter(|r| !r.missing_plugins.is_empty() || r.samples_missing > 0 || r.samples_evicted > 0)
                    .and_then(|r| serde_json::to_string(&r).ok());
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
                    fidelity_json,
                };
                indexer::upsert_preview(&conn, &row)?;
                indexer::update_export_job_status(&conn, job_id, "completed", None)?;
                Ok::<(), anyhow::Error>(())
            }).await;

            if let Err(e) = ingest_result {
                let logs = log_output.lock().unwrap().clone();
                let err_str = format!("Ingestion failed: {}\n\nLogs:\n{}", e, logs);
                let _ = indexer::update_export_job_status(&conn, job_id, "failed", Some(&err_str));
            }
        } else {
            let err_str = error_msg.unwrap_or_else(|| "Unknown export error".into());
            if err_str == "Export cancelled by user" {
                let _ = indexer::update_export_job_status(&conn, job_id, "failed", Some(&err_str));
            } else {
                let logs = log_output.lock().unwrap().clone();
                let detailed_err = format!("{}\n\nLogs:\n{}", err_str, logs);
                let _ = indexer::update_export_job_status(&conn, job_id, "failed", Some(&detailed_err));
            }
        }

        // A missing Accessibility grant fails identically for every job. Pause
        // the queue so we don't burn through (and proxy-churn) the whole backlog
        // — the user fixes permissions, then re-enables Auto-Export.
        if permission_failure {
            let state = app.state::<ExportState>();
            state.active.store(false, Ordering::Relaxed);
            let _ = state.tx.send(false);
            log("[worker] Accessibility permission missing — Auto-Export paused. \
                 Grant Accessibility access to the app in System Settings, then re-enable."
                .into());
        }

        let _ = app.emit("export-queue-updated", ());
    }
}

/// Shown on a job and logged when automated rendering is blocked because macOS
/// hasn't granted the app Accessibility permission (keystroke error 1002).
const PERMISSION_ERROR_MSG: &str = "Automated rendering is blocked: macOS won't let this app send keystrokes to drive Ableton Live (Accessibility permission).\n\nThe grant attaches to whatever LAUNCHED the app, not the app binary itself:\n  • Dev build via `tauri dev` from a terminal → grant your TERMINAL (iTerm / Terminal) Accessibility. This is stable across rebuilds — do it once.\n  • A built .app launched by double-click → grant the app itself.\nSystem Settings → Privacy & Security → Accessibility → enable it (add with +, or toggle OFF/ON if already listed). Then turn Auto-Export back on.\n\nThe queue has been paused so the rest of your jobs weren't burned.";

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
            None, // artist override: in-app scans rely on the path-based guess
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
        
        // NOTE: lowercased — ops compares sample paths case-insensitively.
        let known_samples: std::collections::HashSet<String> = indexer::all_sample_paths(&conn)
            .map_err(|e| e.to_string())?
            .into_iter()
            .map(|p| p.to_lowercase())
            .collect();

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

/// Scan ONE project's folder for preview files (detail-pane action).
/// Harvests renders inside the folder of the set's project; spawn_blocking
/// because it decodes audio.
#[tauri::command(rename_all = "snake_case")]
async fn scan_set_folder_previews(app: tauri::AppHandle, set_id: i64) -> Result<usize, String> {
    tauri::async_runtime::spawn_blocking(move || {
        let conn = indexer::open(&db_path()?).map_err(|e| e.to_string())?;
        let (pid, folder, name): (i64, String, String) = conn
            .query_row(
                "SELECT p.id, p.folder_path, p.name
                 FROM projects p JOIN sets s ON s.project_id = p.id
                 WHERE s.id = ?1",
                [set_id],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
            )
            .map_err(|e| e.to_string())?;
        let known_samples = indexer::all_sample_paths(&conn)
            .map_err(|e| e.to_string())?
            .into_iter()
            .map(|p| p.to_lowercase())
            .collect();
        let app_clone = app.clone();
        let mut log = move |line: String| {
            let _ = app_clone.emit("scan-progress", line);
        };
        ops::harvest_folder_renders(
            &conn,
            std::path::Path::new(&folder),
            &name,
            pid,
            &known_samples,
            None,
            &mut log,
        )
        .map_err(|e| e.to_string())
    })
    .await
    .map_err(|e| e.to_string())?
}

/// Manually attach ANY audio file to a set as its preview, regardless of name.
/// For when a bounce got named in a way the auto-matcher won't catch. The file
/// is referenced in place (never moved/copied); attached as source='manual',
/// confidence 1.0, so it becomes the primary preview. Decodes audio for the
/// waveform, so it runs in spawn_blocking.
#[tauri::command(rename_all = "snake_case")]
async fn attach_preview(set_id: i64, audio_path: String) -> Result<(), String> {
    tauri::async_runtime::spawn_blocking(move || {
        let p = std::path::Path::new(&audio_path);
        if !p.exists() {
            return Err(format!("file not found: {audio_path}"));
        }
        let conn = indexer::open(&db_path()?).map_err(|e| e.to_string())?;
        ops::attach(&conn, set_id, p).map_err(|e| e.to_string())
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
                    // NOTE: scores are NOT cleared at launch (user decision
                    // 2026-06-12) — once scored, jobs stay scored. Re-triage
                    // button / `rescore` CLI are the explicit refresh paths.
                }
            }
            // Only jobs that never got scored (e.g. queued right before the
            // last quit) are picked up here; already-scored jobs are skipped.
            spawn_job_scoring(app.handle().clone(), false);
            let app_handle = app.handle().clone();
            tauri::async_runtime::spawn(async move {
                export_worker_loop(app_handle).await;
            });
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            search, list_artists, reindex_artists, set_artist, set_project_artist, set_artist_bulk,
            get_lists, create_list, delete_list, rename_list, lists_for_set, add_set_to_list, remove_set_from_list,
            inspect, stats, open_set, preview, scan_folder, cancel_scan, bulk_preview_scan,
            scan_set_folder_previews, attach_preview,
            add_to_export_queue, add_to_export_queue_bulk, get_export_queue, remove_from_export_queue,
            clear_completed_jobs, clear_all_jobs, retriage_jobs, toggle_export_queue, get_export_queue_active,
            retry_failed_jobs, remove_preview,
            add_watch_folder, remove_watch_folder, list_watch_folders,
            get_watch_suggestions, ignore_watch_suggestion, link_watch_suggestion,
            link_watch_suggestions, create_proxy_set
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}

