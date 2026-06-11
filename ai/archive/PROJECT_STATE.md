PROJECT_STATE.md

# Current Status

Phase: Planning

Date: 2026-06-11

---

# Product Definition

Ableton Library

A searchable catalog of Ableton projects with metadata and audio previews.

Primary Goal:

Reduce friction when revisiting old music projects.

---

# Current Assumptions

Assumptions are not facts.

They must be validated.

Assumption A:

Ableton Extensions SDK can read Live Set metadata.

Status:
REJECTED

Shift -> 

Filesystem Scanner (.als + project folders)
        ↓
Metadata Extractor
        ↓
SQLite Catalog
        ↓
Web UI

Ableton integration layer (future, optional)

Assumption B:

Ableton Extensions SDK can identify tracks and clips.

Status:
Unverified

Assumption C:

Automated preview generation may be possible.

Status:
Unverified

---

# MVP Milestone 1

Project Metadata Extraction

Success Criteria:

Given an open Ableton project:

Generate:

{
  "projectName": "...",
  "tempo": 120,
  "tracks": [...]
}

Output saved locally.

---

# MVP Milestone 2

Project Catalog

Success Criteria:

User can:

- View projects
- Search projects
- Sort projects

without opening Ableton.

---

# MVP Milestone 3

Preview Integration

Success Criteria:

Project page displays:

- metadata
- waveform
- audio preview

---

# Risks

Risk 1:

SDK limitations.

Mitigation:

Prototype immediately.

---

Risk 2:

Preview generation complexity.

Mitigation:

Support manual preview association initially.

---

Risk 3:

Scope creep.

Mitigation:

Do not build AI features before catalog functionality exists.

---

# Immediate Next Task

Install Ableton Extensions SDK.

Create the smallest possible extension.

Determine:

- available metadata
- output format
- extension execution model

Document findings.

Update this file after validation.

---

# Previous Assumption:

Extensions SDK is foundation
  Status: Rejected.
  Reason: Live 12 Suite Beta only.

New Foundation: Filesystem-first architecture.
  Goal:
    Support Live 11+
    Support future versions
    Support all editions

# Parking Lot

Ideas worth revisiting later:

- Automatic key detection
- Similar project search
- Plugin inventory
- Track fingerprints
- Mix analytics
- Vocal processing analysis
- PhantomRack-style chain suggestions

None are active development targets.