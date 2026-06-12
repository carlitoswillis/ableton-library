# 🗺️ Ableton Library — Human Roadmap

Welcome to the roadmap! This document is designed for quick human reading to see where the project stands, what's been built, and what's coming up next.

---

## 🚦 Current Status
* **Phase**: **Milestone 4 — Export Worker** (⚡ In Progress — mechanism built, real-world rendering is the hard part)
* **Status**: The machinery works (worker loop, `tools/export_set.py` automation, Render Queue UI). But field testing against old projects exposed the real problems: iCloud-evicted samples stall or slow bounces badly; old sets reference missing plugins and moved samples (Live can relocate them but only via slow scanning on open) — so bounces can take forever AND still come out missing a ton of audio. M4 is **in progress** until renders of imperfect projects are acceptable.

---

## 🗺️ The Milestone Roadmap

### ✅ Milestone 1: Metadata Extraction (`als-core`)
* **Goal**: Read gzipped Ableton Live `.als` sets directly from disk and extract metadata without opening Live.
* **Status**: **Complete & Verified** (Live 10.1 to 11.3, backwards-compatible, parser outputs matched byte-for-byte against python test oracle).

### ✅ Milestone 2: Project Catalog (`indexer`)
* **Goal**: Store sets, tracks, plugins, and samples in a local SQLite database with FTS5 search.
* **Status**: **Complete & Verified** (Incremental indexing via size/mtime checks, prune deleted sets, ranking search by name significance).

### ⚡ Milestone 3: Previews (`previews` & `app`) — *Complete*
* **Goal**: Scan folders for renders, match them to sets by name, extract waveform peaks, and play them in the UI.
* **Completed**:
  * **Render discovery & matching**: Loose audio file discovery with stopwords and `vN` stripping, optimized directory walking, and Jaccard name matching.
  * **Peak extraction**: Decode audio using `symphonia` and downsample to canvas peaks.
  * **Player UI**: Interactive bottom player bar with canvas waveform, click-to-seek, and play/pause controls.
  * **Tauri scan progress**: Live-scrolling logs terminal inside the app, stats counters, and full scan cancel/background minimize support.
  * **Multi-threaded scanning**: Library scanning (`.als` decompression + XML parsing), bulk preview scan, and in-folder preview harvest all parallelized via `std::thread::scope` across all CPU cores (~6-8x speedup).
  * **Interleaved scan + harvest**: Instead of indexing all projects then previewing all projects in two separate passes, each project's previews are harvested immediately after its sets are indexed. The user sees `indexed → preview → indexed → preview` in the scan logs. Sample cross-check (`known_samples`) is built incrementally so it's always accurate.
  * **BPM parsing & duplicate render filtering**: Enhanced BPM extraction and smarter filtering of duplicate render matches.
* **Up Next (Backlog)**:
  * 🔀 **Previews list & primary switcher**: Show all matched/manual previews for a set in the detail pane, play them, and choose which one is the "Primary" preview.
  * 🔄 **`roots` table & rescan**: Remember all folders that have been scanned in a database table so they can be refreshed at the click of a button.
  * ☁️ **iCloud `evicted` sample state**: Differentiate between truly missing samples and cloud-only placeholder `.icloud` files.

### ⚡ Milestone 4: Export Worker (Flagship Automation) — *In Progress*
* **Goal**: Automatically render previews for sets that don't have existing renders.
* **Built**: Python GUI automation script `tools/export_set.py` integrated into a Rust background worker; Render Queue UI with start/pause, status feedback, real-time refresh.
* **Field findings (2026-06-12, the hard part)**: old projects render slowly and incompletely —
  * iCloud-evicted samples stall bounces (download-on-touch) or silently slow them to a crawl.
  * Missing third-party plugins (incl. synths) = silent tracks in the render.
  * Samples moved-but-findable: Live relocates them via slow scanning on open; un-relinked refs still cost time or come out missing.
* **Plan (phased)**:
  * **M4a — Triage & fidelity (catalog-driven, no .als modification)**: per-set *renderability score* from data we already have (missing plugins? missing/evicted samples?); worker queue ordered easy-first; pre-flight `brctl download` of a project's iCloud samples before bouncing; store *fidelity metadata* on worker previews ("rendered missing: Serum, soothe2") and show it in the UI.
  * **M4b — Preview-proxy sets (writes a COPY, never the original)**: generate `<name> (preview proxy).als` with (1) missing *effects* bypassed/removed, (2) sample FileRefs relinked to copies our catalog/filename-search can locate (skips Live's slow relocate scan). Render the proxy.
  * **M4c — Instrument stand-ins (experimental)**: substitute missing *synths* with a built-in instrument so MIDI tracks aren't silent. ⚠️ Honest limit: third-party plugin state in .als is an opaque binary blob — we can NEVER recover what the patch sounded like. Stand-ins = category guess from track/device name ("bass" -> bass-ish native preset), i.e. "hear the notes," not "hear the sound."
* **Backlog**: overwrite-confirmation dialog handling in UI scripting.

---

## 🛠️ Project Stack & Layering
1. **`crates/als-core`**: gzip + streaming XML parser (extremely fast, memory safe).
2. **`crates/previews`**: Symphonia audio decoder, waveform peak extractor, name similarity scorer.
3. **`crates/indexer`**: SQLite + FTS5 search schema and queries.
4. **`crates/ops`**: High-level library scan, preview attach, and render hunt workflows. Multi-threaded via `std::thread::scope`.
5. **`app/src-tauri`**: Tauri 2 Rust backend commands (async, running heavy tasks in `spawn_blocking`).
6. **`app/src`**: React 18 + TS + Vite webview frontend.

---

## 📋 Next Tasks Selection
When you are ready for the next task, we can tackle one of the following:

1. **Naming consistency pass**: Unify terminology across the codebase (`hunt`/`harvest`/`discover`, `render`/`preview`, `ops`→`workflows`, etc.) — see ARCHITECTURE.md for the full table.
2. **Previews list & primary switcher in detail pane**: Exposes multiple previews per set in the UI, allowing you to preview different mix iterations and select your favorite.
3. **`roots` table + rescan button**: Remembers previously scanned project libraries to enable quick refreshes/rescans from the app header.
4. **iCloud `evicted` state detection**: Adds support for recognizing evicted `.icloud` sample placeholders.
