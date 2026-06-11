# Ableton Library

Local-first catalog for Ableton Live projects: browse, search, and (eventually) preview your library without opening Live. No Ableton SDK, no cloud — it reads `.als` files (gzipped XML) straight from disk.

**Status**: Milestone 1 (metadata extraction) complete and verified. Milestone 2 (SQLite index) in progress. UI (Tauri) comes later. See `ai/PROJECT_STATE.md` for live status.

## Build & run

Requires Rust 1.79+ (`curl https://sh.rustup.rs -sSf | sh`).

```bash
cargo build --release

# scan a library, human summary on stderr, JSON on stdout
./target/release/ableton-scan "<path to your projects root>" --pretty > library.json

# or via cargo during development
cargo run -p cli -- example-project-library --pretty
```

The binary is **`ableton-scan`** (declared in `crates/cli/Cargo.toml`; the crate itself is named `cli`).

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
cargo run -p cli -- example-project-library --pretty > rust.json
python3 tools/reference_extract.py example-project-library --pretty > py.json
diff rust.json py.json   # must be empty
```

Project state, decisions, and constraints are tracked in `ai/` and preserved via git — read `ai/AGENTS.md` before contributing (human or AI).
