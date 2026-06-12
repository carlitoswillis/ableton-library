//! ops: the operations layer shared by the CLI and the desktop app.
//!
//! Everything here used to live in the CLI binary; it moved so the app can
//! run the same scans. Layering: als-core (parse) + previews (renders/peaks)
//! -> indexer (storage) -> ops (workflows) -> cli / app (frontends).

use std::collections::HashSet;
use std::path::{Path, PathBuf};

use anyhow::Result;
use rusqlite::Connection;

use als_core::scan::iso_mtime;
use als_core::{discover, parse_set, ParseError, SetSnapshot};
use previews::matching::{best_match, normalize, MatchTarget, SetCandidate};

/// Progress sink: cli prints to stderr, the app may ignore or forward.
pub type Log<'a> = &'a mut dyn FnMut(String);

/// Key for path-identity comparisons (known-samples cross-check, dedupe).
/// Lowercased because macOS filesystems are case-insensitive and path casing
/// can drift between scans (user observation 2026-06-11).
fn path_key(s: &str) -> String {
    s.to_lowercase()
}

/// Lowercase every path in a set so lookups via `path_key` match.
fn lowercase_paths(paths: HashSet<String>) -> HashSet<String> {
    paths.into_iter().map(|p| p.to_lowercase()).collect()
}

/// A planned preview-decode job: name matching + DB filtering already done,
/// only the expensive audio decode + peak extraction remains.
#[derive(Debug, Clone)]
pub struct DecodeJob {
    pub audio: PathBuf,
    pub set_id: Option<i64>,
    pub project_id: Option<i64>,
    pub confidence: f64,
}

/// Unit of work for the unified scan worker pool. Parsing .als files and
/// decoding preview audio share one pool so neither starves the other —
/// preview decoding must NEVER stall forward progress on project indexing.
enum Job {
    Parse { als: PathBuf, proj_dir: PathBuf, pid: i64 },
    Decode(DecodeJob),
}

enum Done {
    Parsed {
        als: PathBuf,
        pid: i64,
        res: Result<SetSnapshot, ParseError>,
    },
    Decoded {
        job: DecodeJob,
        res: Result<indexer::PreviewRow>,
    },
}

/// Two-priority work deque for the unified scan pool. Decode jobs go to the
/// FRONT so previews populate as soon as a project finishes ingesting,
/// instead of waiting behind the entire remaining parse backlog (a plain
/// FIFO channel had exactly that problem: previews only appeared at the end
/// of the scan).
struct JobQueue {
    q: std::sync::Mutex<std::collections::VecDeque<Job>>,
    cv: std::sync::Condvar,
    closed: std::sync::atomic::AtomicBool,
}

impl JobQueue {
    fn new() -> Self {
        Self {
            q: std::sync::Mutex::new(std::collections::VecDeque::new()),
            cv: std::sync::Condvar::new(),
            closed: std::sync::atomic::AtomicBool::new(false),
        }
    }

    /// Parse jobs: normal priority (back of the deque).
    fn push_back(&self, job: Job) {
        self.q.lock().unwrap().push_back(job);
        self.cv.notify_one();
    }

    /// Decode jobs: high priority (front of the deque).
    fn push_front(&self, job: Job) {
        self.q.lock().unwrap().push_front(job);
        self.cv.notify_one();
    }

    /// No more jobs will ever be pushed; wake everyone so they can exit.
    fn close(&self) {
        self.closed.store(true, std::sync::atomic::Ordering::Relaxed);
        self.cv.notify_all();
    }

    /// Blocking pop. Returns None when the queue is closed (and drained) or
    /// cancellation is requested. The timeout exists so a parked worker
    /// notices `cancel` even if nobody notifies the condvar again.
    fn pop(&self, cancel: Option<&std::sync::atomic::AtomicBool>) -> Option<Job> {
        let mut guard = self.q.lock().unwrap();
        loop {
            if let Some(c) = cancel {
                if c.load(std::sync::atomic::Ordering::Relaxed) {
                    return None;
                }
            }
            if let Some(job) = guard.pop_front() {
                return Some(job);
            }
            if self.closed.load(std::sync::atomic::Ordering::Relaxed) {
                return None;
            }
            let (g, _) = self
                .cv
                .wait_timeout(guard, std::time::Duration::from_millis(50))
                .unwrap();
            guard = g;
        }
    }
}

