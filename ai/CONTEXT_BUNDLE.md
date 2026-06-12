# AI Context Bundle
Generated: Fri Jun 12 00:49:27 UTC 2026

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
- **Implementation Strategy**: Technology agnostic. Focus on portable, local-first solutions.
- **Potential Stacks**:
  - **Backend**: Node.js, Python (FastAPI), Go, or Rust.
  - **Storage**: SQLite, DuckDB, or JSON/Flat-file.
  - **Frontend**: React, Vue, or Desktop Native (Electron, Tauri).

## Architecture Constraints
- **No Ableton SDK dependency**: User runs Live 11; the Extensions SDK (Live 12 Suite beta only) is off the table. Filesystem-first is the strategy, not a fallback.
- **Version tolerance (backward + forward)**: Parser must handle .als files from older Live versions (9/10/11) and newer ones (12+). Extract leniently — skip unknown elements, never hard-fail on schema drift, record the Live version (Creator attribute) per set.
- **API/Service Structure**: Modular service for metadata and preview management.
- **Database/Persistence**: Local persistence for indexing and snapshots.
- **Markdown Persistence**: All state must be tracked in `/ai`.
- **Local First**: Assume local filesystem and no cloud dependencies.

## Coding Conventions
- **Explicit over Implicit**: Avoid hidden logic, reflection, or complex inheritance.
- **Verification First**: All changes must be verified via tests and project-specific validation scripts.
- **Compact Context**: Keep context files task-scoped and minimal.
- **Verify Before Building**: Never assume SDK capabilities; verify and document findings first.
- **Catalog First**: Prioritize metadata cataloging over audio preview generation or AI features.

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
crates/ops/        # lib: workflows (scan_library, hunt_renders, attach) shared by cli + app  [BUILT]
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

### 2. Metadata & Indexing Service — `indexer` (Rust + SQLite)
- **Decision**: SQLite with FTS5 (over names) for search.
- **Model**: A project *folder* contains one or more `.als` *sets*. Tables: projects -> sets (tempo, version, hash, mtime) -> tracks, plugins, samples (path + missing flag), previews.
- **Incremental**: Reindex keyed on mtime + content hash. Index lives in app data dir, never inside user project folders.

### 3. Preview Service (pluggable source interface)
- **Pipeline**: watcher sees .als save -> debounced job queue -> preview *source* resolves audio -> peaks cached -> catalog updated.
- **Constraint**: Reimplementing Live's render engine is ruled out permanently. Live itself is the only correct renderer.
- **Sources (priority)**:
  - (a) **Discovery** (MVP): user-exported renders in/near project folder; Live 12 set previews in `Ableton Project Info/` (verify); frozen/processed audio fallback.
  - (b) **Automated Live export** (flagship, post-catalog): worker launches a *second* Live install with the set, drives File -> Export via macOS UI automation (proven previously by owner). Constraints: serialize one render at a time; debounce save bursts; handle dialogs (missing samples, version prompts); UI scripting steals focus so make it opt-in/idle-scheduled; treat Live as flaky (timeouts, retry once, mark "render failed" rather than wedging queue). Isolated component — can start as a standalone script consuming jobs and emitting audio files.
- **Previews are per-SET, not per-project** (projects can hold multiple distinct .als, e.g. "wanna be your" + "wanna be your2"). Discovery must match found renders to sets by filename similarity (normalized prefix match vs set name); ambiguous matches attach at project level with low confidence. The export worker has no ambiguity (it knows which set it rendered).
- **Waveforms**: Decode (symphonia), precompute peaks once, cache keyed by set hash.

### 4. User Interface — Tauri 2 [skeleton BUILT 2026-06-11]
- **Decision**: Tauri 2 shell, React/TS frontend; core logic lives in the Tauri Rust backend (no sidecar). Audio streamed to webview via asset protocol (when previews land).
- **Implemented**: commands `search`/`inspect`/`stats` (thin wrappers over `indexer`); debounced FTS search, bpm/plugin filters, results table, detail pane. Dev-only config (bundle.active=false, no icons yet).
- **Views**: Library View (Search/Filters) ✓, Set Detail pane ✓; Player pending Milestone 3.

