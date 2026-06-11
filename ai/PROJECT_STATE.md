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
- **Assumption A**: Ableton Extensions SDK can read Live Set metadata. -> **REJECTED** (Reason: Live 12 Suite Beta only).
- **Assumption B**: Ableton Extensions SDK can identify tracks and clips. -> **Unverified**.
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