#[derive(Debug, Default, serde::Serialize)]
pub struct ScanSummary {
    pub indexed: usize,
    pub unchanged: usize,
    pub errors: usize,
    pub pruned: usize,
    pub harvested: usize,
}

/// Index a library root (incremental), then harvest in-folder renders.
pub fn scan_library(
    conn: &Connection,
    root: &Path,
    force: bool,
    harvest: bool,
    cancel: Option<&std::sync::atomic::AtomicBool>,
    log: Log,
) -> Result<ScanSummary> {
    let root_abs = std::path::absolute(root)?;
    let now = chrono::Utc::now().format("%Y-%m-%dT%H:%M:%S+00:00").to_string();

    let mut s = ScanSummary::default();
    let mut seen: HashSet<String> = HashSet::new();

    // Build known_samples up front (catches previously indexed sets), then
    // grow it incrementally as we ingest new sets so the harvest cross-check
    // is always up-to-date.
    let mut known_samples = if harvest {
        lowercase_paths(indexer::all_sample_paths(conn)?)
    } else {
        HashSet::new()
    };

    // Track how many parse tasks remain per project so we can harvest
    // immediately once a project is fully ingested.
    let mut pending_per_project: std::collections::HashMap<i64, usize> =
        std::collections::HashMap::new();
    // Project info needed for harvest, keyed by pid.
    let mut project_info: std::collections::HashMap<i64, (PathBuf, String)> =
        std::collections::HashMap::new();

    let mut parse_tasks = Vec::new();
    let mut initial_decode_jobs: Vec<DecodeJob> = Vec::new();

    conn.execute_batch("BEGIN")?;
    for proj in discover(&root_abs)? {
        if let Some(c) = cancel {
            if c.load(std::sync::atomic::Ordering::Relaxed) {
                anyhow::bail!("scan cancelled by user");
            }
        }
        let folder = std::path::absolute(&proj.dir)?.to_string_lossy().into_owned();
        let pid = indexer::upsert_project(conn, &folder, &proj.name, &now)?;
        indexer::replace_backups(conn, pid, &proj.backups)?;
        
        if harvest {
            project_info.insert(pid, (proj.dir.clone(), proj.name.clone()));
        }

        let mut project_task_count = 0usize;
        for als in proj.als_files {
            if let Some(c) = cancel {
                if c.load(std::sync::atomic::Ordering::Relaxed) {
                    anyhow::bail!("scan cancelled by user");
                }
            }
            let als_abs = std::path::absolute(&als)?.to_string_lossy().into_owned();
            seen.insert(als_abs.clone());
            let size = std::fs::metadata(&als)?.len();
            let mtime = iso_mtime(&als)?;
            if !force && indexer::set_is_fresh(conn, &als_abs, size, &mtime)? {
                s.unchanged += 1;
                continue;
            }
            parse_tasks.push((als, proj.dir.clone(), pid));
            project_task_count += 1;
        }

        if harvest && project_task_count > 0 {
            pending_per_project.insert(pid, project_task_count);
        } else if harvest && project_task_count == 0 {
            // All sets were fresh (unchanged) — plan a harvest in case new
            // renders appeared in the folder since last scan. Only the cheap
            // matching happens here; decoding is queued for the worker pool.
            match plan_folder_harvest(conn, &proj.dir, &proj.name, pid, &known_samples, cancel) {
                Ok(jobs) => initial_decode_jobs.extend(jobs),
                Err(e) => log(format!("preview harvest failed for {}: {e}", proj.dir.display())),
            }
        }
    }

    // Unified worker pool: .als parsing and preview decoding share one job
    // queue, so preview decoding never stalls forward progress on indexing.
    // Workers do CPU/disk work; the main thread does matching + SQLite writes.
    // Decode jobs are pushed to the FRONT of the queue so previews populate
    // live during the scan instead of after the whole parse backlog.
    let num_cpus = std::thread::available_parallelism().map(|n| n.get()).unwrap_or(4);
    let queue = JobQueue::new();
    // Results are bounded for backpressure; the job queue is unbounded so the
    // main thread (also the result consumer) never blocks while enqueueing.
    let (done_tx, done_rx) = std::sync::mpsc::sync_channel::<Done>(num_cpus * 2);

    let mut outstanding = 0usize;
    for (als, proj_dir, pid) in parse_tasks {
        queue.push_back(Job::Parse { als, proj_dir, pid });
        outstanding += 1;
    }
    for job in initial_decode_jobs {
        queue.push_front(Job::Decode(job));
        outstanding += 1;
    }

    std::thread::scope(|scope| {
        for _ in 0..num_cpus {
            let done_tx = done_tx.clone();
            let queue = &queue;
            scope.spawn(move || {
                loop {
                    match queue.pop(cancel) {
                        Some(Job::Parse { als, proj_dir, pid }) => {
                            let res = parse_set(&als, &proj_dir);
                            if done_tx.send(Done::Parsed { als, pid, res }).is_err() {
                                break;
                            }
                        }
                        Some(Job::Decode(job)) => {
                            let res = build_preview_row(
                                &job.audio,
                                job.set_id,
                                job.project_id,
                                "discovered",
                                job.confidence,
                            );
                            if done_tx.send(Done::Decoded { job, res }).is_err() {
                                break;
                            }
                        }
                        None => break, // queue closed or scan cancelled
                    }
                }
            });
        }
        drop(done_tx);

        while outstanding > 0 {
            // If workers bailed early (cancel), the done channel closes and
            // recv errs — break instead of waiting forever.
            let done = match done_rx.recv() {
                Ok(d) => d,
                Err(_) => break,
            };
            outstanding -= 1;

            match done {
                Done::Parsed { als, pid, res } => {
                    match res {
                        Ok(snap) => {
                            // Add this set's sample paths to known_samples before harvest
                            if harvest {
                                for sample in &snap.samples {
                                    known_samples.insert(path_key(&sample.path));
                                }
                            }

                            if let Err(e) = indexer::ingest_set(conn, pid, &snap) {
                                s.errors += 1;
                                log(format!("ERROR inserting {}: {}", als.display(), e));
                            } else {
                                s.indexed += 1;
                                log(format!("indexed {}", als.display()));
                            }
                        }
                        Err(e) => {
                            s.errors += 1;
                            log(format!("ERROR parsing {}: {}", als.display(), e));
                        }
                    }

                    // Project fully ingested (successes AND failures both
                    // count down) → plan its harvest and queue decode jobs.
                    if harvest {
                        if let Some(remaining) = pending_per_project.get_mut(&pid) {
                            *remaining -= 1;
                            if *remaining == 0 {
                                pending_per_project.remove(&pid);
                                if let Some((dir, name)) = project_info.remove(&pid) {
                                    match plan_folder_harvest(conn, &dir, &name, pid, &known_samples, cancel) {
                                        Ok(jobs) => {
                                            // Front of the queue: previews for a
                                            // finished project decode before the
                                            // remaining parse backlog.
                                            for job in jobs {
                                                queue.push_front(Job::Decode(job));
                                                outstanding += 1;
                                            }
                                        }
                                        Err(e) => log(format!(
                                            "preview harvest failed for {}: {e}",
                                            dir.display()
                                        )),
                                    }
                                }
                            }
                        }
                    }
                }
                Done::Decoded { job, res } => match res {
                    Ok(row) => {
                        if let Err(e) = indexer::upsert_preview(conn, &row) {
                            log(format!("ERROR inserting preview {}: {}", job.audio.display(), e));
                        } else {
                            s.harvested += 1;
                            log(format!("preview ({:.2}) {}", job.confidence, job.audio.display()));
                        }
                    }
                    Err(e) => log(format!("ERROR decoding {}: {}", job.audio.display(), e)),
                },
            }
        }

        // Close the job queue so idle workers exit and the scope can join.
        queue.close();
    });

    let stale_previews = indexer::prune_stale_previews(conn)?;
    for (_, path) in stale_previews {
        log(format!("preview removed (missing from disk): {}", path));
    }
    s.pruned = indexer::prune_missing(conn, &root_abs.to_string_lossy(), &seen)?;
    conn.execute_batch("COMMIT")?;

    Ok(s)
}

