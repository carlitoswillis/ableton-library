# AI Context Bundle
Generated: Fri Jun 12 19:07:23 UTC 2026

## ⚠️ Agent Navigation Guide
1. Start with the **Current State** below to understand the focus.
2. Check **Active Tasks** for your specific assignment.
3. Only read files from the repository structure that are directly related to those tasks.
4. Do NOT perform full repository scans unless the task is an architectural audit.

## 1. Authoritative Rules (AGENTS.md)
# Agent Guidelines (AGENTS.md)

PURPOSE: This is the authoritative rulebook for AI assistants. It defines the 'how' and 'what' of the codebase.

## Project Context
- **Objective**: Build a local-first system to browse, search, organize, and preview Ableton projects without opening Ableton Live.
- **Stack (decided 2026-06-11)**: Rust core + Tauri 2 desktop shell + React 18/TS frontend + SQLite (rusqlite + FTS5). CLI-first development: core logic validated via CLI before UI integration.
- **Working style**: User is NOT writing Rust — AI writes all code, user compiles/tests on their Mac and gives product feedback. The sandbox cannot run cargo; ALL Rust verification happens on the user's machine.

## Architecture Constraints
- **No Ableton SDK dependency**: User runs Live 11; the Extensions SDK (Live 12 Suite beta only) is off the table. Filesystem-first is the strategy, not a fallback.
- **Version tolerance (backward + forward)**: Parser must handle .als files from older Live versions (9/10/11) and newer ones (12+). Extract leniently — skip unknown elements, never hard-fail on schema drift, record the Live version (Creator attribute) per set.
- **Crate layering**: `als-core` + `previews` → `indexer` (storage) → `ops` (workflows) → `cli` / `app` (frontends). Never import a frontend crate from a library crate.
- **Database/Persistence**: SQLite in app data dir (`~/Library/Application Support/ableton-library/library.db`). Catalog is always fully rebuildable from `.als` files. Never store DB inside user project folders.
- **Markdown Persistence**: All project state must be tracked in `/ai`.
- **Local First**: Assume local filesystem and no cloud dependencies.
- **Incremental catalog**: Never assume the catalog is complete — user scans subfolders piecemeal. UI and queries treat the catalog as "what's been indexed so far".

## Coding Conventions
- **Explicit over Implicit**: Avoid hidden logic, reflection, or complex inheritance.
- **Verification First**: All changes must be verified via tests and project-specific validation scripts. Keep `tools/reference_extract.py` in sync with any `als-core` parser change.
- **Compact Context**: Keep context files task-scoped and minimal.
- **Async + spawn_blocking**: ALL Tauri commands must be `async`. Any command touching disk/db goes in `spawn_blocking`. (Learned from beach-ball incident — sync commands run on main thread.)
- **Multi-threading pattern**: CPU-bound batch work (`.als` parsing, audio peak extraction) uses `std::thread::scope` with worker threads funneling results to main thread for sequential SQLite writes.
- **Interleave scan + harvest**: When scanning a library, preview harvesting happens per-project immediately after that project's sets are ingested — never as a separate bulk pass. `known_samples` (sample cross-check) is built incrementally, not queried in bulk after commit.
- **Export worker**: Automated Live export uses macOS UI automation (`tools/export_set.py`). Serialize one render at a time; treat Live as flaky (timeouts, retry once, mark failed rather than wedging queue).

## How to Navigate This Workspace (Priority Flow)
To minimize token waste and maximize focus, follow this priority sequence:
1. **START HERE**: Read `PROJECT_STATE.md`. It defines the current high-level objective and active milestones.
2. **Operational Rules**: Read `AGENTS.md` (this file). Adhere strictly to these constraints.
3. **Architecture Details**: Read `ARCHITECTURE.md` to understand the system components and data flow.
4. **Self-Correction**: If you feel your understanding of the project state is out of sync, you may run `./ai/ai-context.sh` to refresh your local context bundle.

## 2. Architecture (ARCHITECTURE.md)
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
  - (c) **FUTURE — headless/remote render via plugin substitution** (backlog, detailed in PROJECT_STATE.md): pre-render sanitize pass swaps third-party AU/VST/VST3 devices on a TEMP COPY of the .als for built-in Suite equivalents with translated parameters, so the set opens clean in a Live install on a VM/spare/remote machine with zero 3rd-party plugins — rendering without touching the user's active computer. Originals never modified; previews labeled approximate with a substitution log. Live remains the renderer (constraint above unchanged). Requires a .als WRITER (today we only read), substitution/param-mapping tables, and a remote worker speaking the export_jobs queue.
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

## 3. Project State (PROJECT_STATE.md)
# Project State

## ⚡ HANDOFF SNAPSHOT (2026-06-11, end of session — read this first)
- **Where things stand**: M1 (extraction) + M2 (catalog) + UI skeleton DONE and verified on the user's Mac. M3 previews: fully built (discovery, matching, peaks, player bar, in-app folder-picker scanning), but **awaiting user verification** of (a) the async/spawn_blocking UI-freeze fix (beach ball occurred on first in-app scan; fix committed 72ae0a1, not yet re-tested), (b) the matcher against real bounces (user's plan: `reset --yes`, bounce current-year tracks to one folder, scan 2026 projects in-app or via CLI, then `previews <bounce folder> --verbose`), and (c) the unified scan/decode work queue (user observed scans stalling on preview decode; fixed, needs rebuild + re-test).
- **Working style**: user is NOT writing Rust (decided after the fact — AI writes all code, user compiles/tests on their Mac and gives product feedback). The sandbox cannot run cargo (network allowlist); ALL Rust verification happens on the user's machine. Keep tools/reference_extract.py in sync with any als-core parser change.
- **Cadence that works**: user gives product feedback/requests -> implement -> commit with descriptive message -> user pulls, builds, tests -> log results + decisions here. Update these context files and commit at every meaningful step (project instruction).
- **Run commands**: CLI `cargo run -p cli -- <subcommand>`; app `cd app && npm install && npm run tauri dev`.
- **Next likely work**: naming consistency pass; detail-pane preview list (switch primary); `roots` table + rescan; iCloud `evicted` sample state.

