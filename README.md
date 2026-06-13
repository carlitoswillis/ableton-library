# Ableton Library

Local-first catalog for Ableton Live projects: browse, search, and (eventually) preview your library without opening Live. No Ableton SDK, no cloud — it reads `.als` files (gzipped XML) straight from disk.

**Status**: Milestone 1 (metadata extraction) complete and verified. Milestone 2 (SQLite index) in progress. UI (Tauri) comes later. See `ai/PROJECT_STATE.md` for live status.

## Build & run

Requires Rust 1.79+ (`curl https://sh.rustup.rs -sSf | sh`).

```bash
cargo build --release
alias ableton-scan=./target/release/ableton-scan

# index a library into the SQLite catalog (incremental — rescans only changed files).
# renders found inside project folders are auto-harvested as previews.
ableton-scan scan "<path to your projects root>"
ableton-scan scan "<root>" --force         # re-ingest everything (e.g. after parser upgrades)
ableton-scan scan "<root>" --no-previews   # skip render harvest (avoids iCloud audio downloads)
ableton-scan scan "<artist folder>" --artist "Artist Name"  # tag everything here with this artist

# query it
ableton-scan search "korg"                       # FTS over project/set/track/device/sample names
ableton-scan search --min-bpm 140 --max-bpm 160  # tempo range
ableton-scan search --plugin soothe              # by device/plugin name
ableton-scan search --artist burial              # by (path-derived) artist
ableton-scan artists                             # list every artist + project count
ableton-scan reindex-artists                     # backfill artists from stored paths (no scan)
ableton-scan set-artist 42 "deebo"               # hand-tag one set's artist ("" clears)
ableton-scan set-artist 42 "deebo" --project     # tag the whole project folder instead
ableton-scan inspect 42                          # full detail (by set id or path fragment)
ableton-scan stats

# hunt folders for exported renders, match them to indexed sets by name,
# extract waveform peaks (files are never moved — only referenced)
ableton-scan previews ~/Desktop ~/Downloads "<bounce folders...>"
ableton-scan previews ~/Desktop --verbose      # also list unmatched files
ableton-scan attach "522 idea" ~/Desktop/522-bounce.mp3   # manual match
ableton-scan reset --yes                       # delete the catalog (rebuildable)

# render-worker support tools (M4)
ableton-scan triage "522 idea"                 # renderability report: missing plugins/samples, 0..1 score
ableton-scan triage "522 idea" --show-inventory  # + dump known plugin names (debug false "missing")
ableton-scan rescore                           # recompute scores for all pending render jobs
ableton-scan relink "522 idea"                 # symlink missing samples to located copies (explicit only)
ableton-scan proxy "522 idea"                  # write a relinked proxy .als into the cache dir

# one-shot JSON dump, no database (oracle-compatible output)
# convention: redirect outputs into exports/ (gitignored)
ableton-scan json "<root>" --pretty > exports/library.json
```

The binary is **`ableton-scan`** (declared in `crates/cli/Cargo.toml`; the crate itself is named `cli`). The catalog lives at `~/Library/Application Support/ableton-library/library.db` by default (`--db` overrides).

## How scanning works

