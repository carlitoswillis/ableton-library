# Architecture

PURPOSE: Technical system design and data flow of the Ableton Library application.

## Overview
Ableton Library is a metadata and preview indexing system for Ableton projects, allowing users to browse and search their library without opening Ableton Live.

## Stack Decision (2026-06-11)
**Rust core + Tauri 2 desktop shell + React/TS frontend + SQLite.** CLI-first: the extraction core and indexer ship as a Rust CLI and are validated against the real library before any UI is built.

### Repository Layout (Cargo workspace)
```
crates/als-core/   # lib: gzip (flate2) + streaming XML (quick-xml) -> SetSnapshot; discovery  [BUILT, verified]
crates/previews/   # lib: render discovery, name matching, symphonia peaks  [BUILT]
crates/indexer/    # lib: SQLite (rusqlite + FTS5) storage; pure, no workflow logic  [BUILT, verified]
crates/ops/        # lib: workflows (scan_library, hunt_renders, attach) shared by cli + app; multi-threaded  [BUILT]
crates/cli/        # bin: `ableton-scan` — thin wrappers over ops/indexer  [BUILT, verified]
tools/reference_extract.py  # executable spec / test oracle for als-core; keep in sync
app/               # Tauri 2 + React/TS  [BUILT, awaiting first run]; later: symphonia for waveform peaks
```

## System Components

### 1. Filesystem Scanner — `als-core` (Rust)
- **Purpose**: Extract project information from Live Sets and folders.
- **Approach**: Streaming XML parse (never full DOM — .als can decompress to 100s of MB).
- **Version tolerance**: No Ableton SDK (user on Live 11; SDK is Live 12 Suite beta only). Parse leniently across Live versions, backward (9/10/11) and forward (12+): ignore unknown elements, tolerate missing ones, record Creator/version per set, and emit per-field extraction warnings instead of failing the whole file.
- **Extracts**: Live version, tempo/time sig, tracks (type/name/color), clip names, device/plugin names, sample file references.
- **Output**: Normalized ProjectSnapshot JSON per set.
- **Concurrency**: `scan_library` in the `ops` crate runs ONE worker pool (all CPU cores, `std::thread::scope`) consuming a **unified two-priority job deque** (`JobQueue`: `Mutex<VecDeque>` + `Condvar`) of two job kinds: `Parse` (.als decompression + XML parsing, pushed to the BACK) and `Decode` (preview audio decode + peak extraction, pushed to the FRONT). The main thread is the only SQLite writer (single-writer constraint) and the only job producer: it ingests parsed snapshots as they arrive and, when a project's last `.als` is ingested (per-project pending counter), runs the cheap name-matching (`plan_folder_harvest`) and pushes the resulting decode jobs to the front of the same queue. Two pitfalls this design fixes (both user-observed 2026-06-11): (1) inline `harvest_folder_renders` on the consumer thread blocked the parse channel and parked all parser threads whenever a project had previews; (2) a plain FIFO job channel queued decode jobs behind the entire remaining parse backlog, so previews only populated at the END of a scan — front-of-queue priority makes them appear live. Deadlock-safety invariants: job queue unbounded (producer == consumer, must never block), done channel bounded for backpressure, worker `pop` uses `wait_timeout` so parked workers notice cancellation, cancellation/queue-close drains the done channel so the main loop exits. `known_samples` (sample cross-check) is loaded from DB at scan start and grown incrementally from each ingested snapshot's sample paths.

### 2. Metadata & Indexing Service — `indexer` (Rust + SQLite)
- **Decision**: SQLite with FTS5 (over names) for search.
- **Model**: A project *folder* contains one or more `.als` *sets*. Tables: projects -> sets (tempo, version, hash, mtime) -> tracks, plugins, samples (path + missing flag), previews.
- **Incremental**: Reindex keyed on mtime + content hash. Index lives in app data dir, never inside user project folders.

