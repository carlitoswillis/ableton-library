# Agent Guidelines (AGENTS.md)

PURPOSE: This is the authoritative rulebook for AI assistants. It defines the 'how' and 'what' of the codebase.

## Project Context
- **Objective**: Build a local-first system to browse, search, organize, and preview Ableton projects without opening Ableton Live.
- **Stack (decided 2026-06-11)**: Rust core + Tauri 2 desktop shell + React 18/TS frontend + SQLite (rusqlite + FTS5). CLI-first development: core logic validated via CLI before UI integration.
- **Working style**: User is NOT writing Rust — AI writes all code, user compiles/tests on their Mac and gives product feedback. The sandbox cannot run cargo; ALL Rust verification happens on the user's machine.

## Architecture Constraints
- **No Ableton SDK dependency**: User runs Live 11; the Extensions SDK (Live 12 Suite beta only) is off the table. Filesystem-first is the strategy, not a fallback.
- **Version tolerance (backward + forward)**: Parser must handle .als files from older Live versions (9/10/11) and newer ones (12+). Extract leniently — skip unknown elements, never hard-fail on schema drift, record the Live version (Creator attribute) per set.
- **Crate layering**: `als-core` + `previews` → `indexer` (storage) → `ops` (workflows) → `cli` / `app` (frontends). Never import a frontend crate from a library crate.
- **Database/Persistence**: SQLite in app data dir (`~/Library/Application Support/ableton-library/library.db`). Catalog is always fully rebuildable from `.als` files. Never store DB inside user project folders.
- **Markdown Persistence**: All project state must be tracked in `/ai`.
- **Local First**: Assume local filesystem and no cloud dependencies.
- **Incremental catalog**: Never assume the catalog is complete — user scans subfolders piecemeal. UI and queries treat the catalog as "what's been indexed so far".

## Coding Conventions
- **Explicit over Implicit**: Avoid hidden logic, reflection, or complex inheritance.
- **Verification First**: All changes must be verified via tests and project-specific validation scripts. Keep `tools/reference_extract.py` in sync with any `als-core` parser change.
- **Compact Context**: Keep context files task-scoped and minimal.
- **Async + spawn_blocking**: ALL Tauri commands must be `async`. Any command touching disk/db goes in `spawn_blocking`. (Learned from beach-ball incident — sync commands run on main thread.)
- **Multi-threading pattern**: CPU-bound batch work (`.als` parsing, audio peak extraction) uses `std::thread::scope` with worker threads funneling results to main thread for sequential SQLite writes.
- **Interleave scan + harvest**: When scanning a library, preview harvesting happens per-project immediately after that project's sets are ingested — never as a separate bulk pass. `known_samples` (sample cross-check) is built incrementally, not queried in bulk after commit.
- **Export worker**: Automated Live export uses macOS UI automation (`tools/export_set.py`). Serialize one render at a time; treat Live as flaky (timeouts, retry once, mark failed rather than wedging queue).

## How to Navigate This Workspace (Priority Flow)
To minimize token waste and maximize focus, follow this priority sequence:
1. **START HERE**: Read `PROJECT_STATE.md`. It defines the current high-level objective and active milestones.
2. **Operational Rules**: Read `AGENTS.md` (this file). Adhere strictly to these constraints.
3. **Architecture Details**: Read `ARCHITECTURE.md` to understand the system components and data flow.
4. **Self-Correction**: If you feel your understanding of the project state is out of sync, you may run `./ai/ai-context.sh` to refresh your local context bundle.