- The scanner **recurses to any depth** — mixed structures like `2024/march/artist x/song Project/` are fine. Folder organization doesn't matter to it.
- A **project** is any directory that *directly* contains one or more `.als` files. Each `.als` is a distinct **set** (one project folder can hold several).
- `Backup/` folders (Live's timestamped autosaves) are **not parsed** — they're indexed as lineage (filename, size, mtime) only.
- Extraction is **lenient**: a missing field becomes a `warnings` entry on that set; a corrupt file logs an error and the scan continues. One bad project never aborts a scan.
- Extracted per set: Live version, tempo, time signature, tracks (kind/name/color), devices (native + AU/VST/VST3 with manufacturer), referenced sample paths (with `in_project` / `exists` flags), locators.

## Artist (filing by who made it)

Some of the library is filed by artist rather than by year. The artist isn't in the `.als` file, so it's **derived from the folder path** in two passes over the project's full path:

1. **`artists/` marker (primary).** An `artists/` (or `artist/`) folder anywhere in the path means the next folder is the artist — `…/artists/deebo/dahbby Project/` → **deebo**. This reads the whole path, so it fires no matter where you point the scan: at the library root, at `…/artists/`, or directly at `…/artists/deebo/`.
2. **Positional fallback.** With no marker, it looks at the folders *between* the scan root and the project, skips the temporal/generic ones (`2024`, `march`, `Projects`, …), and takes the first survivor. So `…/2024/march/Burial/Untrue Project/` → **Burial**, while a pure `…/2024/march/Untrue Project/` → no artist (correct).

Already have a catalog? **`ableton-scan reindex-artists`** backfills artists from the paths already on record — no scanning, no re-parsing — or click **Reindex Artists** in the app. To override a wrong/missing guess at scan time, `ableton-scan scan "<folder>" --artist "Name"` tags everything under it (sticky — a later broad rescan won't wipe it). Then filter with `search --artist <name>`, browse with `artists`, or use the app's **artist…** box.

**Tagging by hand.** When the path gives nothing (or the wrong thing), assign an artist directly. Artist is per-**set** with a project fallback — the effective artist is the set's own tag if it has one, otherwise the project's derived one — so two sets in the same folder can have different artists. In the app's detail pane, type a name and hit **Save (this set)** (or **Apply to project** for the whole folder); select rows in the results and use **Tag N** to tag many at once. From the CLI: `set-artist <set> "name"` (add `--project` for the whole folder, `""` to clear). Hand-tagged sets survive rescans (scan/reindex only touch the project's derived artist, never a per-set override).

## Lists & favorites (desktop app)

Sets can be organized into **lists** — a set can belong to many at once, and "favorites" is just a list you name however you like. In the results view, each row has a **★ star** on the left: hollow when the set is in no list, filled when it's in at least one. Click it to open a little picker — check/uncheck any existing list, or type a name and **Create** to make a new one (which adds the set to it). The **All lists ▾** filter next to the search box narrows the results to one list.

The **All lists ▾** filter next to the search box narrows results to one list, and the **⚙** button beside it opens **Manage Lists** — rename a list inline, delete one (with a confirm step), or create new ones.

Membership is stored by the set's path, not its database row, so your lists **survive rescans** (re-ingesting a changed set won't drop it from your lists). Deleting a list just removes the grouping — never the sets or files.

## Scanning iCloud folders — read first

If your projects live in iCloud Drive with "Optimize Mac Storage" on, some files may be **evicted** (cloud-only placeholders). Reading them forces a download — a full scan can trigger a large sync, and evicted files that can't download in time may error or hash incorrectly.

Before a big scan:

```bash
# check for cloud-only placeholders (they look like ".song.als.icloud")
find "<icloud projects root>" -name "*.icloud" | head
```

If that prints anything, those files aren't local yet — download them first (Finder → select the folder → right-click → Download Now), then scan. Start with a subfolder (one year, one artist) before scanning the whole tree.

## Desktop app (Tauri)

A native browser over the same catalog the CLI writes. Requires Node 18+.

```bash
cd app
npm install
npm run tauri dev    # first run compiles the Tauri backend — takes a few minutes
```

The app reads `~/Library/Application Support/ableton-library/library.db` — index something with `ableton-scan scan <folder>` first. The catalog is treated as partial by design: scan folders piecemeal and the app shows whatever's indexed so far.

**Attaching a preview by hand**: auto-matching leans on bounces named like the set/project — a good default, but not always how it goes. In the detail pane, **Attach Audio…** lets you pick *any* audio file as that set's preview (referenced in place, never moved; it becomes the primary). The CLI equivalent is `ableton-scan attach <set> <audio>`.

**Render queue (Auto-Export)**: the app can render previews for sets that have none by driving a real Ableton Live install via UI automation (`tools/export_set.py`). Live opens and runs **in the foreground** — don't touch mouse/keyboard during a render. Sets are triaged first (missing plugins/samples = lower score, easy sets render first), missing samples are relinked into a temporary proxy copy of the set (your `.als` is never modified), and finished renders attach as previews with an honest "what was missing" fidelity note.

## Repository layout

```
crates/als-core/            # parser lib: gzip + streaming XML -> SetSnapshot; discovery
crates/indexer/             # SQLite + FTS5 catalog (shared by CLI and app)
crates/cli/                 # the ableton-scan binary
app/                        # Tauri 2 + React desktop app (src-tauri/ = Rust side)
tools/reference_extract.py  # executable spec / test oracle for als-core
ai/                         # project state, architecture, agent rules (start here)
example-project-library/    # local test fixtures (gitignored)
```

## Development workflow

`tools/reference_extract.py` is the **test oracle**: the Rust parser must produce byte-identical JSON. After any parser change:

```bash
cargo run -p cli -- json example-project-library --pretty > exports/rust.json
python3 tools/reference_extract.py example-project-library --pretty > exports/py.json
diff exports/rust.json exports/py.json   # must be empty
```

Project state, decisions, and constraints are tracked in `ai/` and preserved via git — read `ai/AGENTS.md` before contributing (human or AI).
