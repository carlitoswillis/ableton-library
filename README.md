# Ableton Library

Local-first catalog for Ableton Live projects: browse, search, and (eventually) preview your library without opening Live. No Ableton SDK, no cloud — it reads `.als` files (gzipped XML) straight from disk.

**Status**: Milestone 1 (metadata extraction) complete and verified. Milestone 2 (SQLite index) in progress. UI (Tauri) comes later. See `ai/PROJECT_STATE.md` for live status.

## Build & run

Requires Rust 1.79+ (`curl https://sh.rustup.rs -sSf | sh`).

```bash
cargo build --release
alias ableton-scan=./target/release/ableton-scan

# index a library into the SQLite catalog (incremental — rescans only changed files)
ableton-scan scan "<path to your projects root>"

# query it
ableton-scan search "korg"                       # FTS over project/set/track/device/sample names
ableton-scan search --min-bpm 140 --max-bpm 160  # tempo range
ableton-scan search --plugin soothe              # by device/plugin name
ableton-scan inspect 42                          # full detail (by set id or path fragment)
ableton-scan stats

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

## Scanning iCloud folders — read first

If your projects live in iCloud Drive with "Optimize Mac Storage" on, some files may be **evicted** (cloud-only placeholders). Reading them forces a download — a full scan can trigger a large sync, and evicted files that can't download in time may error or hash incorrectly.

Before a big scan:

```bash
# check for cloud-only placeholders (they look like ".song.als.icloud")
find "<icloud projects root>" -name "*.icloud" | head
```

If that prints anything, those files aren't local yet — download them first (Finder → select the folder → right-click → Download Now), then scan. Start with a subfolder (one year, one artist) before scanning the whole tree.

## Repository layout

```
crates/als-core/            # parser lib: gzip + streaming XML -> SetSnapshot
crates/cli/                 # the ableton-scan binary
crates/indexer/             # (next) SQLite + FTS5 catalog
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
