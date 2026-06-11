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
