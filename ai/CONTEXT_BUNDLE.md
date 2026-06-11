# AI Context Bundle
Generated: Thu Jun 11 15:27:04 PDT 2026

## ⚠️ Agent Navigation Guide
1. Start with the **Current State** below to understand the focus.
2. Check **Active Tasks** for your specific assignment.
3. Only read files from the repository structure that are directly related to those tasks.
4. Do NOT perform full repository scans unless the task is an architectural audit.

## 1. Authoritative Rules (AGENTS.md)
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

## 2. Architecture (ARCHITECTURE.md)
# Architecture

PURPOSE: Technical system design and data flow of the Ableton Library application.

## Overview
Ableton Library is a metadata and preview indexing system for Ableton projects, allowing users to browse and search their library without opening Ableton Live.

## System Components

### 1. Filesystem Scanner (.als + project folders)
- **Purpose**: Extract project information from Live Sets and folders.
- **Status**: Pivot from Extension-based to Filesystem-first.
- **Responsibilities**: Metadata extraction, XML/Gzip parsing (.als), and normalization.

### 2. Metadata & Indexing Service
- **Purpose**: Persist and query project information.
- **Options**: 
  - Relational (SQLite) for structured queries.
  - Document-based (JSON/Embedded DB) for flexibility.
- **Responsibilities**: Receive metadata, normalize records, store project snapshots.

### 3. Preview Service
- **Purpose**: Associate audio previews with projects.
- **Responsibilities**: Detect preview files, store preview metadata, generate/cache waveform data.

### 4. User Interface
- **Purpose**: Browse and search projects.
- **Options**: Web-based (React/Vite), Desktop-native (Tauri/Rust), or CLI.
- **Views**: Library View (Search/Filters), Project Detail View (Metadata/Tracks/Player).

## Data Flow
Filesystem (.als) -> Extraction Logic -> Indexing Service -> Local Storage -> UI Layer

## AI Workspace Substrate
This repository uses an AI-assisted engineering substrate located in `/ai`
- **Cognition Layer**: State and tasks are tracked in `/ai`.
- **Rules**: Agent constraints are defined in `AGENTS.md`.
- **Flow**: Human Pilot -> AI Implementation -> Deterministic Verification.

## 3. Project State (PROJECT_STATE.md)
# Project State

## Current Focus
Phase: Planning (2026-06-11)
- [ ] Pivot to Filesystem-first architecture (Live 11+ support).
- [ ] Research/Select technology stack (Go vs Rust vs Node for extraction).
- [ ] Implement Metadata Extraction MVP.

## Current Assumptions & Validations
- **Assumption A**: Ableton Extensions SDK can read Live Set metadata. -> **REJECTED** (Reason: Live 12 Suite Beta only).
- **Assumption B**: Ableton Extensions SDK can identify tracks and clips. -> **Unverified**.
- **Assumption C**: Automated preview generation may be possible. -> **Unverified**.

## Active Milestones
- **Milestone 1: Metadata Extraction**: Generate structured output from .als files (Gzip/XML parsing).
- **Milestone 2: Project Catalog**: Browse, search, and sort projects locally.
- **Milestone 3: Preview Integration**: Display metadata, waveform, and audio preview.

## Backlog
- [ ] Automatic key detection
- [ ] Similar project search
- [ ] Plugin inventory
- [ ] Track fingerprints

## Risks
- SDK limitations (Mitigation: Filesystem-first approach).
- Parsing complexity (.als files are gzipped XML).
- Scope creep (Mitigation: No AI features until catalog exists).

## 4. Repository Structure
```text
.
./ai [old]
./ai [old]/AGENT.md
./ai [old]/ARCHITECTURE.md
./ai [old]/PROJECT_STATE.md
./ai
./ai/ai-context.sh
./ai/ARCHITECTURE.md
./ai/CONTEXT_BUNDLE.md
./ai/PROJECT_STATE.md
./ai/AGENTS.md
```

## 5. Recent Git Changes (Summary)
```text
No git history yet.
```

## 6. Active Diff
```diff
```
