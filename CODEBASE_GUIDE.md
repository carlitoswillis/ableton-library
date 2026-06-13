# Codebase Guide

> **What this is.** A deep, greppable reference for *understanding* and *changing* this
> project. The README is for users; `ARCHITECTURE.md` records high-level decisions;
> `PROJECT_STATE.md` is the running log. **This file is the developer's map** — the data
> model, every subsystem, the invariants that bite, and step-by-step recipes for common
> changes. When you're about to touch the code, start here.
>
> Keep it current: if you change the schema, a command surface, or an invariant, update the
> relevant section. Search by the `§` anchors below.

## Table of contents

- [§1 Orientation](#1-orientation) — what the app is, the philosophy
- [§2 Glossary](#2-glossary) — project vs set vs preview, etc.
- [§3 Crate map & layering](#3-crate-map--layering)
- [§4 End-to-end data flow](#4-end-to-end-data-flow)
- [§5 The data model](#5-the-data-model) — every table, every column, schema versions
- [§6 Subsystem reference](#6-subsystem-reference) — crate by crate
- [§7 Invariants & gotchas](#7-invariants--gotchas) — the hard-won rules
- [§8 How to make common changes](#8-how-to-make-common-changes) — recipes
- [§9 File index](#9-file-index)
- [§10 Build, test, run](#10-build-test-run)

---

## §1 Orientation

**Ableton Library** is a *local-first* catalog for Ableton Live projects: browse, search, and
preview your `.als` files without opening Live. There is **no Ableton SDK** — everything is
read straight from disk. An `.als` is gzipped XML; the parser streams it and extracts
metadata (tempo, tracks, devices/plugins, samples, locators).

Guiding principles (enforced throughout — see [§7](#7-invariants--gotchas)):

- **Filesystem-first, local-only.** No cloud, no SDK. The catalog is a *rebuildable cache*.
- **Incremental & partial by design.** The user scans subfolders piecemeal; never assume the
  catalog is complete.
- **Lenient & version-tolerant parsing.** Tolerate Live 9 → 12+; a missing field is a
  warning, a corrupt file is skipped — one bad project never aborts a scan.
- **CLI-first development.** Core logic is validated through the `ableton-scan` CLI before the
  Tauri app wires it up. Both frontends call the same `ops`/`indexer` code.
- **The user is not writing Rust.** AI writes the code; the user builds/tests on their Mac.
  The dev sandbox cannot run `cargo`.

---

## §2 Glossary

| Term | Meaning |
|------|---------|
| **Project** | A *folder* that directly contains one or more `.als` files. One row in `projects`. |
| **Set** | A single `.als` file. One row in `sets`. A project can hold several sets. |
| **Backup** | A timestamped autosave in a project's `Backup/` folder. Indexed as lineage only (filename/size/mtime), never parsed. |
| **Snapshot** | `SetSnapshot` — the parsed-from-disk representation of one set (see `als-core::model`). |
| **Render / preview / bounce** | An exported audio file associated with a set. The codebase uses all three words (see naming-debt note). Stored in `previews`. |
| **Harvest** | Find renders *inside* a project's own folder during a scan and attach them as previews. |
| **Hunt** | Scan arbitrary *watch folders* (Desktop, Downloads…) for renders and match them to sets by name. |
| **Discovery** | Umbrella for harvest + hunt: matching audio files to sets by filename similarity. |
| **Export worker** | The automation that drives a real Live install to *render* previews for sets that have none. |
| **Proxy set** | An ephemeral `.als` copy (in the app cache) with sample paths rewritten, so the worker can render a set whose samples moved — the original is never touched. |
| **Triage / renderability** | A 0..1 score estimating how cleanly a set will bounce on *this* machine (missing plugins/samples lower it). |
| **Fidelity** | An honest "what was missing" report stamped on worker-rendered previews. |
| **Effective artist** | `COALESCE(sets.artist_override, projects.artist)` — per-set manual tag, else the path-derived project artist. |
| **List** | A user-curated collection of sets (favorites + named lists). Many-to-many, keyed by path. |
| **Oracle** | `tools/reference_extract.py` — the executable spec the Rust parser must match byte-for-byte. |

---

## §3 Crate map & layering

A Cargo workspace. **Dependencies point downward only** — never import a frontend crate from a
library crate.

```
                 als-core            previews
              (parse .als ->      (find renders,
               SetSnapshot;        name-match,
               discovery)          waveform peaks)
                    \                 /
                     \               /
                      v             v
                       indexer  (SQLite + FTS5 catalog; the storage layer)
                          |
                          v
                        ops  (workflows: scan pipeline, harvest/hunt, link
                              suggestions, artist, triage, proxy, places)
                         / \
                        /   \
                       v     v
                     cli     app/src-tauri   (frontends — thin wrappers)
                                  |
                                  v
                            app/src (React UI)
```

| Crate | Path | Role |
|-------|------|------|
| `als-core` | `crates/als-core` | Streaming gzip+XML `.als` parser → `SetSnapshot`; project discovery. Oracle-bound. |
| `previews` | `crates/previews` | Render discovery, filename matching/scoring, symphonia waveform peak extraction. |
| `indexer` | `crates/indexer` | The SQLite catalog: schema, migrations, ingest, search, all CRUD. **The single source of DB truth.** |
| `ops` | `crates/ops` | Cross-cutting workflows shared by both frontends (scan, harvest, hunt, link, artist, triage, proxy). |
| `cli` | `crates/cli` | The `ableton-scan` binary (the crate is named `cli`; the bin is `ableton-scan`). Thin wrappers over `ops`/`indexer`. |
| `app/src-tauri` | `app/src-tauri` | Tauri 2 Rust backend: `#[tauri::command]`s over `ops`/`indexer` + the export worker. |
| `app/src` | `app/src` | React 18 + TS + Vite frontend (single `App.tsx` + `PlayerBar.tsx`). |

---

## §4 End-to-end data flow

**Scan → index.** `ops::scan_library(root)` calls `als_core::discover(root)` (walk to any
depth, a project = a dir directly holding `.als`). For each project it `upsert_project`s
(deriving the artist from the path), then for each set checks freshness (`size`+`mtime`); stale
sets are queued for parsing. A **unified worker pool** (`std::thread::scope`) runs
`Job::Parse` (gzip→XML→`SetSnapshot`) and `Job::Decode` (audio peak extraction) jobs; the main
thread does all SQLite writes sequentially. `ingest_set` replaces the set row + child rows +
its FTS entry. Preview harvest is **interleaved** — a project's renders are matched the moment
its last set is ingested (decode jobs jump to the front of the queue so previews appear live).

**Search.** `indexer::search(SearchOpts)` builds one SQL query (a text branch using the FTS5
`search` table with weighted `bm25`, and a no-text branch) plus filters for tempo, plugin,
artist, list membership, date, and preview presence. Returns `SearchHit`s.

**Preview discovery.** Two entry points: **harvest** (inside project folders, during scan) and
**hunt** (`ops::hunt_renders` over user watch folders). Both normalize names, score
similarity (`previews::matching`), guard against attaching a file that's a known *sample*, and
extract waveform peaks (`previews::peaks`) for matches. Files are **referenced, never moved**.

**Export worker** (app only). When "Auto-Export" is on, `export_worker_loop` polls every 3s,
picks the highest-renderability pending job, pre-flights it (relink missing samples into a
**proxy** copy, materialize iCloud-evicted samples), then runs `tools/export_set.py` to drive
Live's File→Export via macOS UI automation. The finished `.wav` is attached as a
`source='worker'` preview with a fidelity stamp.

---

## §5 The data model

SQLite (bundled via rusqlite) at `~/Library/Application Support/ableton-library/library.db`
(override with `--db`). **WAL mode, `foreign_keys=ON`.** Defined in
`crates/indexer/src/lib.rs` (the `SCHEMA*` consts). The catalog is always fully rebuildable
from `.als` files.

### Tables

**`projects`** — one folder.
| col | type | notes |
|-----|------|-------|
| `id` | INTEGER PK | |
| `folder_path` | TEXT UNIQUE NOT NULL | absolute path |
| `name` | TEXT | folder name |
| `last_scanned` | TEXT | ISO-8601 |
| `artist` | TEXT (v7) | path-derived or `--artist` override; NULL = unknown |

**`sets`** — one `.als`.
| col | type | notes |
|-----|------|-------|
| `id` | INTEGER PK | **changes on re-ingest** (delete+reinsert) |
| `project_id` | INTEGER FK→projects | ON DELETE CASCADE |
| `als_path` | TEXT UNIQUE NOT NULL | **the stable identity** (use for durable refs) |
| `file_size`, `mtime` | INTEGER, TEXT | freshness key |
| `content_hash` | TEXT | SHA-256 of the gzipped bytes |
| `live_version`, `schema_version` | TEXT | from root `Creator`/`MinorVersion` |
| `tempo` | REAL | master tempo |
| `tempos_json` | TEXT (v5) | JSON array of all tempos found |
| `time_signature` | TEXT | e.g. "4/4" |
| `warnings` | TEXT | JSON array of lenient-extraction notes |
| `artist_override` | TEXT (v8) | per-set manual artist; NULL = inherit project's |

**`tracks`** (`set_id`, `idx`, `kind` midi/audio/return/group, `name`, `color`)
**`devices`** (`set_id`, `track_ref` "N"/"master"/NULL, `kind` native/au/vst/vst3, `name`, `manufacturer`)
**`samples`** (`set_id`, `path`, `in_project` 0/1, `exists_on_disk` 0/1)
**`locators`** (`set_id`, `name`, `time` in beats)
**`backups`** (`project_id`, `file`, `size`, `mtime`) — lineage only

**`search`** — FTS5 virtual table. Columns: `set_id UNINDEXED, project_name, set_name,
track_names, device_names, sample_names`. Written by `ingest_set` (space-joined names). Ranked
with `bm25(f.search, 0.0, 8.0, 10.0, 4.0, 1.0, 0.5)` — set/project names weigh most, samples
least. **FTS rows don't cascade** — `ingest_set`/`prune_missing` delete them explicitly.

**`previews`** (v2) — an audio file attached to a set (or project when ambiguous).
| col | notes |
|-----|-------|
| `set_id` FK (nullable) | NULL = project-level ambiguous match |
| `project_id` FK (nullable) | |
| `audio_path` | referenced, never owned/deleted |
| `source` | `discovered` \| `worker` \| `manual` |
| `confidence` | 0..1 match confidence (manual = 1.0) |
| `mtime`, `size`, `duration` | |
| `peaks` | JSON array of 0..1 floats (waveform bins) |
| `is_primary` | highest confidence then newest wins (`recompute_primary`) |
| `fidelity` (v6) | JSON "what was missing" for worker renders; NULL = full fidelity |

**`export_jobs`** (v3) — the render queue. `set_id` UNIQUE; `status` pending/processing/
completed/failed; `score` + `fidelity` (v6) from triage.
**`watch_folders`** (v4) — `path` UNIQUE.
**`ignored_matches`** (v4) — `(set_id, audio_path)` PK; suppressed preview suggestions.
**`lists`** (v9) — `id`, `name`, `created_at`; **UNIQUE index on `name COLLATE NOCASE`**.
**`list_items`** (v9) — `(list_id FK→lists ON DELETE CASCADE, als_path, added_at)`,
PK `(list_id, als_path)`. **Keyed by `als_path`, not set_id**, so lists survive re-ingest;
pruned sets leave benign orphans that reattach if the set returns.

### Schema versions (`PRAGMA user_version`)

| v | Added |
|---|-------|
| 1 | base tables + FTS `search` |
| 2 | `previews` |
| 3 | `export_jobs` |
| 4 | `watch_folders`, `ignored_matches` |
| 5 | `sets.tempos_json` |
| 6 | `export_jobs.score`/`fidelity`, `previews.fidelity` |
| 7 | `projects.artist` |
| 8 | `sets.artist_override` |
| 9 | `lists`, `list_items` |

`SCHEMA_VERSION` is the current value. `open()` migrates older catalogs **in place** via
ordered `if version == N { … version = N+1 }` gates; a catalog **newer** than the build is
refused. See [§8 recipe: add a schema version](#add-a-column--schema-version).

---

## §6 Subsystem reference

### als-core (`crates/als-core`)
- **`model.rs`** — `SetSnapshot` (the parsed set) and children: `Track`/`TrackKind`,
  `Device`/`DeviceKind`/`TrackRef`, `SampleRef`, `Locator`, `ProjectSnapshot`, `BackupEntry`.
  **Must stay field-for-field identical to the Python oracle** (these serialize to the `json`
  command's output).
- **`parser.rs`** — `parse_set(als_path, project_dir) -> SetSnapshot`. Streaming only (gzip →
  `quick_xml` events; never builds a DOM — an `.als` can decompress to 100s of MB). Skips bulk
  subtrees wholesale (`SKIP_SUBTREES`: AutomationEnvelopes, KeyTracks, Notes, Events,
  ParameterSettings, …). Names are scoped by a path stack (`EffectiveName` exists on both
  tracks and devices). Lenient: missing field → warning, never an error.
- **`scan.rs`** — `discover(root) -> Vec<DiscoveredProject>` (recurses to any depth, skips
  `Backup/`); `iso_mtime(path)` (the one canonical timestamp format).

### previews (`crates/previews`)
- **`lib.rs`** — `discover_renders(roots, max_depth) -> Vec<RenderFile>` (audio exts, ≥ size
  floor, skips Samples/Backup/Project Info dirs).
- **`matching.rs`** — `normalize(s)` (strip bracketed timestamps, stopwords, vN; normalize
  "145bpm"→"145 bpm" — bpm/key are kept as distinguishing signal); `score(a, b) -> f64`
  (exact 1.0 > word-boundary prefix 0.85 > token Jaccard); `best_match(stem, cands, threshold)`.
- **`peaks.rs`** — `extract(path) -> PeaksResult` (symphonia decode → ≤1500 coarse-then-
  downsampled bins); `to_json(&[f32])`.

### indexer (`crates/indexer/src/lib.rs`) — 57 public fns, grouped:
- **Lifecycle:** `open` (migrations), `SCHEMA_VERSION`.
- **Ingest:** `upsert_project` (artist via `COALESCE(?, artist)` — won't clobber an explicit
  artist with NULL), `ingest_set` (replace set + children + FTS), `replace_backups`,
  `set_is_fresh`, `prune_missing` (root-scoped: only prunes under the scanned root).
- **Search/detail:** `search(SearchOpts) -> Vec<SearchHit>`, `set_detail`, `resolve_set`,
  `stats`, `set_path`, `set_project_id`, `project_sets`, `set_match_candidates`.
- **Previews:** `upsert_preview`, `recompute_primary`, `primary_preview`,
  `preview_is_fresh`, `remove_preview` (DB-row only — never deletes the audio),
  `prune_stale_previews`.
- **Export queue:** `add_export_job(s_bulk)`, `get_pending_export_job` (easy-first), triage
  setters, status updates, clear/retry helpers.
- **Artist:** `set_project_artist` / `set_project_artist_opt`, `set_set_artist_override`,
  `list_artists` (counts sets by effective artist), `all_projects` (backfill).
- **Lists:** `create_list` (select-first get-or-create, case-insensitive — *not* `ON CONFLICT`,
  see gotcha), `delete_list`, `rename_list`, `all_lists`, `add_to_list`, `remove_from_list`,
  `lists_for_path`.
- **Watch/ignore:** `add/remove/list_watch_folders`, `add_ignored_match`, `is_match_ignored`.

`SearchOpts` fields: `text, min_bpm, max_bpm, plugin, artist, list_id, sort_by, date_modified,
date_scanned, has_preview`. `SearchHit` fields: `set_id, project, artist, als_path, tempo,
tempos, time_signature, live_version, has_preview, preview_duration, in_list`.

### ops (`crates/ops`)
- **`lib.rs`** — `scan_library(conn, root, force, harvest, artist_override, cancel, log)` (the
  unified parse+decode worker pool), `reindex_artists` (no-scan artist backfill from stored
  paths), `plan_folder_harvest` / `harvest_folder_renders` (in-project render matching),
  `hunt_renders` (watch-folder discovery), `attach` (manual preview), `link_suggestions` /
  `get_watch_suggestions` (bounce-to-set suggestions). `Log` = `&mut dyn FnMut(String)`.
- **`artist.rs`** — `infer_artist(root, project_dir)` (pass 1: `artists/<name>` marker over the
  full path; pass 2: positional skip of year/month/bucket below the scan root);
  `artist_from_full_path` (marker-only, used by `reindex_artists`).
- **`triage.rs`** — `installed_plugins[_quick]` (recursive plugin-dir scan; auval removed),
  `plugin_installed` (space-squashed fuzzy match), `renderability` (0..1 score), `sample_state`
  (incl. `.icloud` eviction), `score_pending_jobs`, `relink_missing_samples`,
  `materialize_icloud_samples`, `restamp_worker_previews`.
- **`proxy.rs`** — `plan_relink`, `create_proxy_set` (write a rewritten `.als` copy to the
  cache; original untouched), `get_proxy_cache_dir`.
- **`sample_index.rs`** — `SampleIndex` / `build_search_index` (one-pass recursive index of
  Places + project folders, with budgets; Live-style relaxed lookup tiers).
- **`places.rs`** — `get_ableton_places()` (parse `Library.cfg` for user-pinned folders).

### cli (`crates/cli/src/main.rs`) — binary `ableton-scan`
Subcommands: `json`, `scan` (`--force --no-previews --artist`), `search`
(`--min-bpm --max-bpm --plugin --artist`), `artists`, `reindex-artists`,
`set-artist <set> <name> [--project]`, `inspect`, `stats`, `previews`, `reset`, `triage`,
`relink`, `rescore`, `attach`, `proxy`. Each is a thin wrapper calling `ops`/`indexer`.

### app (`app/`)
- **`src-tauri/src/lib.rs`** — Tauri commands (all `async`, snake_case args). Search/detail:
  `search`, `inspect`, `stats`. Artist: `list_artists`, `reindex_artists`, `set_artist`,
  `set_project_artist`, `set_artist_bulk`. Lists: `get_lists`, `create_list`, `delete_list`,
  `rename_list`, `lists_for_set`, `add_set_to_list`, `remove_set_from_list`. Scan/preview:
  `scan_folder`, `cancel_scan`, `bulk_preview_scan`, `scan_set_folder_previews`, `preview`,
  `open_set`. Export queue: `add_to_export_queue[_bulk]`, `get_export_queue`,
  `toggle_export_queue`, `retriage_jobs`, etc. Watch/suggestions: `*_watch_folder(s)`,
  `*_watch_suggestion(s)`, `create_proxy_set`. The **export worker** is
  `export_worker_loop` (3s poll) + `spawn_job_scoring` (detached triage scoring).
- **`src/App.tsx`** (~2200 lines) — the whole UI: filters bar, results table (with the list
  ★ column), detail pane, player bar, scan progress modal, render queue modal, watch/
  suggestions modal, Manage Lists modal, the floating list popup. **`PlayerBar.tsx`** — canvas
  waveform + click-seek. **`styles.css`** — dark theme.

---

## §7 Invariants & gotchas

These are the rules that, if violated, cause the bugs this project already fought. **Read
before changing related code.**

1. **All Tauri commands are `async`; anything touching disk/db goes in `spawn_blocking`.**
   Sync commands run on the main thread and freeze the window (the "beach-ball incident").
2. **Nothing user-facing ever `await`s slow work.** Enqueue-style commands return immediately
   and emit an event (e.g. `export-queue-updated`) when slow enrichment (triage scoring)
   finishes in a detached task.
3. **Any unbounded filesystem/external work MUST have a budget and narrate progress.** Burned
   three times (auval, queue button, SampleIndex walk). Budgets truncate + log; never hang.
4. **The parser must match the oracle byte-for-byte.** After any `als-core` parser/model
   change, sync `tools/reference_extract.py` and diff (`json` vs the oracle must be empty).
   Artist/lists/previews live in `indexer`/`ops` and are **deliberately kept out of
   `SetSnapshot`** so the oracle is unaffected.
5. **The catalog is incremental & partial.** `prune_missing` is root-scoped so scans of
   different roots accumulate without clobbering. UI/queries treat the catalog as "what's
   indexed so far".
6. **`sets.id` is not stable; `als_path` is.** Re-ingest deletes+reinserts the set row (new
   id). Durable cross-references (lists) key on `als_path`. FK-to-`sets(id)` data (previews,
   tracks…) is intentionally rebuildable.
7. **Files are referenced, never owned.** `remove_preview` deletes the DB row only, never the
   audio file. Discovery never moves files.
8. **Artist precedence:** effective = `COALESCE(sets.artist_override, projects.artist)`. Scan
   writes `projects.artist` only (per-set overrides survive rescans). `upsert_project` uses
   `COALESCE(?, artist)` so a broad rescan that infers nothing won't wipe an explicit artist.
   *Caveat:* a project-level tag can be overwritten by a rescan whose path yields an artist;
   per-set overrides are always safe.
9. **`ON CONFLICT(col)` must match a unique index's collation.** `lists.name` has a
   `COLLATE NOCASE` unique index, so `create_list` is **select-first**, not `ON CONFLICT(name)`
   (that's a cross-version SQLite footgun). Other upserts target default-collation constraints
   and are fine.
10. **FTS rows don't cascade.** Deleting/replacing a set must explicitly delete its `search`
    row (`ingest_set`, `prune_missing` do this).
11. **macOS-specifics:** `user-select` needs the `-webkit-` prefix in Tauri's WKWebView; never
    put side effects in a `setState` updater (StrictMode double-invokes them).

---

## §8 How to make common changes

### Add a search filter
1. `indexer`: add the field to `SearchOpts`; add a `AND (?N IS NULL OR …)` clause to **both**
   SQL branches in `search()` (text + no-text); add the bind to the `params![…]` array (mind
   the positional index); if it should display, add to `SearchHit` + the SELECT (and bump the
   `r.get(n)` indices). 
2. `cli`: add the arg to `Search` + pass it in `cmd_search`'s `SearchOpts`.
3. `app/src-tauri`: add the param to the `search` command + `SearchOpts`.
4. `app/src`: add state, pass it in the `invoke("search", …)` body + the `runSearch` deps,
   render the control, and add it to `anyFilterActive`/`clearFilters` + the active-ring class.

### Add a column / schema version
1. `indexer`: write `const SCHEMA_VN = "ALTER TABLE … ADD COLUMN …;"` (+ index if needed).
2. Bump `SCHEMA_VERSION` to N.
3. In `open()`: add `conn.execute_batch(SCHEMA_VN)?;` to the **fresh-install** block, add a
   gate `if version == N-1 { execute_batch(SCHEMA_VN)?; version = N; }`, and — **only if it's a
   `CREATE … IF NOT EXISTS`** (idempotent) — add it to the tail re-run block. *Never* re-run
   `ALTER` statements blindly.
4. Update every `SCHEMA_V*` list in the `#[cfg(test)]` setups.
5. Existing catalogs migrate in place; new columns are NULL/default until repopulated.

### Add a CLI subcommand
Add a variant to the `Cmd` enum (`crates/cli/src/main.rs`), a match arm in `main()`, and a
`cmd_*` fn. Keep it a thin wrapper over `ops`/`indexer`.

### Add a Tauri command (and use it in the UI)
1. `app/src-tauri`: write an `async fn` with `#[tauri::command(rename_all = "snake_case")]`;
   wrap disk/db work in `spawn_blocking`; **register it in `tauri::generate_handler![…]`**.
2. `app/src`: call `invoke<Ret>("name", { snake_case_args })`; handle errors with `setError`.

### Add an extracted metadata field (from the `.als`)
1. `als-core/model.rs`: add the field to `SetSnapshot` (or a child).
2. `als-core/parser.rs`: extract it (streaming, lenient).
3. **`tools/reference_extract.py`: mirror the extraction exactly**, then diff `json` vs oracle.
4. `indexer`: add a column ([recipe above]) + write it in `ingest_set`; surface in `set_detail`/
   `search` as needed.

### Change render matching / scoring
`previews::matching` (`normalize`, `score`, `best_match`). Keep the disambiguation tests green
(bpm/key are signal, not noise). Triage scoring lives in `ops::triage::renderability`.

---

## §9 File index

```
crates/als-core/src/model.rs     SetSnapshot + child types (oracle-bound)
crates/als-core/src/parser.rs    streaming gzip+XML -> SetSnapshot; SKIP_SUBTREES
crates/als-core/src/scan.rs      discover(); iso_mtime()
crates/previews/src/lib.rs       discover_renders() (RenderFile)
crates/previews/src/matching.rs  normalize/score/best_match
crates/previews/src/peaks.rs     symphonia waveform peak extraction
crates/indexer/src/lib.rs        SCHEMA*, open()/migrations, ingest, search, all CRUD
crates/ops/src/lib.rs            scan_library, harvest/hunt, link suggestions, reindex_artists
crates/ops/src/artist.rs         path -> artist inference (+ tests)
crates/ops/src/triage.rs         renderability, plugin inventory, sample relink, icloud
crates/ops/src/proxy.rs          proxy .als writer (relinked copy)
crates/ops/src/sample_index.rs   one-pass recursive sample lookup index
crates/ops/src/places.rs         Ableton Library.cfg "Places" parser
crates/cli/src/main.rs           ableton-scan binary; Cmd enum; cmd_* wrappers
app/src-tauri/src/lib.rs         Tauri commands + export_worker_loop
app/src/App.tsx                  the entire React UI
app/src/PlayerBar.tsx            waveform player
app/src/styles.css               dark theme
tools/reference_extract.py       THE ORACLE — parser spec/test
tools/export_set.py              macOS UI automation: drive Live's File->Export
ai/PROJECT_STATE.md              running log + handoff snapshots (read first)
ai/AGENTS.md                     rules for contributors (human + AI)
ai/ARCHITECTURE.md               high-level decisions & data flow
ai/CODEBASE_GUIDE.md             this file
```

---

## §10 Build, test, run

```bash
# CLI
cargo build --release
alias ableton-scan=./target/release/ableton-scan
ableton-scan scan "<projects root>"      # index (incremental)
ableton-scan search "korg" --artist burial
ableton-scan stats

# App (Tauri 2 + React; Node 18+)
cd app && npm install && npm run tauri dev

# Tests
cargo test -p indexer    # schema/migrations, search, artist, lists
cargo test -p ops        # artist inference (marker + fallback)
cargo test -p als-core   # parser

# The oracle (MUST be empty after any parser change)
cargo run -p cli -- json example-project-library --pretty > exports/rust.json
python3 tools/reference_extract.py example-project-library --pretty > exports/py.json
diff exports/rust.json exports/py.json
```

The dev sandbox cannot run `cargo` (and its git mount can't delete `.git` lock files) — **all
Rust build/test happens on the user's Mac**. Frontend-only changes (`.tsx`/`.css`) hot-reload
under `npm run tauri dev` without a recompile.