### 3. Preview Service (pluggable source interface)
- **Pipeline**: watcher sees .als save -> debounced job queue -> preview *source* resolves audio -> peaks cached -> catalog updated.
- **Constraint**: Reimplementing Live's render engine is ruled out permanently. Live itself is the only correct renderer.
- **Sources (priority)**:
  - (a) **Discovery** (MVP): user-exported renders in/near project folder; Live 12 set previews in `Ableton Project Info/` (verify); frozen/processed audio fallback.
  - (b) **Automated Live export** (flagship, post-catalog; queue infra BUILT): `export_jobs` table (schema v3) + worker loop in the Tauri backend (polls every 3s while "Auto-Export" is toggled on, one render at a time) + `tools/export_set.py` UI automation; finished renders are attached as previews (source=worker, confidence=1.0). Sets are queued from the UI per-row, from the detail pane, or in bulk via multi-select (checkboxes, cmd-click toggle, shift-click range; `add_to_export_queue_bulk`). Worker launches a *second* Live install with the set, drives File -> Export via macOS UI automation (proven previously by owner). Constraints: serialize one render at a time; debounce save bursts; handle dialogs (missing samples, version prompts); UI scripting steals focus so make it opt-in/idle-scheduled; treat Live as flaky (timeouts, retry once, mark "render failed" rather than wedging queue). Isolated component — can start as a standalone script consuming jobs and emitting audio files.
- **Previews are per-SET, not per-project** (projects can hold multiple distinct .als, e.g. "wanna be your" + "wanna be your2"). Discovery must match found renders to sets by filename similarity (normalized prefix match vs set name); ambiguous matches attach at project level with low confidence. The export worker has no ambiguity (it knows which set it rendered).
- **Waveforms**: Decode (symphonia), precompute peaks once, cache keyed by set hash.
- **Concurrency**: `hunt_renders` (bulk scan) and standalone `harvest_folder_renders` (the app's per-folder rescan) parallelize audio decoding + peak extraction via `std::thread::scope`. Inside `scan_library`, harvesting is split: `plan_folder_harvest` (cheap matching + DB filter, main thread) emits `DecodeJob`s into the scanner's unified worker pool.

### 4. User Interface — Tauri 2 [skeleton BUILT 2026-06-11]
- **Decision**: Tauri 2 shell, React/TS frontend; core logic lives in the Tauri Rust backend (no sidecar). Audio streamed to webview via asset protocol (when previews land).
- **Implemented**: commands `search`/`inspect`/`stats` (thin wrappers over `indexer`); debounced FTS search, bpm/plugin filters, results table, detail pane. Dev-only config (bundle.active=false, no icons yet).
- **Views**: Library View (Search/Filters) ✓, Set Detail pane ✓; Player pending Milestone 3.

## Data Flow
```
Filesystem (.als + renders) -> unified worker pool (Parse | Decode jobs, all cores)
                            -> main thread (SQLite writes + plan_folder_harvest matching)
                            -> Tauri commands -> React UI
```
Key design: scan + harvest are interleaved per-project AND share one worker pool — a project's decode jobs are queued the moment its last `.als` is ingested, but indexing of later projects continues in parallel. Logs interleave (`indexed -> preview -> indexed`) without lockstep stalls.

## Known Naming Inconsistencies (backlog)
The codebase has grown organically and several naming choices are vague or inconsistent. These should be addressed in a dedicated rename pass:

| Current Name | Problem | Suggested Direction |
|---|---|---|
| `hunt_renders` / `harvest_folder_renders` | Two different verbs ("hunt" / "harvest") for the same concept | Unify: e.g. `scan_previews` / `scan_folder_previews` |
| `RenderFile` / "preview" / "render" | Three terms for one thing (an audio file linked to a set) | Pick one term project-wide |
| `ops` crate | Too generic | Consider `workflows` or `commands` |
| `set_match_candidates` | Ambiguous — returns sets? candidates? | `preview_match_candidates` |
| `matching.rs` | Could be anything | `render_matching.rs` or `name_matching.rs` |
| Tauri: `scan_folder` vs `scan_previews` | Both are "scanning" from the user's POV | Clarify *what* is being scanned |
| `ingest_set` / `upsert_preview` / `recompute_primary` | Indexer functions mix abstraction levels | Group or prefix consistently |

## AI Workspace Substrate
This repository uses an AI-assisted engineering substrate located in `/ai`
- **Cognition Layer**: State and tasks are tracked in `/ai`.
- **Rules**: Agent constraints are defined in `AGENTS.md`.
- **Flow**: Human Pilot -> AI Implementation -> Deterministic Verification.
