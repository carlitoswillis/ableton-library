# AI Context Bundle
Generated: Thu Jun 11 23:24:12 UTC 2026

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
crates/als-core/   # lib: gzip (flate2) + streaming XML (quick-xml) -> SetSnapshot (serde)  [BUILT, verified vs oracle]
crates/cli/        # bin: `ableton-scan` (clap + walkdir)  [BUILT, verified vs oracle]
crates/indexer/    # lib: SQLite (rusqlite + FTS5), incremental scan (mtime+hash)  [NEXT]
tools/reference_extract.py  # executable spec / test oracle for als-core; keep in sync
app/               # Tauri 2 + React/TS (Milestone 3+); later: symphonia for waveform peaks
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

### 4. User Interface — Tauri 2 (Milestone 2+)
- **Decision**: Tauri 2 shell, React/TS frontend; core logic lives in the Tauri Rust backend (no sidecar). Audio streamed to webview via asset protocol.
- **Views**: Library View (Search/Filters), Project Detail View (Metadata/Tracks/Player).

## Data Flow
Filesystem (.als) -> als-core (streaming parse) -> indexer (SQLite) -> Tauri commands -> React UI

## AI Workspace Substrate
This repository uses an AI-assisted engineering substrate located in `/ai`
- **Cognition Layer**: State and tasks are tracked in `/ai`.
- **Rules**: Agent constraints are defined in `AGENTS.md`.
- **Flow**: Human Pilot -> AI Implementation -> Deterministic Verification.

## 3. Project State (PROJECT_STATE.md)
# Project State

## Current Focus
Phase: Milestone 2 — Project Catalog (indexer) (2026-06-11)
- [ ] Implement `indexer` crate: SQLite (rusqlite, bundled) + FTS5 over names; schema projects -> sets -> tracks/devices/samples + previews; incremental reindex keyed on mtime + content_hash.
- [ ] CLI subcommands: `scan` (index into db), `query`/`search`, `inspect <set>`.
- [ ] Decide index location (app data dir, e.g. ~/Library/Application Support/ableton-library/).

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
- **Repo conventions**: scan JSON outputs go in `exports/` (gitignored); Cargo.lock untracked (user preference; revisit — convention for binary projects is to commit it). (2026-06-11)

## Backlog
- [ ] Automated Live export worker (second Live install + UI automation; see ARCHITECTURE.md Preview Service)
- [ ] Preview archive: keep historical previews per set, potentially anchored to Backup/ timestamps (stretch; pairs with --deep backup parsing)
- [ ] Sample `evicted` state: detect iCloud `.icloud` placeholders vs truly missing files
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
f7e57bd update clock
884b891 gitignore: normalize (user edit); untrack Cargo.lock per user preference
64156ad Track Cargo.lock (reproducible builds for binary workspace)
7bb3100 README: document exports/ convention
3a906e1 Convention: scan outputs live in exports/ (gitignored); SQLite will be canonical store
```

## 6. Active Diff
```diff
diff --git a/ai/CONTEXT_BUNDLE.md b/ai/CONTEXT_BUNDLE.md
index 0480fcb..994b09e 100644
--- a/ai/CONTEXT_BUNDLE.md
+++ b/ai/CONTEXT_BUNDLE.md
@@ -1,5 +1,5 @@
 # AI Context Bundle
-Generated: Thu Jun 11 15:27:04 PDT 2026
+Generated: Thu Jun 11 23:24:12 UTC 2026
 
 ## ⚠️ Agent Navigation Guide
 1. Start with the **Current State** below to understand the focus.
@@ -21,6 +21,8 @@ PURPOSE: This is the authoritative rulebook for AI assistants. It defines the 'h
   - **Frontend**: React, Vue, or Desktop Native (Electron, Tauri).
 
 ## Architecture Constraints
+- **No Ableton SDK dependency**: User runs Live 11; the Extensions SDK (Live 12 Suite beta only) is off the table. Filesystem-first is the strategy, not a fallback.
+- **Version tolerance (backward + forward)**: Parser must handle .als files from older Live versions (9/10/11) and newer ones (12+). Extract leniently — skip unknown elements, never hard-fail on schema drift, record the Live version (Creator attribute) per set.
 - **API/Service Structure**: Modular service for metadata and preview management.
 - **Database/Persistence**: Local persistence for indexing and snapshots.
 - **Markdown Persistence**: All state must be tracked in `/ai`.
@@ -48,31 +50,47 @@ PURPOSE: Technical system design and data flow of the Ableton Library applicatio
 ## Overview
 Ableton Library is a metadata and preview indexing system for Ableton projects, allowing users to browse and search their library without opening Ableton Live.
 
+## Stack Decision (2026-06-11)
+**Rust core + Tauri 2 desktop shell + React/TS frontend + SQLite.** CLI-first: the extraction core and indexer ship as a Rust CLI and are validated against the real library before any UI is built.
+
+### Repository Layout (Cargo workspace)
+```
+crates/als-core/   # lib: gzip (flate2) + streaming XML (quick-xml) -> SetSnapshot (serde)  [BUILT, verified vs oracle]
+crates/cli/        # bin: `ableton-scan` (clap + walkdir)  [BUILT, verified vs oracle]
+crates/indexer/    # lib: SQLite (rusqlite + FTS5), incremental scan (mtime+hash)  [NEXT]
+tools/reference_extract.py  # executable spec / test oracle for als-core; keep in sync
+app/               # Tauri 2 + React/TS (Milestone 3+); later: symphonia for waveform peaks
+```
+
 ## System Components
 