## Data Flow
Filesystem (.als) -> als-core (streaming parse) -> indexer (SQLite) -> Tauri commands -> React UI

## AI Workspace Substrate
This repository uses an AI-assisted engineering substrate located in `/ai`
- **Cognition Layer**: State and tasks are tracked in `/ai`.
- **Rules**: Agent constraints are defined in `AGENTS.md`.
- **Flow**: Human Pilot -> AI Implementation -> Deterministic Verification.

## 3. Project State (PROJECT_STATE.md)
# Project State

## ⚡ HANDOFF SNAPSHOT (2026-06-11, end of session — read this first)
- **Where things stand**: M1 (extraction) + M2 (catalog) + UI skeleton DONE and verified on the user's Mac. M3 previews: fully built (discovery, matching, peaks, player bar, in-app folder-picker scanning), but **awaiting user verification** of (a) the async/spawn_blocking UI-freeze fix (beach ball occurred on first in-app scan; fix committed 72ae0a1, not yet re-tested) and (b) the matcher against real bounces (user's plan: `reset --yes`, bounce current-year tracks to one folder, scan 2026 projects in-app or via CLI, then `previews <bounce folder> --verbose`).
- **Working style**: user is NOT writing Rust (decided after the fact — AI writes all code, user compiles/tests on their Mac and gives product feedback). The sandbox cannot run cargo (network allowlist); ALL Rust verification happens on the user's machine. Keep tools/reference_extract.py in sync with any als-core parser change.
- **Cadence that works**: user gives product feedback/requests -> implement -> commit with descriptive message -> user pulls, builds, tests -> log results + decisions here. Update these context files and commit at every meaningful step (project instruction).
- **Run commands**: CLI `cargo run -p cli -- <subcommand>`; app `cd app && npm install && npm run tauri dev`.
- **Next likely work**: live scan progress via Tauri events; detail-pane preview list (switch primary); `roots` table + rescan; iCloud `evicted` sample state; M4 in-app export worker (the flagship: drives a second Live install via UI automation to render previews overnight).

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
- [x] In-folder harvest (user request): `scan` auto-harvests renders found inside project folders (folder placement = signal): name match -> set (+0.05 bonus); no name match in single-set project -> 0.7; else project-level. `--no-previews` opts out (iCloud). Harvest runs post-commit so sample cross-check sees the just-indexed catalog.
- [x] **ARCHITECTURE: crates/ops extracted** (user wants in-app scanning; "CLI for dev, app for users"): scan_library/hunt_renders/attach moved out of the cli bin into shared ops crate. Layering: als-core+previews -> indexer (storage) -> ops (workflows) -> cli/app (frontends). CLI commands are now thin wrappers.
- [x] In-app scanning: "Scan folder…" header button -> native picker (tauri-plugin-dialog, dialog:default capability) -> scan_folder command (ops::scan_library incl. harvest) -> stats+results refresh + summary message. NOTE: requires `npm install` (new plugin-dialog dep); per-file progress not yet surfaced (future: Tauri events + progress UI).
- [x] **GOTCHA (beach-ball incident)**: sync Tauri commands run on the MAIN thread -> scan froze the window. ALL commands now async; scan_folder additionally wraps work in tauri::async_runtime::spawn_blocking. Rule going forward: any command touching disk/db is async; anything heavy goes in spawn_blocking.
- [ ] **NEXT (user's test plan)**: dump db (`ableton-scan reset --yes`), bounce some current-year tracks into one folder, `scan` the matching projects + `previews` that folder, evaluate match quality from a controlled sample. NO full-system hunt (user explicitly declined).
- [ ] Later in M3: previews in detail pane (list all, switch primary), historical preview archive, in-app "hunt for previews" UI.
- [ ] M4: in-app export worker (second Live install + UI automation queue).

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
- [ ] Automated Live export worker (second Live install + UI automation; see ARCHITECTURE.md Preview Service)
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
./crates
./crates/als-core
./crates/cli
./crates/ops
./crates/previews
./crates/indexer
./app
./app/index.html
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
./ai/AGENTS.md
```

## 5. Recent Git Changes (Summary)
```text
9576e69 cargo lock update
72ae0a1 Fix UI freeze: all Tauri commands async, scan in spawn_blocking (sync commands run on main thread)
c92f1aa In-app scanning: ops crate + native folder picker
ac7b44d scan auto-harvests in-folder renders as previews (--no-previews opts out); folder placement boosts confidence
fdced93 Sample safety: discovery cross-checks catalog samples table; known sample paths never attach as previews
```

## 6. Active Diff
```diff
diff --git a/ai/CONTEXT_BUNDLE.md b/ai/CONTEXT_BUNDLE.md
index 0fdc921..0b94c06 100644
--- a/ai/CONTEXT_BUNDLE.md
+++ b/ai/CONTEXT_BUNDLE.md
@@ -1,5 +1,5 @@
 # AI Context Bundle
-Generated: Fri Jun 12 00:29:35 UTC 2026
+Generated: Fri Jun 12 00:49:27 UTC 2026
 
 ## ⚠️ Agent Navigation Guide
 1. Start with the **Current State** below to understand the focus.
@@ -104,6 +104,13 @@ This repository uses an AI-assisted engineering substrate located in `/ai`
 ## 3. Project State (PROJECT_STATE.md)
 # Project State
 
+## ⚡ HANDOFF SNAPSHOT (2026-06-11, end of session — read this first)
+- **Where things stand**: M1 (extraction) + M2 (catalog) + UI skeleton DONE and verified on the user's Mac. M3 previews: fully built (discovery, matching, peaks, player bar, in-app folder-picker scanning), but **awaiting user verification** of (a) the async/spawn_blocking UI-freeze fix (beach ball occurred on first in-app scan; fix committed 72ae0a1, not yet re-tested) and (b) the matcher against real bounces (user's plan: `reset --yes`, bounce current-year tracks to one folder, scan 2026 projects in-app or via CLI, then `previews <bounce folder> --verbose`).
+- **Working style**: user is NOT writing Rust (decided after the fact — AI writes all code, user compiles/tests on their Mac and gives product feedback). The sandbox cannot run cargo (network allowlist); ALL Rust verification happens on the user's machine. Keep tools/reference_extract.py in sync with any als-core parser change.
+- **Cadence that works**: user gives product feedback/requests -> implement -> commit with descriptive message -> user pulls, builds, tests -> log results + decisions here. Update these context files and commit at every meaningful step (project instruction).
+- **Run commands**: CLI `cargo run -p cli -- <subcommand>`; app `cd app && npm install && npm run tauri dev`.
+- **Next likely work**: live scan progress via Tauri events; detail-pane preview list (switch primary); `roots` table + rescan; iCloud `evicted` sample state; M4 in-app export worker (the flagship: drives a second Live install via UI automation to render previews overnight).
+
 ## Current Focus
 Phase: Milestone 3 — Previews (discovery half BUILT, awaiting host verification) (2026-06-11)
 - **Key user decision**: renders are SCATTERED across the computer (old consolidation script defunct) — discovery must NOT rely on project folders. It hunts user-chosen roots (Desktop, Downloads, ...) and name-matches against the catalog. Files never moved, only referenced.