/// Extract peaks and build a PreviewRow (shared by harvest / hunt / attach).
fn build_preview_row(
    audio: &Path,
    set_id: Option<i64>,
    project_id: Option<i64>,
    source: &str,
    confidence: f64,
) -> Result<indexer::PreviewRow> {
    let meta = std::fs::metadata(audio)?;
    let pk = previews::peaks::extract(audio)?;
    Ok(indexer::PreviewRow {
        set_id,
        project_id,
        audio_path: std::path::absolute(audio)?.to_string_lossy().into_owned(),
        source: source.into(),
        confidence,
        mtime: iso_mtime(audio)?,
        size: meta.len(),
        duration: Some(pk.duration_secs),
        peaks_json: Some(previews::peaks::to_json(&pk.peaks)),
    })
}

/// Plan the harvest of renders found INSIDE one project folder: discover
/// audio files, name-match them to the project's sets, pick winners, and
/// filter against DB state. Returns the decode jobs still to be done — the
/// expensive part (audio decode + peaks) is deliberately NOT performed here
/// so callers can schedule it on a worker pool.
///
/// Folder placement is strong evidence, so matching is local and generous:
/// name match -> that set (+0.05 folder bonus); no name match but the project
/// has exactly one set -> that set at 0.7; otherwise skipped.
pub fn plan_folder_harvest(
    conn: &Connection,
    dir: &Path,
    project_name: &str,
    pid: i64,
    known_samples: &HashSet<String>,
    cancel: Option<&std::sync::atomic::AtomicBool>,
) -> Result<Vec<DecodeJob>> {
    let sets = indexer::project_sets(conn, pid)?;
    if sets.is_empty() {
        return Ok(Vec::new());
    }
    let norm_project = normalize(project_name.trim_end_matches(" Project"));
    let cands: Vec<SetCandidate> = sets
        .iter()
        .map(|(set_id, als_path)| SetCandidate {
            set_id: *set_id,
            project_id: pid,
            norm_stem: normalize(
                &Path::new(als_path)
                    .file_stem()
                    .map(|x| x.to_string_lossy().into_owned())
                    .unwrap_or_default(),
            ),
            norm_project: norm_project.clone(),
            project_set_count: sets.len(),
        })
        .collect();

    // First pass: Group by (set_id, project_id) and keep only the best match
    struct Win {
        render: previews::RenderFile,
        confidence: f64,
        mtime: String,
        size: u64,
    }
    let mut winners: std::collections::HashMap<(Option<i64>, Option<i64>), Win> = std::collections::HashMap::new();

    for r in previews::discover_renders(&[dir.to_path_buf()], Some(2))? {
        if let Some(c) = cancel {
            if c.load(std::sync::atomic::Ordering::Relaxed) {
                anyhow::bail!("scan cancelled by user");
            }
        }
        let abs = std::path::absolute(&r.path)?.to_string_lossy().into_owned();
        if known_samples.contains(&path_key(&abs)) {
            continue;
        }
        let (set_id, project_id, confidence) =
            match best_match(&normalize(&r.stem), &cands, 0.6) {
                Some(m) => match m.target {
                    MatchTarget::Set { set_id, project_id } => {
                        (Some(set_id), Some(project_id), (m.confidence + 0.05).min(1.0))
                    }
                    MatchTarget::Project { project_id } => (None, Some(project_id), m.confidence),
                },
                None if sets.len() == 1 => (Some(sets[0].0), Some(pid), 0.7),
                None => continue,
            };
            
        let mtime = iso_mtime(&r.path)?;
        let key = (set_id, project_id);
        
        let is_better = match winners.get(&key) {
            Some(existing) => {
                if confidence > existing.confidence {
                    true
                } else if (confidence - existing.confidence).abs() < f64::EPSILON {
                    mtime > existing.mtime
                } else {
                    false
                }
            }
            None => true,
        };
        
        if is_better {
            winners.insert(key, Win {
                render: r.clone(),
                confidence,
                mtime,
                size: r.size,
            });
        }
    }

    // Second pass: filter winners against DB state, emit decode jobs
    let mut jobs = Vec::new();
    for (key, win) in winners {
        let (set_id, project_id) = key;
        let abs = std::path::absolute(&win.render.path)?.to_string_lossy().into_owned();

        if indexer::preview_is_fresh(conn, set_id, &abs, win.size, &win.mtime)? {
            continue;
        }

        // If a primary preview already exists, ensure our new winner is strictly better
        if let Some(sid) = set_id {
            if let Ok(Some((db_conf, db_mtime))) = indexer::primary_preview_stats(conn, sid) {
                if win.confidence < db_conf {
                    continue; // DB already has a more confident match
                } else if (win.confidence - db_conf).abs() < f64::EPSILON && win.mtime <= db_mtime {
                    continue; // DB already has a newer or equally new match
                }
            }
        }

        jobs.push(DecodeJob {
            audio: win.render.path,
            set_id,
            project_id,
            confidence: win.confidence,
        });
    }
    Ok(jobs)
}