## Current Focus
Phase: Milestone 3 — Previews (discovery half BUILT, awaiting host verification) (2026-06-11)
- **Key user decision**: renders are SCATTERED across the computer (old consolidation script defunct) — discovery must NOT rely on project folders. It hunts user-chosen roots (Desktop, Downloads, ...) and name-matches against the catalog. Files never moved, only referenced.
- **User direction**: preview GENERATION (export worker) should be an in-app feature eventually ("most people have bad habits too") — discovery is the bridge, worker is the destination. `source` column (discovered|worker|manual) exists for this.
- [x] Schema v2 + real in-place migration (v1 catalogs upgraded, not rebuilt): previews table (set_id nullable for ambiguous project-level matches, confidence, source, peaks JSON, is_primary).
- [x] crates/previews: render hunt (audio exts, >=1MB, skips Samples/Backup/Project Info dirs), normalizer (stopwords/vN/bpm/bracketed chunks), scorer (exact 1.0 > word-boundary prefix 0.85 > token Jaccard; project-name fallback -> single-set x0.9 else project-level x0.5), symphonia peak extraction (<=1500 bins, coarse-then-downsample, JSON).
- [x] CLI: `previews <roots...> [--threshold 0.6] [--verbose]` (freshness-checked, decode only matches) + `attach <set> <audio>` (manual, confidence 1.0). Primary = highest confidence then newest.
- [x] App: `preview` command; asset protocol enabled (scope **, tauri feature protocol-asset — user added the cargo feature); bottom PlayerBar (canvas waveform, click-seek, match-confidence shown when <85%); ▶ on rows with previews.
- **Matcher revision (user feedback)**: bpm/key/"(prod. x)" are often PART of project names (user's old naming habit) — KEEP them as distinguishing signal; normalize form instead ("145bpm" -> "145 bpm"); strip only [bracketed timestamps], stopwords (final/master/...), vN. Tests cover the disambiguation case.
- [x] `reset` subcommand (deletes db + WAL/SHM; dry-run unless --yes).
- [x] Sample-safety (user concern): discovery skips Samples/Backup/Ableton Project Info dirs, hidden files, <1MB files; AND cross-checks the catalog's samples table — a file referenced as a sample by any set is never attached as a preview.
- [x] In-folder harvest (user request): `scan` auto-harvests renders found inside project folders (folder placement = signal): name match -> set (+0.05 bonus); no name match in single-set project -> 0.7; else project-level. `--no-previews` opts out (iCloud).
- [x] **ARCHITECTURE: crates/ops extracted** (user wants in-app scanning; "CLI for dev, app for users"): scan_library/hunt_renders/attach moved out of the cli bin into shared ops crate. Layering: als-core+previews -> indexer (storage) -> ops (workflows) -> cli/app (frontends). CLI commands are now thin wrappers.
- [x] In-app scanning: "Scan folder…" header button -> native picker (tauri-plugin-dialog, dialog:default capability) -> scan_folder command (ops::scan_library incl. harvest) -> stats+results refresh + summary message. NOTE: requires `npm install` (new plugin-dialog dep).
- [x] **Live scan progress via Tauri events**: backend command `scan_folder` accepts `AppHandle` and forwards `ops::scan_library` log lines via `"scan-progress"` events. Frontend listens, parses log types, and presents a beautiful scrolling terminal logs modal with live-updating stat counters. (Fixed duplicates from React 18 Strict Mode double-mount listener races and nested-project recursive harvests. Scan is fully exitable, and minimizable to a background top banner on modal close).
- [x] **Previews normalization fix**: Fixed unit test in the `previews` crate (where `v2` was incorrectly split into `v 2` and thus not filtered out as a version token).
- [x] **GOTCHA (beach-ball incident)**: sync Tauri commands run on the MAIN thread -> scan froze the window. ALL commands now async; scan_folder additionally wraps work in tauri::async_runtime::spawn_blocking. Rule going forward: any command touching disk/db is async; anything heavy goes in spawn_blocking.
- [x] **Multi-threaded scanning + preview processing** (2026-06-11): `scan_library` (`.als` decompression/XML parsing), `hunt_renders` (bulk preview scan), and `harvest_folder_renders` (in-folder preview harvest) all parallelized via `std::thread::scope`. Worker threads parse/decode on all CPU cores, results funnel to main thread for sequential SQLite writes. Expected ~6-8x speedup on multi-core machines.
- [x] **Interleaved scan + harvest** (2026-06-11, user request): previously `scan_library` indexed ALL projects first, then harvested previews for ALL projects in a separate bulk pass. Now harvesting is interleaved — each project's previews are harvested immediately after its `.als` files are fully ingested, so the user sees `indexed → preview → indexed → preview` in the logs instead of `index all → preview all`. Implementation: per-project pending counter tracks remaining `.als` parse tasks; when a project hits zero, `harvest_folder_renders` fires immediately. `known_samples` (the sample cross-check set that prevents attaching a sample as a preview) is built from the DB at scan start and grown incrementally from each ingested snapshot's `SampleRef` paths, so it's always accurate without needing a separate post-commit query. Projects whose sets are all unchanged (fresh) still get harvested in case new renders appeared in the folder since last scan.
- [x] **Unified scan/decode work queue** (2026-06-11, user-spotted bug — AWAITING HOST BUILD + RE-TEST): user observed the scan stalling whenever a project had previews; confirmed in code — `harvest_folder_renders` ran inline on the main thread (the channel consumer), so the parse channel (cap 2×cores) filled and ALL parser threads blocked while one project's audio decoded. Fix: ONE worker pool consuming both `Job::Parse` and `Job::Decode` from an unbounded job queue; main thread only does matching + SQLite writes and pushes decode jobs as projects complete (`plan_folder_harvest` = cheap matching/DB-filter split out of harvest; `harvest_folder_renders` kept as standalone wrapper for the app's per-folder rescan). Scanning never stops for preview decoding; logs still interleave. Deadlock-safety: job queue unbounded (main never blocks enqueueing), done channel bounded (backpressure), cancel closes done channel so main loop exits.
- [x] **Decode jobs jump the queue** (2026-06-11, follow-up user report "previews not populating during scan"): first version of the unified queue was plain FIFO — decode jobs landed BEHIND the entire remaining parse backlog, so previews only appeared at the end of a scan. Replaced the mpsc job channel with `JobQueue` (Mutex<VecDeque> + Condvar): parse jobs push_back, decode jobs push_front → previews decode immediately after their project finishes ingesting. Worker `pop` uses 50ms `wait_timeout` so parked workers notice cancel. NOTE for diagnosis: if previews STILL don't populate live, the cause is matching (no qualifying audio ≥100KB/1MB in the project folder, name match below 0.6, or files skipped as known samples) — run the CLI scan with logs to see `preview (…)` lines.
- [ ] **NEXT (user's test plan)**: dump db (`ableton-scan reset --yes`), bounce some current-year tracks into one folder, `scan` the matching projects + `previews` that folder, evaluate match quality from a controlled sample. NO full-system hunt (user explicitly declined).
- [ ] Later in M3: previews in detail pane — list ALL of a set's previews, switch primary, manual attach/replace from the UI (user asked "what if i want to update the preview?": re-bounce to same path = auto-replaced on rescan via mtime; new file = new row, newest wins at equal confidence; `attach` = manual trump at 1.0 — UI affordance for all this still missing). Also: historical preview archive, in-app "hunt for previews" UI.
- [~] M4 IN PROGRESS (user corrected status 2026-06-12 — do NOT mark complete): mechanism built (export_jobs table schema v3, worker loop polling every 3s when Auto-Export active, tools/export_set.py drives Live, renders attached source=worker conf=1.0) but real-world rendering of OLD projects is the unsolved part. Field findings: iCloud-evicted samples stall/slow bounces; missing plugins (incl. synths) = silent tracks; moved-but-findable samples force Live's slow relocate scan. Renders can be both slow AND missing a ton.
- [ ] M4a (next, catalog-driven, no .als modification): renderability score per set (we already index plugins + samples); worker queue easy-first; pre-flight iCloud download (brctl) of project samples before bounce; fidelity metadata on worker previews ("missing: Serum, soothe2") surfaced in UI.
- [ ] M4b: preview-PROXY sets — write a transformed COPY (never the original): bypass/remove missing effects, relink sample FileRefs to located copies (we can rewrite the XML we already parse).
- [ ] M4c (experimental, user request "replace missing plugins, yes even synths, with closest built-in at medium effort"): instrument stand-ins so MIDI isn't silent. HONEST CONSTRAINT (told to user): third-party plugin state in .als = opaque vendor binary blob; parameter recovery is impossible in general. Closest achievable = category-guess stand-in from track/device names ("hear the notes, not the sound"); effects are better bypassed than badly approximated.
- [x] **Bulk export / multi-select** (2026-06-11, user request): checkbox column + select-bar above results ("Select all" w/ indeterminate state, "Queue N renders", "Clear"); Finder-style row selection — cmd/ctrl-click toggles a row, shift-click extends range from last-clicked anchor (works on checkboxes AND rows; range action follows the clicked row's new state); selection auto-prunes to visible search results so hidden rows are never exported. Backend: `indexer::add_export_jobs_bulk` (single prepared stmt, re-queues pending/failed/completed but NEVER clobbers a `processing` job) + Tauri `add_to_export_queue_bulk(set_ids) -> queued count`. AWAITING HOST BUILD + TEST.
- [x] **Bulk link rewrite** (2026-06-11, user: "Link Selected not fully working, maybe async"): old `link_watch_suggestions` Tauri command did N file moves + N SERIAL audio decodes directly on the async runtime (no spawn_blocking — the beach-ball gotcha again) with zero feedback. Now `ops::link_suggestions`: phase 1 moves files + resolves targets on the calling thread, phase 2 decodes in parallel (thread::scope) with sequential upserts; both Tauri link commands wrapped in spawn_blocking; bulk emits `link-progress (done,total)` events → button shows "Linking N/M…". Bulk link is a full background job like scan_folder/bulk_preview_scan (user request): shares ScanState cancel flag (Cancel button works), logs via scan-progress into the progress modal/banner ("linked …" lines count toward the previews stat), cancellable mid-decode, summary in scanMsg. Single-link surfaces the per-file error message.
- [x] **Watch suggestions: all matches, grouped per set, best auto-selected** (2026-06-11, user request): previously suggestions matched only preview-LESS sets and the list was flat. Now `get_watch_suggestions` matches against the whole catalog with a `has_preview` flag (already-previewed sets stay visible for swapping in better bounces); UI groups candidates under a per-set header row (badge "has preview", match count), preview-less groups sort first; on refresh the single best candidate per preview-less set is auto-checked. has_preview is judged at the PROJECT level (user decision): ANY preview on the set, a sibling set, or the project itself blocks auto-select — replacements are always explicit.
- [x] **Suggestions rework round 2** (2026-06-11, user feedback): (a) groups now show the set's CURRENT primary preview ("current: file.wav" + ▶ play + × Unlink) so linking decisions can be reconsidered later — previously a linked bounce vanished from the scan area (the file moves into the project folder, out of watch-folder discovery); (b) **DECISION (flag to user if wrong)**: `indexer::remove_preview` no longer DELETES the audio file from disk — it was calling fs::remove_file on the bounce! Now DB-row-only + recompute_primary, per the "files are referenced, not owned" principle; (c) **case-insensitive path identity**: COLLATE NOCASE on preview-freshness + ignored-matches path compares; known_samples keys lowercased everywhere (macOS FS is case-insensitive, casing drifts between scans). Name matching was already case-insensitive (normalize() lowercases) as was extension detection; (d) detail pane gained "Scan Folder for Previews" (new `scan_set_folder_previews` Tauri command → harvest_folder_renders on that project's folder, spawn_blocking).
- [x] **Suggestions source B: project folders** (2026-06-11, user request "the original scan's indexed files for previews should automatically be results in the watch scan"): `get_watch_suggestions` now scans BOTH watch folders (global catalog matching) AND every indexed project's folder (local matching against that project's own sets, harvest rules: +0.05 folder bonus, single-set 0.7 fallback, project-level matches skipped as non-actionable). Shared `push_suggestion` helper validates (ignored, already-attached COLLATE NOCASE, sample guard) + dedupes across sources via (set_id, lowercased path). Files auto-attached by the scan harvest are filtered (already previews) — what surfaces is what harvest SKIPPED (non-winning duplicates, below-threshold-at-scan-time, new files). Watch folders may now be empty; suggestions still work from project folders alone. Select-all semantics unchanged (best per preview-less set only). NOTE: refresh now walks all project folders — manual refresh acceptable; revisit if startup refresh gets slow on big catalogs.
- [x] **GOTCHA (StrictMode, 2nd incident)**: never put side effects (ref mutation) inside a `setState` updater — StrictMode double-invokes them; the shift-range anchor moved on the first invocation so the second degraded to a single toggle. Also: `user-select` CSS needs the `-webkit-` prefix in Tauri's WKWebView.

