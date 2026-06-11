# ARCHITECTURE.md

# System Overview

Ableton Library is a metadata and preview indexing system for Ableton projects.

The system consists of four primary components.

---

## Component 1: Ableton Extension

Purpose:

Extract project information from Live Sets.

Responsibilities:

- Read current Live Set
- Extract metadata
- Export structured JSON

Example Output:

{
  "projectName": "Summer Beat 4",
  "tempo": 145,
  "tracks": [
    {
      "name": "Lead",
      "type": "midi"
    }
  ]
}

Status:

Prototype

Unknowns:

- Exact SDK metadata availability
- Export workflow support
- Project path access

---

## Component 2: Metadata Service

Purpose:

Persist project information.

Technology:

- Node.js
- SQLite

Responsibilities:

- Receive metadata exports
- Normalize records
- Store project snapshots

Tables:

projects

- id
- name
- path
- bpm
- created_at
- updated_at

project_snapshots

- id
- project_id
- snapshot_json
- created_at

---

## Component 3: Preview Service

Purpose:

Associate audio previews with projects.

Responsibilities:

- Detect preview files
- Store preview metadata
- Generate waveform data

Potential Sources:

1. Manual export
2. Extension-assisted export
3. Automated export workflow

Unknown:

Whether automated rendering is practical.

---

## Component 4: Web Application

Purpose:

Browse and search projects.

Technology:

- React
- Vite
- TypeScript

Views:

Library View

- Search
- Filters
- Sorting

Project Detail View

- Metadata
- Track list
- Preview player
- History

---

# Data Flow

Ableton Live
    ↓
Extension
    ↓
JSON Export
    ↓
Metadata Service
    ↓
SQLite
    ↓
Web UI

Optional:

Preview Audio
    ↓
Preview Service
    ↓
SQLite
    ↓
Web UI

---

# Future Architecture

Not currently prioritized.

Potential future modules:

- Similarity Search
- Key Detection
- Genre Classification
- Mix Analysis
- Vocal Chain Analysis
- PhantomRack-style Recommendations

These are intentionally deferred.