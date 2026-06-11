# Project State

## Current Focus
Phase: Milestone 1 — Metadata Extraction (2026-06-11)
- [x] Pivot to Filesystem-first architecture (Live 11+ support).
- [x] Select technology stack -> **Rust core + Tauri 2 shell + React/TS frontend + SQLite** (decided 2026-06-11; owner learning Rust alongside).
- [ ] Scaffold Cargo workspace (crates/als-core, crates/indexer, crates/cli; app/ deferred).
- [ ] Implement `als-core`: gzip (flate2) + streaming XML (quick-xml) -> ProjectSnapshot JSON.
- [ ] Implement `cli` (`ableton-scan`): scan folder of projects, emit snapshots.
- [ ] Validate parser against real .als files from user's library.

## Current Assumptions & Validations
- **Assumption A**: Ableton Extensions SDK can read Live Set metadata. -> **REJECTED** (Live 12 Suite Beta only; user is on Live 11). SDK is permanently off the table — filesystem-first is the strategy, not a fallback.
- **Assumption B**: Ableton Extensions SDK can identify tracks and clips. -> **MOOT** (SDK ruled out per Assumption A).
- **Constraint**: Parser must be version-tolerant across Live versions, backward (9/10/11) and forward (12+). Lenient extraction; never hard-fail on schema drift.

## Format Findings (2026-06-11, from /example-project-library — Live 11.3.43)
- Root: `<Ableton Creator="Ableton Live 11.3.43" MinorVersion="11.0_11300">` -> version branching trivial.
- Tracks: typed elements (MidiTrack/AudioTrack/ReturnTrack/Group), names in `EffectiveName` — but `EffectiveName` also exists on devices, so names MUST be scoped by parent path (validates streaming parser w/ path stack).
- Tempo: `<Tempo><Manual Value="...">` (location differs in older versions — verify when older fixture available).
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

## Backlog
- [ ] Automated Live export worker (second Live install + UI automation; see ARCHITECTURE.md Preview Service)
- [ ] Automatic key detection
- [ ] Similar project search
- [ ] Plugin inventory
- [ ] Track fingerprints

## Risks
- SDK limitations (Mitigation: Filesystem-first approach).
- Parsing complexity (.als files are gzipped XML).
- Scope creep (Mitigation: No AI features until catalog exists).