## UI skeleton (Tauri): ✅ DONE + verified (2026-06-11)
- [x] `indexer` refactor: `set_detail`/`resolve_set` moved into lib (shared CLI + app); Serialize on SearchHit/Stats.
- [x] `app/src-tauri`: Tauri 2 backend, commands `search`/`inspect`/`stats` (snake_case args) over the shared catalog; bundle inactive (dev-only, no icons needed yet); workspace member.
- [x] `app/` frontend: React 18 + Vite + TS; debounced search, bpm/plugin filters, results table, detail pane (tracks/devices/samples/locators chips), partial-catalog empty state, dark theme.
- [x] **VERIFIED on user's Mac** (2026-06-11): app runs after icon fix ("looks great"). Note: Tauri opens its own window; localhost:1420 in a browser has no `invoke` (expected).
- [x] Search ranking: weighted bm25 (set/project names 10/8, tracks 4, devices 1, samples 0.5) so plugin/sample hits rank below name hits (user feedback).
- [x] "Open in Live" (rows, hover-revealed) + "Reveal in Finder" (detail pane): `open_set` command -> macOS `open` on the stored als_path (catalog paths only, existence-checked; macOS-only cfg).
- [ ] Then (Milestone 3): previews table + discovery -> waveform peaks -> player in detail pane; later the automated Live export worker.

