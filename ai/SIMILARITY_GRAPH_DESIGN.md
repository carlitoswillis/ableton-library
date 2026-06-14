# Set Similarity Graph — alternative "map" views of the library

DESIGN — v2 (decisions locked 2026-06-13). **Phase 1 SHIPPED & working in-app
(2026-06-13)**: `indexer::load_graph_features` + `ops::similarity::build_graph`
+ Tauri `similarity_graph` + `app/src/SimilarityMap.tsx` (react-force-graph-3d
overlay). Metadata blend only so far (samples/devices/tempo/artist/names); MIDI
key + audio sounds-alike still pending. See PROJECT_STATE.md for the running log
and the performance backlog. Author: handoff agent.
Source of intent: user — "a graph type thing where sets group closer or farther
depending on similarities, colorized… so I can jump around even more with this
large database of sets… alternative views in addition to the long list, for
creative flow… with linked sketches/previews available there too."

**Decisions locked (user, 2026-06-13):**
1. **Default lens = blend**, with **artist as a strong bond**: audio "sounds-alike"
   + shared samples + key (from MIDI) + tempo + artist, plus names/devices. Weights in §3.
2. **Layout = persisted deterministic force-directed** (Phase 1): re-weighting
   re-lays-out live, coords saved per config so it's stable across sessions.
   Optional UMAP "atlas" later.
3. **Render = `react-force-graph-2d`** (canvas lib) for Phase 1; revisit only if 2000 nodes lag.
4. **Audio "sounds-alike" pulled forward** into Phase 1 — it's a core signal, not
   Phase 3. Source is **real bounces only** (discovered renders + worker exports);
   the **sketch renderer is NOT a feature source** — it stays a playback fallback.
   Audio similarity has partial coverage (bounced sets only) and grows as real
   previews accrue.
5. **Sparse sets stay in one map**, parked in a desaturated, toggleable outer
   "halo" (no separate shelf — keep the space whole).

---

## 1. The problem & the goal

Today the library is **one view: a flat list of 2000+ sets**. That's great for
search/triage, bad for *wandering*. The ask is a second, spatial view — a **map
of the catalog** where:

- proximity = similarity (alike sets sit close, different ones drift apart),
- **color** encodes a chosen dimension (cluster, tempo, artist, preview status…),
- you can **pan/zoom and jump** between related ideas instead of scrolling,
- **sketches/previews play right there** (hover/click; on-demand sketch when no
  real bounce exists — reuse the existing `sketch_preview` path),
- it's **additive** — the list view stays; this is a new lens beside it.

Framing it as "creative flow" matters: the win isn't analytics, it's *serendipity*
— "what else lives near this?" So the system should be tunable and explorable,
not a single fixed graph.

## 2. What we can compute TODAY (no new parsing)

The catalog already holds everything a first version needs (see
`crates/indexer` SCHEMA). Per set:

| Signal | Source table | Notes |
|---|---|---|
| **Shared samples** | `samples(path)` → basename | highest-signal: same drum kit / vocal chops / loops ⇒ related work. We already index this (`sample_paths_by_basename`). |
| **Shared devices/plugins** | `devices(name, manufacturer, kind)` | same instruments/FX ⇒ same sonic palette / era. |
| **Tempo** | `sets.tempo`, `sets.tempos_json` | cheap continuous axis. |
| **Names / vocabulary** | `search` fts5 (`set_name, track_names, device_names, sample_names`) | TF-IDF tokens: "808", "vox", artist tags, kit names. |
| **Structure** | `tracks(kind, name, color)` counts; `locators` | track-count, audio/MIDI/group mix, arrangement-marker density. |
| **Artist / project** | `projects.artist`, `sets.artist_override`, `project_id` | strong prior — same project folder ≈ same session lineage. |
| **Time signature / Live version** | `sets.time_signature`, `live_version` | minor lenses / color modes. |
| **Preview/sketch status** | `previews(source, is_primary, peaks, duration)` | node ring + inline playback; `source="sketch"` vs real. |

Most of this is **SQL aggregation over tables we already populate**. Two signals
need a one-time **feature-extraction pass** (no new schema beyond `set_features`,
no Ableton):

