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
- [ ] **NEXT (user's test plan)**: dump db (`ableton-scan reset --yes`), bounce some current-year tracks into one folder, `scan` the matching projects + `previews` that folder, evaluate match quality from a controlled sample. NO full-system hunt (user explicitly declined).
- [ ] Later in M3: previews in detail pane — list ALL of a set's previews, switch primary, manual attach/replace from the UI (user asked "what if i want to update the preview?": re-bounce to same path = auto-replaced on rescan via mtime; new file = new row, newest wins at equal confidence; `attach` = manual trump at 1.0 — UI affordance for all this still missing). Also: historical preview archive, in-app "hunt for previews" UI.
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
- [x] Automated Live export worker (second Live install + UI automation; see ARCHITECTURE.md Preview Service) [cmd + a to select all in arragement view, cmd + r then click export [or hit enter], but if there is no arrangement (usually there is so this is rare) like if nothing is there, or we are playing from session view then export some or all of the session view scences/rows]
- [ ] Handle overwrite/replace dialogs gracefully during automated render queue exports (e.g. if file deletion fails or other conflicts occur)
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