## Milestone 2 — Project Catalog (indexer): ✅ DONE (2026-06-11)
- [x] Implement `indexer` crate: SQLite (rusqlite bundled) + FTS5; schema projects -> sets -> tracks/devices/samples/locators/backups; incremental via (file_size, mtime) freshness check; prune removed sets.
- [x] CLI subcommands: `json` (oracle-compatible dump), `scan`, `search` (FTS + --min-bpm/--max-bpm/--plugin), `inspect`, `stats`.
- [x] Index location: dirs::data_dir()/ableton-library/library.db (macOS: ~/Library/Application Support/...), `--db` override.
- [x] Discovery moved to als-core::scan (shared with future Tauri app).
- [x] `scan --force` (full re-ingest, e.g. after parser upgrades) + db stamped with PRAGMA user_version (SCHEMA_VERSION=1); mismatched dbs refused with rebuild instructions. Catalog is always fully rebuildable from .als files.
- [x] **VERIFIED on user's Mac** (2026-06-11): build clean, oracle diff clean, scan/search working ("everything went great").
- [ ] previews table (schema exists conceptually; add when preview discovery lands — Milestone 3).

### Library indexing strategy: INCREMENTAL ADOPTION (decided 2026-06-11)
- User's full library is extensive + iCloud-hosted; a full first scan would force mass downloads (eviction) and take very long. **Full-library scan is deliberately deferred — do not push for it.**
- Instead: scan subfolders piecemeal (per year / per artist) as needed. This is SAFE BY DESIGN: `prune_missing` is root-scoped (only prunes sets under the root being scanned), so scans of different roots **accumulate** in one catalog without clobbering each other.
- Implication for all future features: never assume the catalog is complete. UI and queries must treat the catalog as "what's been indexed so far".
- Possible future ergonomics (backlog): `roots` table remembering scanned roots -> `ableton-scan rescan` refreshes all known roots; per-root scan timestamps.

## Milestone 1 — Metadata Extraction: ✅ DONE (2026-06-11)
- [x] Cargo workspace: crates/als-core (parser lib), crates/cli (binary `ableton-scan` — defined in crates/cli/Cargo.toml [[bin]]).
- [x] `als-core`: streaming gzip+XML -> SetSnapshot; lenient, version-tolerant, skips bulk subtrees.
- [x] **VERIFIED on host Mac**: `cargo build` clean; `ableton-scan` output diffs CLEAN against `tools/reference_extract.py` (the executable spec / test oracle) on all 4 fixture projects / 5 sets, 0 warnings.
- Note: sandbox cannot install Rust toolchain (network allowlist); all Rust verification happens on the user's Mac. Keep oracle in sync with any parser change.

### Real-library validation (2026-06-11, user's iCloud library)
- 2021 folder (nested year/month structure): 85 projects, 129 sets, 811 backups — no errors.
- Version tolerance proven: Live 10.1.30 -> 11.3.43 (incl. betas), 0 warnings on all native Live sets.
- Only warnings: 3 sets exported by **KORG Gadget** (also writes .als; no Tempo/Manual element) — degraded gracefully (tempo null + warning).
- Caveat found: `exists` check can't distinguish iCloud-evicted placeholders from deleted samples -> backlog: third state `evicted` (detect `.icloud` placeholder files).
- iCloud syncing noticeably slows scans (eviction-triggered downloads).