/// Plan + execute a single-folder harvest (decode parallelized internally).
/// Standalone entry point for callers outside a library scan (the app's
/// per-folder rescan); `scan_library` instead feeds `plan_folder_harvest`
/// jobs into its unified worker pool.
pub fn harvest_folder_renders(
    conn: &Connection,
    dir: &Path,
    project_name: &str,
    pid: i64,
    known_samples: &HashSet<String>,
    cancel: Option<&std::sync::atomic::AtomicBool>,
    log: Log,
) -> Result<usize> {
    let jobs = plan_folder_harvest(conn, dir, project_name, pid, known_samples, cancel)?;
    let mut count = 0usize;

    let num_cpus = std::thread::available_parallelism().map(|n| n.get()).unwrap_or(4);
    let (tx, rx) = std::sync::mpsc::sync_channel(num_cpus * 2);
    let jobs_iter = std::sync::Mutex::new(jobs.into_iter());

    std::thread::scope(|scope| {
        for _ in 0..num_cpus {
            let tx = tx.clone();
            let jobs_iter = &jobs_iter;
            scope.spawn(move || {
                loop {
                    let job = {
                        let mut iter = jobs_iter.lock().unwrap();
                        iter.next()
                    };
                    match job {
                        Some(job) => {
                            let row_res = build_preview_row(&job.audio, job.set_id, job.project_id, "discovered", job.confidence);
                            if tx.send((job, row_res)).is_err() {
                                break;
                            }
                        }
                        None => break,
                    }
                }
            });
        }
        drop(tx);

        for (job, row_res) in rx {
            match row_res {
                Ok(row) => {
                    if let Err(e) = indexer::upsert_preview(conn, &row) {
                        log(format!("ERROR inserting preview {}: {}", job.audio.display(), e));
                    } else {
                        count += 1;
                        log(format!("preview ({:.2}) {}", job.confidence, job.audio.display()));
                    }
                }
                Err(e) => log(format!("ERROR decoding {}: {}", job.audio.display(), e)),
            }
        }
    });

    Ok(count)
}

