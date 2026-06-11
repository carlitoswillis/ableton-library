# Project State

## Current Focus
Phase: Milestone 1 — Metadata Extraction (2026-06-11)
- [x] Pivot to Filesystem-first architecture (Live 11+ support).
- [x] Select technology stack -> **Rust core + Tauri 2 shell + React/TS frontend + SQLite** (decided 2026-06-11; owner learning Rust alongside).
- [x] Scaffold Cargo workspace (crates/als-core, crates/cli; indexer + app/ deferred).
- [x] Implement `als-core`: gzip (flate2) + streaming XML (quick-xml) -> SetSnapshot JSON.
- [x] Implement `cli` (`ableton-scan`): scan folder of projects, emit snapshots.
- [x] Validate extraction logic against real .als fixtures -> via `tools/reference_extract.py` (executable spec / test oracle; all 5 sets, 0 warnings).
- [ ] **NEXT (on user's Mac)**: `cargo build`, run `ableton-scan example-project-library --pretty`, diff JSON against the Python oracle, fix any compile/output drift. (Sandbox cannot install Rust toolchain — network allowlist blocks rustup/static.rust-lang.org.)
- [ ] Implement `indexer` crate (SQLite + FTS5).

## Current Assumptions & Validations
- **Assumption A**: Ableton Extensions SDK can read Live Set metadata. -> **REJECTED** (Live 12 Suite Beta only; user is on Live 11). SDK is permanently off the table — filesystem-first is the strategy, not a fallback.
- **Assumption B**: Ableton Extensions SDK can identify tracks and clips. -> **MOOT** (SDK ruled out per Assumption A).
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
- Gap: all fixtures are 11.3.43; need older-era .als for backward-compat testing.
- **Assumption C**: Automated preview generation may be possible. -> **VALIDATED in principle**: owner previously scripted a second Live install to open + export sets via macOS UI automation. Previews = pluggable source interface: discovery (MVP) -> automated Live export worker (post-catalog). **Unverified** whether Live 12 desktop writes preview audio on save.

## Active Milestones
- **Milestone 1: Metadata Extraction**: Generate structured output from .als files (Gzip/XML parsing).
- **Milestone 2: Project Catalog**: Browse, search, and sort projects locally.
- **Milestone 3: Preview Integration**: Display metadata, waveform, and audio preview.

## Decisions
- **Backups**: lineage-only indexing (filename, timestamp, size); full parse behind a `--deep` flag later. (2026-06-11)
- **Snapshot schema**: SetSnapshot/ProjectSnapshot as defined in als-core (version, tempo, time sig, tracks, devices, samples, locators, warnings). Approved 2026-06-11.

## Backlog
- [ ] Automated Live export worker (second Live install + UI automation; see ARCHITECTURE.md Preview Service)
- [ ] Preview archive: keep historical previews per set, potentially anchored to Backup/ timestamps (stretch; pairs with --deep backup parsing)
- [ ] Automatic key detection
- [ ] Similar project search
- [ ] Plugin inventory
- [ ] Track fingerprints

## Risks
- SDK limitations (Mitigation: Filesystem-first approach).
- Parsing complexity (.als files are gzipped XML).
- Scope creep (Mitigation: No AI features until catalog exists).