## Current Assumptions & Validations
- **Assumption A**: Ableton Extensions SDK can read Live Set metadata. -> **REJECTED** (Live 12 Suite Beta only; user is on Live 11). SDK is permanently off the table — filesystem-first is the strategy, not a fallback.
- **Assumption B**: Ableton Extensions SDK can identify tracks and clips. -> **MOOT** (SDK ruled out per Assumption A).
- **Assumption C**: Automated preview generation may be possible. -> **VALIDATED in principle**: owner previously scripted a second Live install to open + export sets via macOS UI automation. Previews = pluggable source interface: discovery (MVP) -> automated Live export worker (post-catalog). **Unverified** whether Live 12 desktop writes preview audio on save.
- **Constraint**: Parser must be version-tolerant across Live versions, backward (9/10/11) and forward (12+). Lenient extraction; never hard-fail on schema drift.

## Format Findings (2026-06-11, from /example-project-library — Live 11.3.43)
- Root: `<Ableton Creator="Ableton Live 11.3.43" MinorVersion="11.0_11300">` -> version branching trivial.
- Tracks: typed elements (MidiTrack/AudioTrack/ReturnTrack/Group), names in `EffectiveName` — but `EffectiveName` also exists on devices, so names MUST be scoped by parent path (validates streaming parser w/ path stack).
- Tempo: `<Tempo><Manual Value="...">` under MasterTrack (location differs in older versions — verify when older fixture available).
- Master time signature: `<TimeSignature><Manual Value="N">` where N = 99*log2(denominator) + (numerator-1) (e.g. 201 -> 4/4). Clip-level sigs use RemoteableTimeSignature/Numerator+Denominator.
- Plugins: `AuPluginInfo`/`VstPluginInfo`/`Vst3PluginInfo` with `Name` + `Manufacturer`; native devices are bare element names. Plugin inventory ~free.
- Samples: absolute paths in `<Path>` under `FileRef`, reference files across projects/Downloads/iCloud -> missing-sample detection + cross-project sample queries high-value.
- Noise: thousands of AutomationTarget/PluginFloatParameter elements = most of file size -> parser must skip these subtrees (392KB .als -> 8.1MB XML).
- `Backup/` folder per project = free timestamped lineage; multi-set projects confirmed (wanna be your + wanna be your2).
- ~~Gap: all fixtures are 11.3.43~~ -> CLOSED: real-library scan validated Live 10.1.30-11.3.43. Remaining untested: Live 9 and 12+.

## Active Milestones
- **Milestone 1: Metadata Extraction**: Generate structured output from .als files (Gzip/XML parsing).
- **Milestone 2: Project Catalog**: Browse, search, and sort projects locally.
- **Milestone 3: Preview Integration**: Display metadata, waveform, and audio preview.

## Decisions
- **Backups**: lineage-only indexing (filename, timestamp, size); full parse behind a `--deep` flag later. (2026-06-11)
- **Snapshot schema**: SetSnapshot/ProjectSnapshot as defined in als-core (version, tempo, time sig, tracks, devices, samples, locators, warnings). Approved 2026-06-11.
- **Repo conventions**: scan JSON outputs go in `exports/` (gitignored); lockfiles (Cargo.lock, app/package-lock.json) ARE tracked (user flipped to binary-project convention); local *.db files gitignored (catalog = rebuildable cache, lives in app data dir). (2026-06-11)

## Backlog
- [x] Automated Live export worker (second Live install + UI automation; see ARCHITECTURE.md Preview Service) [cmd + a to select all in arragement view, cmd + r then click export [or hit enter], but if there is no arrangement (usually there is so this is rare) like if nothing is there, or we are playing from session view then export some or all of the session view scences/rows]
- [x] Handle overwrite/replace dialogs gracefully during automated render queue exports (2026-06-11, user about to run overnight queue): two layers — (1) worker pre-flight: if `<stem>.wav` already exists next to the .als AND wav mtime >= als mtime, skip Live entirely and attach the existing file (fresh-render short-circuit; stale files still re-render); (2) export_set.py confirms "Replace?" dialogs after filename entry (guarded button-"Replace" clicks on sheet variants + AXDialog return fallback — NO blind extra Return, that could toggle playback). Pre-delete of the target stays as the primary path; dialogs were wedging when iCloud-locked files failed to delete, poisoning every later job in the queue.
- [ ] **HEADLESS/REMOTE RENDERING via plugin substitution** (user request 2026-06-11 — FUTURE, do NOT start without explicit go-ahead; user: "we cant work on it" yet). Goal: render previews WITHOUT stealing the reins of the user's current computer — headless, in a container, in a VM, on a spare/remote machine; location doesn't matter, non-interference does. The core blocker for headless render is third-party plugins (AU/VST/VST3: machine-bound licenses, iLok, GUIs, missing-plugin dialogs). The proposal:
  - **Pre-render sanitize pass**: operate on a TEMP COPY of the .als — NEVER the original, nothing is removed or lost from the user's project. Parse the set XML (we already stream-parse .als; writing = re-gzip modified XML, new capability, low risk on copies), find every non-built-in device (`AuPluginInfo`/`VstPluginInfo`/`Vst3PluginInfo` — native Live devices are bare element names, parser already distinguishes), and REPLACE each with the closest built-in Suite device with settings matched as closely as possible.
  - **Mapping engine**: substitution tables per plugin family with parameter translation (e.g. 3rd-party parametric EQ → EQ Eight w/ translated bands/freqs/gains; 3rd-party comp → Compressor/Glue w/ threshold/ratio/attack/release mapped; reverbs → Hybrid Reverb; delays → Delay/Echo; saturators → Saturator). Per-substitution confidence score + per-render substitution LOG so the preview is honestly labeled approximate. Unmappable params get sane defaults.
  - **Hard case — instruments/synths**: effect substitution is tractable; replacing a 3rd-party SYNTH is not (sound IS the preset). Options to evaluate: skip/mute those tracks (and log), freeze/flatten on the user's machine ONCE as a cheap prerequisite step, or render-with-silence + warn. Do not pretend a substituted synth is the track.
  - **Execution environments to evaluate**: Live has NO official headless mode — it still renders via the real Live (ARCHITECTURE constraint stands: Live is the only correct renderer; we never reimplement it). Candidates: a macOS VM (licensing of Live + macOS EULA to check), a spare/second machine driven over the network by the same job queue (export_jobs is already serializable — worker could be a remote daemon), virtual display so UI automation doesn't touch the real desktop. Suite-only device usage means the sanitized set opens clean with zero 3rd-party installs on the render box.
  - **Why this fits the architecture**: sanitized copy → existing export_jobs queue → render (wherever) → attach as preview source=worker w/ an `approximate`/substituted flag (consider previews schema addition) + substitution log stored for display in detail pane.
  - **Prereqs already in place**: per-set device/plugin inventory (devices table), export job queue + worker loop, version-tolerant parser. **New work needed**: .als writer (gzip+XML emit), substitution mapping tables, param translation, remote worker protocol, UI labeling of approximate previews.
  - **Risks**: preview diverges from true mix (must be labeled, never silently); XML write corrupting a set (mitigate: copies only, never touch originals, checksum original); device param schemas drift across Live versions; license/EULA constraints for VM/remote Live installs.
