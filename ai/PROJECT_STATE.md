# Project State

## Current Focus
Phase: UI skeleton (Tauri) — built, awaiting first run on host (2026-06-11)
- [x] `indexer` refactor: `set_detail`/`resolve_set` moved into lib (shared CLI + app); Serialize on SearchHit/Stats.
- [x] `app/src-tauri`: Tauri 2 backend, commands `search`/`inspect`/`stats` (snake_case args) over the shared catalog; bundle inactive (dev-only, no icons needed yet); workspace member.
- [x] `app/` frontend: React 18 + Vite + TS; debounced search, bpm/plugin filters, results table, detail pane (tracks/devices/samples/locators chips), partial-catalog empty state, dark theme.
- [ ] **NEXT (on user's Mac)**: `cd app && npm install && npm run tauri dev` (first run compiles Tauri ~minutes). Fix compile/runtime issues. Verify against indexed fixtures + any real folders indexed so far.
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
- **Repo conventions**: scan JSON outputs go in `exports/` (gitignored); Cargo.lock untracked (user preference; revisit — convention for binary projects is to commit it). (2026-06-11)

## Backlog
- [ ] Automated Live export worker (second Live install + UI automation; see ARCHITECTURE.md Preview Service)
- [ ] Preview archive: keep historical previews per set, potentially anchored to Backup/ timestamps (stretch; pairs with --deep backup parsing)
- [ ] Sample `evicted` state: detect iCloud `.icloud` placeholders vs truly missing files
- [ ] `roots` table + `rescan` subcommand (refresh all previously scanned roots)
- [ ] Automatic key detection
- [ ] Similar project search
- [ ] Plugin inventory
- [ ] Track fingerprints

## Risks
- ~~SDK limitations~~ RETIRED: filesystem-first proven end-to-end.
- ~~Parsing complexity~~ RETIRED: parser validated on ~136 real sets across Live 10.1-11.3.
- iCloud eviction: slows scans, corrupts `exists` signal (backlog: `evicted` state).
- Scope creep (Mitigation: No AI features until catalog exists).
