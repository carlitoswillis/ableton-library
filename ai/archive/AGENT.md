# AGENT.md

# Ableton Library

## Mission

Build a local-first system that allows users to browse, search, organize, and preview Ableton projects without opening Ableton.

The project should prioritize verified functionality over speculative functionality.

---

## Core Product Vision

A user with hundreds or thousands of Ableton projects should be able to:

- Search projects
- Browse projects
- View metadata
- View project history
- Play previews when available

without launching Ableton Live.

---

## Non-Goals (Current Phase)

Do NOT prioritize:

- AI-assisted mixing
- Vocal chain generation
- Automatic mastering
- Plugin reverse engineering
- PhantomRack-style processing inference
- Machine learning

These may become future features, but are explicitly out of scope until the catalog system is functional.

---

## Engineering Principles

### 1. Verify Before Building

Never assume an Ableton SDK capability exists.

Before implementing a feature:

- verify SDK support
- document findings
- update architecture

---

### 2. Build Useful Layers

Every milestone should produce a usable artifact.

Bad:

"Implemented future infrastructure."

Good:

"Can search 50 projects by BPM."

---

### 3. Prefer Structured Data

Whenever possible:

Store information as structured JSON.

Avoid free-form text.

---

### 4. Catalog First

The project is primarily a cataloging system.

Audio preview generation is secondary.

AI features are tertiary.

---

### 5. Local First

Assume:

- local SQLite
- local filesystem
- local web UI

No cloud dependencies required.

---

## Agent Workflow

When working on a feature:

1. Read PROJECT_STATE.md
2. Confirm current milestone
3. Confirm SDK capability exists
4. Implement smallest useful version
5. Update PROJECT_STATE.md

---

## Definition of Success

A user can:

- Open a browser
- Search an Ableton project
- View project metadata
- Play a preview

without opening Ableton Live.