- [ ] **Naming consistency pass**: Many internal names are vague or inconsistent. Candidates:
  - `hunt_renders` / `harvest_folder_renders` — "hunt" vs "harvest" for the same concept (matching audio files to sets). Consider unifying to one verb (e.g. `scan_previews` / `scan_folder_previews`, or `discover_*`).
  - `previews` crate — does render discovery + name matching + peak extraction. Could be split or at least have its modules named more clearly (e.g. `matching.rs` → `name_matching.rs` or `render_matching.rs`).
  - `ops` crate — generic name; consider `workflows` or `commands`.
  - `ingest_set` / `upsert_preview` / `recompute_primary` — indexer functions mix abstraction levels; some are CRUD, some are workflow. Consider grouping or prefixing.
  - `set_match_candidates` — unclear whether this returns sets or candidates for matching. Consider `preview_match_candidates`.
  - `RenderFile` vs "preview" vs "render" — the codebase uses all three terms for the same concept (an audio file associated with a set). Pick one and be consistent.
  - Tauri command names: `scan_folder` (scans projects), `scan_previews` (scans for audio matches) — from the user's perspective both are "scanning". Consider renaming to clarify what's being scanned.
  - Frontend: verify component/function names align with the backend terminology once it's cleaned up.
- [ ] Preview archive: keep historical previews per set, potentially anchored to Backup/ timestamps (stretch; pairs with --deep backup parsing)
- [ ] Sample `evicted` state: detect iCloud `.icloud` placeholders vs truly missing files
- [ ] `roots` table + `rescan` subcommand (refresh all previously scanned roots)
- [ ] UI polish pass (user verdict on skeleton: "looks great, a little bland but functional")
- [ ] Search: consider match-source indicator in results (why did this set match?) and column-scoped queries (e.g. plugin:soothe)
- [ ] Automatic key detection
- [ ] Similar project search
- [ ] Plugin inventory
- [ ] Track fingerprints

## Risks
- ~~SDK limitations~~ RETIRED: filesystem-first proven end-to-end.
- ~~Parsing complexity~~ RETIRED: parser validated on ~136 real sets across Live 10.1-11.3.
- iCloud eviction: slows scans, corrupts `exists` signal (backlog: `evicted` state).
- Scope creep (Mitigation: No AI features until catalog exists).

## 4. Repository Structure
```text
.
./Cargo.toml
./tools
./tools/reference_extract.py
./tools/export_set.py
./crates
./crates/als-core
./crates/cli
./crates/ops
./crates/previews
./crates/indexer
./app
./app/index.html
./app/dist
./app/node_modules
./app/package-lock.json
./app/package.json
./app/src-tauri
./app/tsconfig.json
./app/vite.config.ts
./app/src
./example-project-library
./example-project-library/big guy Project
./example-project-library/522 idea Project
./example-project-library/king st Project
./example-project-library/wanna be your Project
./target
./target/CACHEDIR.TAG
./target/debug
./Cargo.lock
./README.md
./exports
./exports/2021_with_month_folders.json
./exports/py.json
./exports/leeroy veto.json
./exports/rust.json
./ai
./ai/ai-context.sh
./ai/ARCHITECTURE.md
./ai/archive
./ai/CONTEXT_BUNDLE.md
./ai/PROJECT_STATE.md
./ai/HUMAN.md
./ai/AGENTS.md
```

## 5. Recent Git Changes (Summary)
```text
116121c fix: render queue never wedges on existing files (overnight-run hardening)
9128911 feat: project-folder renders surface in the suggestions scan
08bde30 fix: suggestions select-all never picks previewed projects
669f476 fix: build error — rusqlite::params! in app crate (no rusqlite dep)
3689a5f feat: reconsiderable links, case-insensitive paths, per-project preview scan
```