| Signal | How we get it | Cost |
|---|---|---|
| **Key / harmonic** | reuse `parse_sketch_data` (already reads every MIDI note) → 12-bin pitch-class histogram per set → Krumhansl-Schmuckler key estimate. Audio-only sets get harmonic info later from chroma (below). | cheap; one parse per set |
| **Audio "sounds-alike"** | **real bounces only** — discovered renders + worker exports in `previews` (a real `audio_path`). Compute a compact fingerprint: chroma(12) + MFCC mean/var + spectral centroid/rolloff/flatness ≈ 40-dim vector. | one fingerprint per real preview, cached by `content_hash` |

**The sketch renderer is NOT a feature source** (user, 2026-06-13). It stays a
playback fallback inside the view only. Fingerprinting an approximation would
pollute the "sounds-alike" space with renderer artifacts (missing FX, wrong
timbres) and make the stopgap load-bearing. So a set only gets an audio vector
once it has a **real** bounce; until then it places by its other signals (samples,
key, tempo, artist, names) and may sit in the sparse "halo" (decision #5). Audio
similarity therefore has **partial coverage** — strong among bounced sets, absent
for un-rendered ideas — which is honest and improves as real previews accrue.

## 3. Similarity model — multi-signal and TUNABLE

Don't pick one definition of "similar." Build a **feature representation per
set**, compute **per-signal similarities**, and **combine with weights the user
controls**. Re-weighting in the UI = morphing the map ("cluster by sample
sharing" → drag → "cluster by tempo/vibe"). That tunability *is* the creative
feature.

Per-signal similarity (each normalized to 0..1):

- **Audio fingerprint cosine** *(sounds-alike)*: cosine over the ~40-dim
  chroma+MFCC+spectral vector (§2). The "does it actually sound similar" signal.
- **Sample Jaccard**: `|A∩B| / |A∪B|` over sample basenames. Production lineage.
- **Key / harmonic**: circle-of-fifths distance between estimated keys, with
  relative major/minor treated as near; or cosine over pitch-class profiles when
  no confident key. (From MIDI; §2.)
- **Tempo kernel**: `exp(-(Δbpm)² / 2σ²)`, σ≈8 BPM, **half/double-time aware**
  (also test Δ against 2× and ½× and take the max — 140 and 70 are "close").
- **Artist/project prior** *(strong bond, per user)*: 1 same artist, 0.5 same
  project tree, else 0.
- **Name cosine**: TF-IDF over tokenized set+track+sample names.
- **Device Jaccard**: over `manufacturer:name` device keys.
- **Structure** *(minor)*: cosine over [n_audio, n_midi, n_group, n_locators, duration].

**Combined** `sim(A,B) = Σ wᵢ·simᵢ(A,B)`, `dist = 1 − sim`. Default weights
(locked blend, artist bonded strongly, audio leading):

`{ audio 0.30, sample 0.20, artist 0.20, key 0.10, tempo 0.10, name 0.07, device 0.03 }`

Defaults cluster a fresh catalog sensibly before the user touches a thing. Any
missing signal contributes 0 and the rest renormalize, so a set with no MIDI
(no key), no samples, or no audio yet still places by whatever it does have —
which is exactly the **sparse-set "halo"** behavior in decision #5. Weights are
UI sliders (Phase 2): dragging them is how you morph the map.

## 4. Similarity → positions + clusters (the pipeline)

1. **kNN graph**: for each set keep its top-k neighbors by combined sim (k≈8–12).
   This is the edge set we draw, and it bounds the layout cost (don't lay out a
   dense 2000×2000 matrix).
2. **Layout to 2D**: two viable engines —
   - *Force-directed* on the kNN graph (springs = sim weight). Intuitive, edges
     visible, runs in-browser (live re-layout on weight change). Good default.
   - *UMAP/PCA* on the feature vectors → stable "atlas" coords. Nicer global
     structure, heavier; run in Rust/precompute. Phase 2+.
3. **Clusters**: community detection (Louvain/Leiden) on the kNN graph, or
   k-means on feature vectors → a cluster id per set for the default coloring.
4. **Persist & cache**: new tables, layout cached by a hash of
   `(weight_config, catalog_content_hash)` so re-opening is instant and only
   re-computes when sets change or weights change.

Proposed tables (additive migration):

```
set_features(set_id PK, sample_sig BLOB, device_sig BLOB, name_tfidf BLOB,
             tempo REAL, struct_vec BLOB, updated_at TEXT)
set_layout(config_hash, set_id, x REAL, y REAL, cluster INT,
           PRIMARY KEY(config_hash, set_id))
set_edges(config_hash, a, b, weight REAL, shared_kind TEXT)  -- for draw + "why"
```

`shared_kind` ("12 shared samples", "same artist") powers an explainable
"why are these near each other?" tooltip — useful for trust and for flow.

Scale note: 2000 nodes × k≈10 ≈ 20k edges — trivial to render on canvas/WebGL and
to force-lay-out in a Web Worker. Feature build is the heavier part (pairwise
sample Jaccard is O(n²) worst case); mitigate with an inverted index
(sample → sets) so we only compare sets that actually share a sample.

## 5. Backend (Rust)

New module `crates/ops/src/similarity.rs` (heavy linear-algebra/clustering could
graduate to its own `crates/graph` crate later):

- `build_features(conn) -> Vec<SetFeatures>` — SQL aggregation, writes `set_features`.
- `compute_layout(conn, config: GraphConfig) -> GraphData` — kNN (via inverted
  index for sample/device, brute force for the rest), layout seed coords,
  Louvain clusters; writes `set_layout`/`set_edges`; returns nodes+edges+clusters.
- `GraphConfig { weights, k, color_mode, layout_engine }` → `config_hash`.

Tauri commands (mirror existing command style in `app/src-tauri/src/lib.rs`):

- `graph_layout(config) -> { nodes:[{set_id,x,y,cluster,color_key,preview_kind,
  name,tempo,artist}], edges:[{a,b,weight,shared_kind}], clusters:[...] }`
- `graph_neighbors(set_id, config) -> [neighbor set_ids + why]` — for "find similar"
  without recomputing the whole graph.
- Playback reuses what already exists: real preview path, or `sketch_preview(set_id)`
  on demand. **No new audio plumbing.**

If in-browser force layout is chosen, the command just returns nodes (no x/y) +
edges and the frontend lays it out; if precomputed (UMAP), it returns x/y.

## 6. Frontend (React) — the view

**Rendering engine — recommend `react-force-graph-2d`** (canvas, handles a few
thousand nodes, built-in zoom/pan/hover/click, custom node paint) for the first
draft. If 2000 nodes + edges feels heavy, step up to **sigma.js** (WebGL, built
for large graphs). A **custom canvas** renderer is also on-brand (the waveform
peaks view is already hand-rolled canvas) and gives full control — but it's more
work; defer unless a lib fights us.

Node/visual encoding:

- **position** = layout; **color** = current color mode (default: cluster).
- **ring/outline** = preview status, reusing the existing rule that sketch
  controls are a *different color* than real previews: e.g. solid ring = real
  bounce, dashed/amber ring = sketch-only, no ring = nothing yet.
- **size** = optional (track count, or duration).
- **label** = appears on zoom-in (semantic zoom) and on hover.

Color modes (toggle, top-of-view control): **Cluster** | **Tempo** (gradient) |
**Artist** | **Primary device family** | **Preview status** | **Live version**.
Same coordinates, different paint — instant lens switching.

Interactions tuned for "jumping around":

- **Hover** → tooltip: name, tempo, artist, mini-waveform (reuse `previews.peaks`).
- **Click** → existing **detail pane** + inline play (real preview, else generate
  sketch). Selecting a node also **highlights its kNN neighbors** and dims the
  rest — the literal "what's near this?" move.
- **Double-click / "recenter"** → animate the camera to that node's neighborhood.
- **Search box** → dims non-matches in place (don't reflow), so you keep spatial
  memory.
- **Lasso / box select** → bulk action: make a **List** (existing `lists` feature)
  or **queue renders** (existing `export_jobs`) from a region. This is where the
  map becomes generative, not just pretty.
- **Weight sliders** (Phase 2) → live re-layout, watch clusters reorganize.

Coexistence: a top-level **view toggle (List ⇄ Map)** sharing the same filter
state, detail pane, and player, so it's genuinely "an additional view," not a
separate app.

## 7. Phasing (smallest valuable thing first)

- **Phase 1 — MVP map (the locked blend):** one feature-extraction pass — sample
  signatures (SQL), **MIDI key/pitch-class** (reuse `parse_sketch_data`), and the
  **audio fingerprint for sets that already have a real bounce** — plus tempo,
  artist, name TF-IDF. Blend with default weights → kNN → **persisted deterministic
  force layout** → color by cluster, toggles for tempo/artist/key/preview-status →
  click opens the existing detail pane and plays the real preview or an on-demand
  sketch (playback only). Sparse sets land in the desaturated **halo**. Ships the
  core "wander the galaxy" feel with sounds-alike already wired (partial coverage).
- **Phase 2 — make it a tool:** device/plugin overlap signal; **weight sliders**
  with live re-layout; lasso → List/render-queue; semantic-zoom labels; minimap;
  layout cache (`set_layout`) keyed by config; "why near?" explanations.
- **Phase 3 — deeper similarity & saved exploration:** upgrade the audio
  fingerprint to a learned embedding model; derive harmonic/key for **audio-only**
  sets from chroma (so non-MIDI bounces get a key); optional stable **UMAP atlas**;
  saved "constellations"/regions; alternative layouts (tempo axis, timeline/recency,
  radial-by-artist). Audio coverage keeps improving as the worker exports real
  bounces — **never** by fingerprinting sketches.

## 8. Open decisions (need your steer — these change the build)

1. **Default lens.** Should the map default to grouping by *shared samples*
   (production lineage), *names/vibe*, or a blend? (Draft: blend, sample-weighted.)
2. **Live re-layout vs fixed atlas.** In-browser force layout = tweak weights and
   watch it move (fun, exploratory) but positions aren't stable run-to-run.
   Precomputed UMAP = a stable "map you learn" but re-weighting is a recompute.
   (Draft: force layout for Phase 1, optional stable atlas later.)
3. **Render lib vs custom canvas.** `react-force-graph-2d` to move fast, or hand-
   rolled canvas to match the peaks view and avoid a dep? (Draft: lib first.)
4. **How early to invest in audio embeddings.** Phase 3 as drafted, or is "sounds
   alike" the whole point and worth pulling forward?
5. **Sparse sets.** Many sets may share no samples/devices (loose ideas) — accept
   they cluster by name/tempo, or surface them as a separate "unconnected" shelf?

## 9. Risks / watch-items

- **Similarity quality tracks data coverage** — if `samples`/`devices` are thin
  for many sets, early clusters lean on names/tempo; validate on the real catalog.
- **Perf** — 2000 nodes is fine; the O(n²) feature step needs the inverted-index
  shortcut (done). **Observed in Phase 1 (user, 2026-06-13): the 3D map slows the
  whole app at times.** Biggest cause was the hidden-but-mounted WebGL render loop
  (fixed: pause on `!visible`). Remaining levers: stop the engine once settled
  (`onEngineStop`/low `cooldownTicks`), `spawn_blocking` + cache the backend
  `GraphData` (recomputes from scratch each call), and bound the snap-to-nearest
  projection (all nodes per mousemove frame). See PROJECT_STATE.md perf backlog.
- **Position instability** can disorient — seed force layouts deterministically
  and persist coords so the map doesn't "jump" between sessions.
- **Scope** — this is a multi-week addition. Phase 1 alone is a real feature; ship
  and feel it before building sliders/embeddings.

## 10. Relationship to existing work

- Reuses: catalog tables, `lists`, `export_jobs`, the preview/peaks pipeline,
  `sketch_preview`, and the detail pane/player. Mostly new code is the
  `similarity` module + the Map view component + the layout tables.
- Complements (doesn't replace) the list view and the preview/sketch system —
  this is the "creative flow" front-end on top of the same catalog.