#[derive(Debug, Default, serde::Serialize)]
pub struct HuntSummary {
    pub matched: usize,
    pub unchanged: usize,
    pub ambiguous: usize,
    pub unmatched: usize,
    pub samples_skipped: usize,
    pub errors: usize,
}

/// Hunt arbitrary folders for renders and match them against the whole
/// catalog by name (renders are scattered — user decision 2026-06-11).
pub fn hunt_renders(
    conn: &Connection,
    roots: &[PathBuf],
    threshold: f64,
    verbose: bool,
    log: Log,
) -> Result<HuntSummary> {
    let cands = catalog_candidates(conn)?;
    if cands.is_empty() {
        anyhow::bail!("catalog is empty — scan a projects folder first");
    }
    let known_samples = lowercase_paths(indexer::all_sample_paths(conn)?);
    let renders = previews::discover_renders(roots, None)?;
    log(format!(
        "{} candidate audio file(s), matching against {} set(s)",
        renders.len(),
        cands.len()
    ));

    let mut s = HuntSummary::default();
    
    // First pass: Group by (set_id, project_id) and keep only the best match
    struct Win {
        render: previews::RenderFile,
        confidence: f64,
        mtime: String,
        size: u64,
    }
    let mut winners: std::collections::HashMap<(Option<i64>, Option<i64>), Win> = std::collections::HashMap::new();

    for r in &renders {
        let abs = std::path::absolute(&r.path)?.to_string_lossy().into_owned();
        if known_samples.contains(&path_key(&abs)) {
            s.samples_skipped += 1;
            if verbose {
                log(format!("skipped (known sample): {}", r.path.display()));
            }
            continue;
        }
        match best_match(&normalize(&r.stem), &cands, threshold) {
            Some(m) => {
                let key = match m.target {
                    MatchTarget::Set { set_id, project_id } => (Some(set_id), Some(project_id)),
                    MatchTarget::Project { project_id } => {
                        s.ambiguous += 1;
                        (None, Some(project_id))
                    }
                };
                let mtime = iso_mtime(&r.path)?;
                
                let is_better = match winners.get(&key) {
                    Some(existing) => {
                        if m.confidence > existing.confidence {
                            true
                        } else if (m.confidence - existing.confidence).abs() < f64::EPSILON {
                            mtime > existing.mtime
                        } else {
                            false
                        }
                    }
                    None => true,
                };
                
                if is_better {
                    winners.insert(key, Win {
                        render: r.clone(),
                        confidence: m.confidence,
                        mtime,
                        size: r.size,
                    });
                }
            }
            None => {
                s.unmatched += 1;
                if verbose {
                    log(format!("unmatched: {}", r.path.display()));
                }
            }
        }
    }

    // Second pass: filter winners against DB state
    let mut tasks = Vec::new();
    for (key, win) in winners {
        let (set_id, project_id) = key;
        let abs = std::path::absolute(&win.render.path)?.to_string_lossy().into_owned();
        
        // Skip if this exact file is already the preview
        if indexer::preview_is_fresh(conn, set_id, &abs, win.size, &win.mtime)? {
            s.unchanged += 1;
            continue;
        }

        // If a primary preview already exists, ensure our new winner is strictly better
        if let Some(sid) = set_id {
            if let Ok(Some((db_conf, db_mtime))) = indexer::primary_preview_stats(conn, sid) {
                if win.confidence < db_conf {
                    continue; // DB already has a more confident match
                } else if (win.confidence - db_conf).abs() < f64::EPSILON && win.mtime <= db_mtime {
                    continue; // DB already has a newer or equally new match
                }
            }
        }
        
        tasks.push((key, win));
    }

    // Third pass: decode audio in parallel
    let num_cpus = std::thread::available_parallelism().map(|n| n.get()).unwrap_or(4);
    let (tx, rx) = std::sync::mpsc::sync_channel(num_cpus * 2);
    let tasks_iter = std::sync::Mutex::new(tasks.into_iter());

    std::thread::scope(|scope| {
        for _ in 0..num_cpus {
            let tx = tx.clone();
            let tasks_iter = &tasks_iter;
            scope.spawn(move || {
                loop {
                    let task = {
                        let mut iter = tasks_iter.lock().unwrap();
                        iter.next()
                    };
                    match task {
                        Some((key, win)) => {
                            let (set_id, project_id) = key;
                            let row_res = build_preview_row(&win.render.path, set_id, project_id, "discovered", win.confidence);
                            if tx.send((win, row_res, set_id)).is_err() {
                                break;
                            }
                        }
                        None => break,
                    }
                }
            });
        }
        drop(tx);

        for (win, row_res, set_id) in rx {
            match row_res {
                Ok(row) => {
                    if let Err(e) = indexer::upsert_preview(conn, &row) {
                        s.errors += 1;
                        log(format!("ERROR inserting preview {}: {}", win.render.path.display(), e));
                    } else {
                        s.matched += 1;
                        log(format!("matched ({:.2}) {} -> set {:?}", win.confidence, win.render.path.display(), set_id));
                    }
                }
                Err(e) => {
                    s.errors += 1;
                    log(format!("ERROR decoding {}: {}", win.render.path.display(), e));
                }
            }
        }
    });

    Ok(s)
}