## 6. Active Diff
```diff
diff --git a/ai/CONTEXT_BUNDLE.md b/ai/CONTEXT_BUNDLE.md
index 6e22634..2d25784 100644
--- a/ai/CONTEXT_BUNDLE.md
+++ b/ai/CONTEXT_BUNDLE.md
@@ -1,5 +1,5 @@
 # AI Context Bundle
-Generated: Fri Jun 12 00:51:42 UTC 2026
+Generated: Fri Jun 12 19:07:23 UTC 2026
 
 ## ⚠️ Agent Navigation Guide
 1. Start with the **Current State** below to understand the focus.
@@ -14,26 +14,26 @@ PURPOSE: This is the authoritative rulebook for AI assistants. It defines the 'h
 
 ## Project Context
 - **Objective**: Build a local-first system to browse, search, organize, and preview Ableton projects without opening Ableton Live.
-- **Implementation Strategy**: Technology agnostic. Focus on portable, local-first solutions.
-- **Potential Stacks**:
-  - **Backend**: Node.js, Python (FastAPI), Go, or Rust.
-  - **Storage**: SQLite, DuckDB, or JSON/Flat-file.
-  - **Frontend**: React, Vue, or Desktop Native (Electron, Tauri).
+- **Stack (decided 2026-06-11)**: Rust core + Tauri 2 desktop shell + React 18/TS frontend + SQLite (rusqlite + FTS5). CLI-first development: core logic validated via CLI before UI integration.
+- **Working style**: User is NOT writing Rust — AI writes all code, user compiles/tests on their Mac and gives product feedback. The sandbox cannot run cargo; ALL Rust verification happens on the user's machine.
 
 ## Architecture Constraints
 - **No Ableton SDK dependency**: User runs Live 11; the Extensions SDK (Live 12 Suite beta only) is off the table. Filesystem-first is the strategy, not a fallback.
 - **Version tolerance (backward + forward)**: Parser must handle .als files from older Live versions (9/10/11) and newer ones (12+). Extract leniently — skip unknown elements, never hard-fail on schema drift, record the Live version (Creator attribute) per set.
-- **API/Service Structure**: Modular service for metadata and preview management.
-- **Database/Persistence**: Local persistence for indexing and snapshots.
-- **Markdown Persistence**: All state must be tracked in `/ai`.
+- **Crate layering**: `als-core` + `previews` → `indexer` (storage) → `ops` (workflows) → `cli` / `app` (frontends). Never import a frontend crate from a library crate.
+- **Database/Persistence**: SQLite in app data dir (`~/Library/Application Support/ableton-library/library.db`). Catalog is always fully rebuildable from `.als` files. Never store DB inside user project folders.
+- **Markdown Persistence**: All project state must be tracked in `/ai`.
 - **Local First**: Assume local filesystem and no cloud dependencies.
+- **Incremental catalog**: Never assume the catalog is complete — user scans subfolders piecemeal. UI and queries treat the catalog as "what's been indexed so far".
 
 ## Coding Conventions
 - **Explicit over Implicit**: Avoid hidden logic, reflection, or complex inheritance.
-- **Verification First**: All changes must be verified via tests and project-specific validation scripts.
+- **Verification First**: All changes must be verified via tests and project-specific validation scripts. Keep `tools/reference_extract.py` in sync with any `als-core` parser change.
 - **Compact Context**: Keep context files task-scoped and minimal.
-- **Verify Before Building**: Never assume SDK capabilities; verify and document findings first.
-- **Catalog First**: Prioritize metadata cataloging over audio preview generation or AI features.
+- **Async + spawn_blocking**: ALL Tauri commands must be `async`. Any command touching disk/db goes in `spawn_blocking`. (Learned from beach-ball incident — sync commands run on main thread.)
+- **Multi-threading pattern**: CPU-bound batch work (`.als` parsing, audio peak extraction) uses `std::thread::scope` with worker threads funneling results to main thread for sequential SQLite writes.
+- **Interleave scan + harvest**: When scanning a library, preview harvesting happens per-project immediately after that project's sets are ingested — never as a separate bulk pass. `known_samples` (sample cross-check) is built incrementally, not queried in bulk after commit.
+- **Export worker**: Automated Live export uses macOS UI automation (`tools/export_set.py`). Serialize one render at a time; treat Live as flaky (timeouts, retry once, mark failed rather than wedging queue).
 
 ## How to Navigate This Workspace (Priority Flow)
 To minimize token waste and maximize focus, follow this priority sequence:
@@ -58,7 +58,7 @@ Ableton Library is a metadata and preview indexing system for Ableton projects,
 crates/als-core/   # lib: gzip (flate2) + streaming XML (quick-xml) -> SetSnapshot; discovery  [BUILT, verified]
 crates/previews/   # lib: render discovery, name matching, symphonia peaks  [BUILT]
 crates/indexer/    # lib: SQLite (rusqlite + FTS5) storage; pure, no workflow logic  [BUILT, verified]
-crates/ops/        # lib: workflows (scan_library, hunt_renders, attach) shared by cli + app  [BUILT]
+crates/ops/        # lib: workflows (scan_library, hunt_renders, attach) shared by cli + app; multi-threaded  [BUILT]
 crates/cli/        # bin: `ableton-scan` — thin wrappers over ops/indexer  [BUILT, verified]
 tools/reference_extract.py  # executable spec / test oracle for als-core; keep in sync
 app/               # Tauri 2 + React/TS  [BUILT, awaiting first run]; later: symphonia for waveform peaks
@@ -72,6 +72,7 @@ app/               # Tauri 2 + React/TS  [BUILT, awaiting first run]; later: sym
 - **Version tolerance**: No Ableton SDK (user on Live 11; SDK is Live 12 Suite beta only). Parse leniently across Live versions, backward (9/10/11) and forward (12+): ignore unknown elements, tolerate missing ones, record Creator/version per set, and emit per-field extraction warnings instead of failing the whole file.
 - **Extracts**: Live version, tempo/time sig, tracks (type/name/color), clip names, device/plugin names, sample file references.
 - **Output**: Normalized ProjectSnapshot JSON per set.
+- **Concurrency**: `scan_library` in the `ops` crate runs ONE worker pool (all CPU cores, `std::thread::scope`) consuming a **unified two-priority job deque** (`JobQueue`: `Mutex<VecDeque>` + `Condvar`) of two job kinds: `Parse` (.als decompression + XML parsing, pushed to the BACK) and `Decode` (preview audio decode + peak extraction, pushed to the FRONT). The main thread is the only SQLite writer (single-writer constraint) and the only job producer: it ingests parsed snapshots as they arrive and, when a project's last `.als` is ingested (per-project pending counter), runs the cheap name-matching (`plan_folder_harvest`) and pushes the resulting decode jobs to the front of the same queue. Two pitfalls this design fixes (both user-observed 2026-06-11): (1) inline `harvest_folder_renders` on the consumer thread blocked the parse channel and parked all parser threads whenever a project had previews; (2) a plain FIFO job channel queued decode jobs behind the entire remaining parse backlog, so previews only populated at the END of a scan — front-of-queue priority makes them appear live. Deadlock-safety invariants: job queue unbounded (producer == consumer, must never block), done channel bounded for backpressure, worker `pop` uses `wait_timeout` so parked workers notice cancellation, cancellation/queue-close drains the done channel so the main loop exits. `known_samples` (sample cross-check) is loaded from DB at scan start and grown incrementally from each ingested snapshot's sample paths.
 
 ### 2. Metadata & Indexing Service — `indexer` (Rust + SQLite)
 - **Decision**: SQLite with FTS5 (over names) for search.
@@ -83,9 +84,11 @@ app/               # Tauri 2 + React/TS  [BUILT, awaiting first run]; later: sym
 - **Constraint**: Reimplementing Live's render engine is ruled out permanently. Live itself is the only correct renderer.
 - **Sources (priority)**:
   - (a) **Discovery** (MVP): user-exported renders in/near project folder; Live 12 set previews in `Ableton Project Info/` (verify); frozen/processed audio fallback.
-  - (b) **Automated Live export** (flagship, post-catalog): worker launches a *second* Live install with the set, drives File -> Export via macOS UI automation (proven previously by owner). Constraints: serialize one render at a time; debounce save bursts; handle dialogs (missing samples, version prompts); UI scripting steals focus so make it opt-in/idle-scheduled; treat Live as flaky (timeouts, retry once, mark "render failed" rather than wedging queue). Isolated component — can start as a standalone script consuming jobs and emitting audio files.
+  - (b) **Automated Live export** (flagship, post-catalog; queue infra BUILT): `export_jobs` table (schema v3) + worker loop in the Tauri backend (polls every 3s while "Auto-Export" is toggled on, one render at a time) + `tools/export_set.py` UI automation; finished renders are attached as previews (source=worker, confidence=1.0). Sets are queued from the UI per-row, from the detail pane, or in bulk via multi-select (checkboxes, cmd-click toggle, shift-click range; `add_to_export_queue_bulk`). Worker launches a *second* Live install with the set, drives File -> Export via macOS UI automation (proven previously by owner). Constraints: serialize one render at a time; debounce save bursts; handle dialogs (missing samples, version prompts); UI scripting steals focus so make it opt-in/idle-scheduled; treat Live as flaky (timeouts, retry once, mark "render failed" rather than wedging queue). Isolated component — can start as a standalone script consuming jobs and emitting audio files.
 - **Previews are per-SET, not per-project** (projects can hold multiple distinct .als, e.g. "wanna be your" + "wanna be your2"). Discovery must match found renders to sets by filename similarity (normalized prefix match vs set name); ambiguous matches attach at project level with low confidence. The export worker has no ambiguity (it knows which set it rendered).
+  - (c) **FUTURE — headless/remote render via plugin substitution** (backlog, detailed in PROJECT_STATE.md): pre-render sanitize pass swaps third-party AU/VST/VST3 devices on a TEMP COPY of the .als for built-in Suite equivalents with translated parameters, so the set opens clean in a Live install on a VM/spare/remote machine with zero 3rd-party plugins — rendering without touching the user's active computer. Originals never modified; previews labeled approximate with a substitution log. Live remains the renderer (constraint above unchanged). Requires a .als WRITER (today we only read), substitution/param-mapping tables, and a remote worker speaking the export_jobs queue.
 - **Waveforms**: Decode (symphonia), precompute peaks once, cache keyed by set hash.
+- **Concurrency**: `hunt_renders` (bulk scan) and standalone `harvest_folder_renders` (the app's per-folder rescan) parallelize audio decoding + peak extraction via `std::thread::scope`. Inside `scan_library`, harvesting is split: `plan_folder_harvest` (cheap matching + DB filter, main thread) emits `DecodeJob`s into the scanner's unified worker pool.
 
 ### 4. User Interface — Tauri 2 [skeleton BUILT 2026-06-11]
 - **Decision**: Tauri 2 shell, React/TS frontend; core logic lives in the Tauri Rust backend (no sidecar). Audio streamed to webview via asset protocol (when previews land).
@@ -93,7 +96,25 @@ app/               # Tauri 2 + React/TS  [BUILT, awaiting first run]; later: sym
 - **Views**: Library View (Search/Filters) ✓, Set Detail pane ✓; Player pending Milestone 3.
 
 ## Data Flow
-Filesystem (.als) -> als-core (streaming parse) -> indexer (SQLite) -> Tauri commands -> React UI
+```
+Filesystem (.als + renders) -> unified worker pool (Parse | Decode jobs, all cores)
+                            -> main thread (SQLite writes + plan_folder_harvest matching)
+                            -> Tauri commands -> React UI
+```
+Key design: scan + harvest are interleaved per-project AND share one worker pool — a project's decode jobs are queued the moment its last `.als` is ingested, but indexing of later projects continues in parallel. Logs interleave (`indexed -> preview -> indexed`) without lockstep stalls.
+
+## Known Naming Inconsistencies (backlog)
+The codebase has grown organically and several naming choices are vague or inconsistent. These should be addressed in a dedicated rename pass:
+
+| Current Name | Problem | Suggested Direction |
+|---|---|---|
+| `hunt_renders` / `harvest_folder_renders` | Two different verbs ("hunt" / "harvest") for the same concept | Unify: e.g. `scan_previews` / `scan_folder_previews` |
+| `RenderFile` / "preview" / "render" | Three terms for one thing (an audio file linked to a set) | Pick one term project-wide |
+| `ops` crate | Too generic | Consider `workflows` or `commands` |
+| `set_match_candidates` | Ambiguous — returns sets? candidates? | `preview_match_candidates` |
```
