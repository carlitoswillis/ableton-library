# Architecture

PURPOSE: Technical system design and data flow of the Ableton Library application.

## Overview
Ableton Library is a metadata and preview indexing system for Ableton projects, allowing users to browse and search their library without opening Ableton Live.

## Stack Decision (2026-06-11)
**Rust core + Tauri 2 desktop shell + React/TS frontend + SQLite.** CLI-first: the extraction core and indexer ship as a Rust CLI and are validated against the real library before any UI is built.

### Repository Layout (Cargo workspace)
```
crates/als-core/   # lib: gzip (flate2) + streaming XML (quick-xml) -> ProjectSnapshot (serde)
crates/indexer/    # lib: SQLite (rusqlite + FTS5), incremental scan (walkdir, mtime+hash)
crates/cli/        # bin: `ableton-scan` (clap) — scan, query, inspect
app/               # Tauri 2 + React/TS (Milestone 2+); later: symphonia for waveform peaks
```

## System Components

### 1. Filesystem Scanner — `als-core` (Rust)
- **Purpose**: Extract project information from Live Sets and folders.
- **Approach**: Streaming XML parse (never full DOM — .als can decompress to 100s of MB).
- **Extracts**: Live version, tempo/time sig, tracks (type/name/color), clip names, device/plugin names, sample file references.
- **Output**: Normalized ProjectSnapshot JSON per set.

### 2. Metadata & Indexing Service — `indexer` (Rust + SQLite)
- **Decision**: SQLite with FTS5 (over names) for search.
- **Model**: A project *folder* contains one or more `.als` *sets*. Tables: projects -> sets (tempo, version, hash, mtime) -> tracks, plugins, samples (path + missing flag), previews.
- **Incremental**: Reindex keyed on mtime + content hash. Index lives in app data dir, never inside user project folders.

### 3. Preview Service
- **Constraint**: Headless render of .als without Live is impossible. Previews are *discovered*, priority order: (a) user-exported renders in/near project folder, (b) Live 12 set previews in `Ableton Project Info/` (verify), (c) frozen/processed audio fallback.
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