@@ -118,6 +125,7 @@ Phase: Milestone 3 — Previews (discovery half BUILT, awaiting host verificatio
 - [x] In-folder harvest (user request): `scan` auto-harvests renders found inside project folders (folder placement = signal): name match -> set (+0.05 bonus); no name match in single-set project -> 0.7; else project-level. `--no-previews` opts out (iCloud). Harvest runs post-commit so sample cross-check sees the just-indexed catalog.
 - [x] **ARCHITECTURE: crates/ops extracted** (user wants in-app scanning; "CLI for dev, app for users"): scan_library/hunt_renders/attach moved out of the cli bin into shared ops crate. Layering: als-core+previews -> indexer (storage) -> ops (workflows) -> cli/app (frontends). CLI commands are now thin wrappers.
 - [x] In-app scanning: "Scan folder…" header button -> native picker (tauri-plugin-dialog, dialog:default capability) -> scan_folder command (ops::scan_library incl. harvest) -> stats+results refresh + summary message. NOTE: requires `npm install` (new plugin-dialog dep); per-file progress not yet surfaced (future: Tauri events + progress UI).
+- [x] **GOTCHA (beach-ball incident)**: sync Tauri commands run on the MAIN thread -> scan froze the window. ALL commands now async; scan_folder additionally wraps work in tauri::async_runtime::spawn_blocking. Rule going forward: any command touching disk/db is async; anything heavy goes in spawn_blocking.
 - [ ] **NEXT (user's test plan)**: dump db (`ableton-scan reset --yes`), bounce some current-year tracks into one folder, `scan` the matching projects + `previews` that folder, evaluate match quality from a controlled sample. NO full-system hunt (user explicitly declined).
 - [ ] Later in M3: previews in detail pane (list all, switch primary), historical preview archive, in-app "hunt for previews" UI.
 - [ ] M4: in-app export worker (second Live install + UI automation queue).
@@ -251,113 +259,12 @@ Phase: Milestone 3 — Previews (discovery half BUILT, awaiting host verificatio
 
 ## 5. Recent Git Changes (Summary)
 ```text
+9576e69 cargo lock update
+72ae0a1 Fix UI freeze: all Tauri commands async, scan in spawn_blocking (sync commands run on main thread)
+c92f1aa In-app scanning: ops crate + native folder picker
 ac7b44d scan auto-harvests in-folder renders as previews (--no-previews opts out); folder placement boosts confidence
 fdced93 Sample safety: discovery cross-checks catalog samples table; known sample paths never attach as previews
-9ede958 Matcher: keep bpm/key/prod tokens as identity (normalize form, not content) per user; add reset subcommand
-d20f14e M3 (discovery half): scattered-render hunt, name matcher, peaks, player bar
-e53f74b Open in Live / Reveal in Finder: open_set command (catalog paths only), row hover button + detail actions
 ```
 
 ## 6. Active Diff
 ```diff
-diff --git a/Cargo.lock b/Cargo.lock
-index 44f8a4f..1279cfb 100644
---- a/Cargo.lock
-+++ b/Cargo.lock
-@@ -129,10 +129,12 @@ dependencies = [
-  "als-core",
-  "dirs 5.0.1",
-  "indexer",
-+ "ops",
-  "serde",
-  "serde_json",
-  "tauri",
-  "tauri-build",
-+ "tauri-plugin-dialog",
- ]
- 
- [[package]]
-@@ -463,12 +465,10 @@ version = "0.1.0"
- dependencies = [
-  "als-core",
-  "anyhow",
-- "chrono",
-  "clap",
-  "dirs 5.0.1",
-  "indexer",
-- "previews",
-- "rusqlite",
-+ "ops",
-  "serde_json",
- ]
- 
-@@ -2225,6 +2225,7 @@ checksum = "e3e0adef53c21f888deb4fa59fc59f7eb17404926ee8a6f59f5df0fd7f9f3272"
- dependencies = [
-  "bitflags 2.13.0",
-  "block2",
-+ "libc",
-  "objc2",
-  "objc2-core-foundation",
- ]
-@@ -2309,6 +2310,19 @@ version = "1.70.2"
- source = "registry+https://github.com/rust-lang/crates.io-index"
- checksum = "384b8ab6d37215f3c5301a95a4accb5d64aa607f1fcb26a11b5303878451b4fe"
- 
-+[[package]]
-+name = "ops"
-+version = "0.1.0"
-+dependencies = [
-+ "als-core",
-+ "anyhow",
-+ "chrono",
-+ "indexer",
```