-### 1. Filesystem Scanner (.als + project folders)
+### 1. Filesystem Scanner — `als-core` (Rust)
 - **Purpose**: Extract project information from Live Sets and folders.
-- **Status**: Pivot from Extension-based to Filesystem-first.
-- **Responsibilities**: Metadata extraction, XML/Gzip parsing (.als), and normalization.
-
-### 2. Metadata & Indexing Service
-- **Purpose**: Persist and query project information.
-- **Options**: 
-  - Relational (SQLite) for structured queries.
-  - Document-based (JSON/Embedded DB) for flexibility.
-- **Responsibilities**: Receive metadata, normalize records, store project snapshots.
-
-### 3. Preview Service
-- **Purpose**: Associate audio previews with projects.
-- **Responsibilities**: Detect preview files, store preview metadata, generate/cache waveform data.
-
-### 4. User Interface
-- **Purpose**: Browse and search projects.
-- **Options**: Web-based (React/Vite), Desktop-native (Tauri/Rust), or CLI.
+- **Approach**: Streaming XML parse (never full DOM — .als can decompress to 100s of MB).
+- **Version tolerance**: No Ableton SDK (user on Live 11; SDK is Live 12 Suite beta only). Parse leniently across Live versions, backward (9/10/11) and forward (12+): ignore unknown elements, tolerate missing ones, record Creator/version per set, and emit per-field extraction warnings instead of failing the whole file.
+- **Extracts**: Live version, tempo/time sig, tracks (type/name/color), clip names, device/plugin names, sample file references.
+- **Output**: Normalized ProjectSnapshot JSON per set.
+
+### 2. Metadata & Indexing Service — `indexer` (Rust + SQLite)
+- **Decision**: SQLite with FTS5 (over names) for search.
+- **Model**: A project *folder* contains one or more `.als` *sets*. Tables: projects -> sets (tempo, version, hash, mtime) -> tracks, plugins, samples (path + missing flag), previews.
+- **Incremental**: Reindex keyed on mtime + content hash. Index lives in app data dir, never inside user project folders.
+
+### 3. Preview Service (pluggable source interface)
+- **Pipeline**: watcher sees .als save -> debounced job queue -> preview *source* resolves audio -> peaks cached -> catalog updated.
+- **Constraint**: Reimplementing Live's render engine is ruled out permanently. Live itself is the only correct renderer.
+- **Sources (priority)**:
+  - (a) **Discovery** (MVP): user-exported renders in/near project folder; Live 12 set previews in `Ableton Project Info/` (verify); frozen/processed audio fallback.
+  - (b) **Automated Live export** (flagship, post-catalog): worker launches a *second* Live install with the set, drives File -> Export via macOS UI automation (proven previously by owner). Constraints: serialize one render at a time; debounce save bursts; handle dialogs (missing samples, version prompts); UI scripting steals focus so make it opt-in/idle-scheduled; treat Live as flaky (timeouts, retry once, mark "render failed" rather than wedging queue). Isolated component — can start as a standalone script consuming jobs and emitting audio files.
+- **Previews are per-SET, not per-project** (projects can hold multiple distinct .als, e.g. "wanna be your" + "wanna be your2"). Discovery must match found renders to sets by filename similarity (normalized prefix match vs set name); ambiguous matches attach at project level with low confidence. The export worker has no ambiguity (it knows which set it rendered).
+- **Waveforms**: Decode (symphonia), precompute peaks once, cache keyed by set hash.
+
+### 4. User Interface — Tauri 2 (Milestone 2+)
+- **Decision**: Tauri 2 shell, React/TS frontend; core logic lives in the Tauri Rust backend (no sidecar). Audio streamed to webview via asset protocol.
 - **Views**: Library View (Search/Filters), Project Detail View (Metadata/Tracks/Player).
 
 ## Data Flow
-Filesystem (.als) -> Extraction Logic -> Indexing Service -> Local Storage -> UI Layer
+Filesystem (.als) -> als-core (streaming parse) -> indexer (SQLite) -> Tauri commands -> React UI
 
 ## AI Workspace Substrate
 This repository uses an AI-assisted engineering substrate located in `/ai`
@@ -84,42 +102,94 @@ This repository uses an AI-assisted engineering substrate located in `/ai`
 # Project State
 
 ## Current Focus
-Phase: Planning (2026-06-11)
-- [ ] Pivot to Filesystem-first architecture (Live 11+ support).
-- [ ] Research/Select technology stack (Go vs Rust vs Node for extraction).
-- [ ] Implement Metadata Extraction MVP.
+Phase: Milestone 2 — Project Catalog (indexer) (2026-06-11)
+- [ ] Implement `indexer` crate: SQLite (rusqlite, bundled) + FTS5 over names; schema projects -> sets -> tracks/devices/samples + previews; incremental reindex keyed on mtime + content_hash.
+- [ ] CLI subcommands: `scan` (index into db), `query`/`search`, `inspect <set>`.
+- [ ] Decide index location (app data dir, e.g. ~/Library/Application Support/ableton-library/).
+
```