/// Manually attach an audio file to a set (confidence 1.0).
pub fn attach(conn: &Connection, set_id: i64, audio: &Path) -> Result<()> {
    let project_id = indexer::set_project_id(conn, set_id)?;
    let row = build_preview_row(audio, Some(set_id), Some(project_id), "manual", 1.0)?;
    indexer::upsert_preview(conn, &row)?;
    Ok(())
}

/// Bulk-link suggested bounce matches: move each audio file into its set's
/// project folder (renamed to the set's stem) and attach it as a manual
/// preview (confidence 1.0). File moves + SQLite writes run on the calling
/// thread; audio decoding + peak extraction are parallelized across cores.
/// `progress(done)` fires after each match is fully processed (linked OR
/// failed) so a UI can show `done/total`. Returns the number linked.
///
/// MUST be called from a blocking-safe thread (spawn_blocking in the app) —
/// this decodes audio and touches possibly-iCloud files.
pub fn link_suggestions(
    conn: &Connection,
    matches: &[(i64, String)],
    cancel: Option<&std::sync::atomic::AtomicBool>,
    progress: &mut dyn FnMut(usize),
    log: Log,
) -> Result<usize> {
    let cancelled =
        || cancel.is_some_and(|c| c.load(std::sync::atomic::Ordering::Relaxed));
    struct LinkTask {
        set_id: i64,
        project_id: i64,
        target: PathBuf,
    }

    // Phase 1 (calling thread): move files into place, collect decode tasks.
    let mut done = 0usize;
    let mut tasks: Vec<LinkTask> = Vec::new();
    for (set_id, audio_path) in matches {
        if cancelled() {
            anyhow::bail!("link cancelled by user");
        }
        let res = (|| -> Result<LinkTask> {
            let als_path = indexer::set_path(conn, *set_id)?;
            let als = Path::new(&als_path);
            let stem = als
                .file_stem()
                .map(|x| x.to_string_lossy().into_owned())
                .ok_or_else(|| anyhow::anyhow!("invalid set path: {als_path}"))?;
            let project_dir = als
                .parent()
                .ok_or_else(|| anyhow::anyhow!("set path has no parent dir: {als_path}"))?;
            let src = Path::new(audio_path);
            if !src.exists() {
                anyhow::bail!("source audio missing: {audio_path}");
            }
            let ext = src
                .extension()
                .map(|x| x.to_string_lossy().into_owned())
                .unwrap_or_else(|| "wav".into());
            let target = project_dir.join(format!("{stem}.{ext}"));
            std::fs::create_dir_all(project_dir)?;
            if std::fs::rename(src, &target).is_err() {
                // cross-device (e.g. iCloud <-> local): copy + delete
                std::fs::copy(src, &target)?;
                let _ = std::fs::remove_file(src);
            }
            Ok(LinkTask {
                set_id: *set_id,
                project_id: indexer::set_project_id(conn, *set_id)?,
                target,
            })
        })();
        match res {
            Ok(t) => tasks.push(t),
            Err(e) => {
                log(format!("link failed for set {set_id}: {e}"));
                done += 1;
                progress(done);
            }
        }
    }

    // Phase 2: decode in parallel, upsert sequentially on this thread.
    let num_cpus = std::thread::available_parallelism().map(|n| n.get()).unwrap_or(4);
    let (tx, rx) = std::sync::mpsc::sync_channel(num_cpus * 2);
    let tasks_iter = std::sync::Mutex::new(tasks.into_iter());
    let mut linked = 0usize;

    std::thread::scope(|scope| {
        for _ in 0..num_cpus {
            let tx = tx.clone();
            let tasks_iter = &tasks_iter;
            scope.spawn(move || {
                loop {
                    if let Some(c) = cancel {
                        if c.load(std::sync::atomic::Ordering::Relaxed) {
                            break;
                        }
                    }
                    let task = {
                        let mut iter = tasks_iter.lock().unwrap();
                        iter.next()
                    };
                    match task {
                        Some(t) => {
                            let row_res = build_preview_row(
                                &t.target,
                                Some(t.set_id),
                                Some(t.project_id),
                                "manual",
                                1.0,
                            );
                            if tx.send((t, row_res)).is_err() {
                                break;
                            }
                        }
                        None => break,
                    }
                }
            });
        }
        drop(tx);

        for (t, row_res) in rx {
            match row_res {
                Ok(row) => match indexer::upsert_preview(conn, &row) {
                    Ok(_) => {
                        linked += 1;
                        log(format!("linked {}", t.target.display()));
                    }
                    Err(e) => log(format!("ERROR inserting preview {}: {e}", t.target.display())),
                },
                Err(e) => log(format!("ERROR decoding {}: {e}", t.target.display())),
            }
            done += 1;
            progress(done);
        }
    });

    if cancelled() {
        anyhow::bail!("link cancelled by user ({linked} linked before cancel)");
    }
    Ok(linked)
}

/// Matcher candidates for the whole catalog.
fn catalog_candidates(conn: &Connection) -> Result<Vec<SetCandidate>> {
    let mut out = Vec::new();
    for (set_id, project_id, als_path, project_name, count) in
        indexer::set_match_candidates(conn)?
    {
        let stem = Path::new(&als_path)
            .file_stem()
            .map(|x| x.to_string_lossy().into_owned())
            .unwrap_or_default();
        out.push(SetCandidate {
            set_id,
            project_id,
            norm_stem: normalize(&stem),
            norm_project: normalize(project_name.trim_end_matches(" Project")),
            project_set_count: count as usize,
        });
    }
    Ok(out)
}

#[derive(Debug, serde::Serialize)]
pub struct Suggestion {
    pub set_id: i64,
    pub set_name: String,
    pub project_name: String,
    pub audio_path: String,
    pub file_name: String,
    pub confidence: f64,
    /// The set's PROJECT already has a preview somewhere — shown so users can
    /// swap in a better/newer bounce, but the UI must not auto-select these.
    pub has_preview: bool,
    /// This set's current primary preview file, if any — displayed so the
    /// user can compare/reconsider what is linked after the fact.
    pub current_preview: Option<String>,
}

pub fn get_watch_suggestions(conn: &Connection) -> Result<Vec<Suggestion>> {
    // 1. List watch folders
    let watch_folders = indexer::list_watch_folders(conn)?;
    if watch_folders.is_empty() {
        return Ok(Vec::new());
    }

    // 2. Discover audio files in watch folders
    let roots: Vec<PathBuf> = watch_folders.iter().map(|(_, p)| PathBuf::from(p)).collect();
    let renders = previews::discover_renders(&roots, Some(3))?; // limit depth to 3 for watch folders

    // 3. Match against ALL sets — sets that already have a preview are
    //    included (flagged via has_preview) so every matching bounce is
    //    visible, not just matches for preview-less sets.
    let cands = catalog_candidates(conn)?;
    if cands.is_empty() {
        return Ok(Vec::new());
    }

    // "Already previewed" is judged at the PROJECT level (user decision
    // 2026-06-11): if ANY preview exists for the set, a sibling set, or the
    // project itself (project-level match), suggestions for it are shown but
    // must never be auto-selected — replacements are explicit-only.
    let mut previewed_projects: HashSet<i64> = HashSet::new();
    {
        let mut stmt = conn.prepare(
            "SELECT DISTINCT project_id FROM previews WHERE project_id IS NOT NULL
             UNION
             SELECT DISTINCT s.project_id FROM previews pv
               JOIN sets s ON s.id = pv.set_id",
        )?;
        let rows = stmt.query_map([], |r| r.get::<_, i64>(0))?;
        for row in rows {
            previewed_projects.insert(row?);
        }
    }

    // 4. Match
    let mut suggestions = Vec::new();
    let known_samples = lowercase_paths(indexer::all_sample_paths(conn)?);
    // Cache each set's current primary preview path (shown for reconsideration).
    let mut primary_cache: std::collections::HashMap<i64, Option<String>> =
        std::collections::HashMap::new();

    for r in &renders {
        let abs = std::path::absolute(&r.path)?.to_string_lossy().into_owned();
        if known_samples.contains(&path_key(&abs)) {
            continue;
        }

        if let Some(m) = best_match(&normalize(&r.stem), &cands, 0.6) {
            if let MatchTarget::Set { set_id, project_id } = m.target {
                // Check if this match is ignored in database
                if indexer::is_match_ignored(conn, set_id, &abs)? {
                    continue;
                }

                // Check if this audio path is already a preview for this set (just in case)
                let already_exists: i64 = conn.query_row(
                    "SELECT COUNT(*) FROM previews WHERE set_id = ?1 AND audio_path = ?2",
                    rusqlite::params![set_id, abs],
                    |row| row.get(0),
                )?;
                if already_exists > 0 {
                    continue;
                }

                // Get details about this set
                let (als_path, project_name): (String, String) = conn.query_row(
                    "SELECT s.als_path, p.name FROM sets s JOIN projects p ON p.id = s.project_id WHERE s.id = ?1",
                    rusqlite::params![set_id],
                    |row| Ok((row.get(0)?, row.get(1)?)),
                )?;

                let set_name = Path::new(&als_path)
                    .file_name()
                    .map(|x| x.to_string_lossy().into_owned())
                    .unwrap_or_else(|| als_path.clone());

                let file_name = r.path.file_name()
                    .map(|x| x.to_string_lossy().into_owned())
                    .unwrap_or_else(|| abs.clone());

                let current_preview = primary_cache
                    .entry(set_id)
                    .or_insert_with(|| {
                        indexer::primary_preview(conn, set_id)
                            .ok()
                            .flatten()
                            .map(|(path, ..)| path)
                    })
                    .clone();

                suggestions.push(Suggestion {
                    set_id,
                    set_name,
                    project_name,
                    audio_path: abs,
                    file_name,
                    confidence: m.confidence,
                    has_preview: previewed_projects.contains(&project_id),
                    current_preview,
                });
            }
        }
    }

    // Sort suggestions by confidence DESC
    suggestions.sort_by(|a, b| b.confidence.partial_cmp(&a.confidence).unwrap_or(std::cmp::Ordering::Equal));

    Ok(suggestions)
}

