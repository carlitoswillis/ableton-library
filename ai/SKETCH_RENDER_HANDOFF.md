# Sketch / Approximate Preview Renderer — Handoff

PURPOSE: everything needed to (1) port the validated Python prototype
`tools/sketch_render.py` to Rust and (2) wire it into the app UI as an
on-demand **fallback preview**. Written for handoff — a new engineer (or future
AI) should not need to re-derive the `.als` structure or the algorithm.

Status (2026-06-13): **Python prototype BUILT and validated** on the example
library; logic is the source of truth. **Rust port not started.** UI wiring not
started.

---

## 1. What it is / why

A fast, **no-Ableton** "sketch" of how a set sounds, to show when a set has no
real bounce/preview. It is an explicitly **approximate** fallback (ARCHITECTURE
§3 source d) — never presented as a real render. Live remains the only faithful
renderer (sources a–c).

Reframe that made it viable: the user's MIDI "drum" tracks aren't abstract —
each is a Simpler/Sampler loaded with a **real sample**. So MIDI notes trigger
the actual sample (repitched), not a synth. Generic synthesis is only a
fallback for true synths (Analog/Operator/3rd-party) with no sample.

## 2. Behavior (what the prototype does)

Input: a `.als` path (+ a library root to relink samples across). Output: a
normalized stereo `.wav`, ≤ 60 s by default.

- Streams the gzipped `.als` XML once (no DOM).
- Collects **arrangement** clips only (session/ClipSlot clips excluded).
- **Audio clips**: resolve sample → decode → place at `CurrentStart` (beats→sec
  via tempo) → content offset from Loop/warp → SampleVolume + clip fades → sum.
- **MIDI clips**: parse note events; each note triggers the owning track's
  **instrument sample** (Simpler `MultiSamplePart`), repitched by `note−RootKey`;
  **generic synth fallback** if the track has no sample part.
- Honors **track mute** + **per-clip Disabled** (user: "always respect mutes").
- Mirrors the user's **mix**: per-track mixer Volume (linear gain) applied.
- **Overlap resolution**: one clip per track at a time (truncate each clip at
  the next clip's start on its track) — kills doubled/take-lane stacks that
  otherwise double-trigger notes ("glitchy, no grid").
- **De-click**: short raised-cosine edge fades on every voice/clip.
- Normalize to −1.5 dBFS.

## 3. `.als` structure reference (hard-won — trust this)

`.als` = gzip( XML ). Stream with a tag stack. Key facts:

- **Tempo**: `…Tempo > Manual @Value`. Prefer the one under `MainTrack`
  (Live 12) / `MasterTrack` (≤11); else first. Beats→sec: `sec = beat * 60/bpm`.
- **Tracks** are siblings under `<Tracks>` (NOT nested; group membership is via
  `TrackGroupId`, not XML nesting). Tags: `AudioTrack|MidiTrack|GroupTrack|ReturnTrack`.
  - **name**: `Track > Name > EffectiveName @Value`.
  - **mute**: `Track > DeviceChain > Mixer > Speaker > Manual @Value` — `false`
    = MUTED. (Path-qualify; nested device mixers also have a `Speaker`.)
  - **volume**: `Track > DeviceChain > Mixer > Volume > Manual @Value` — linear
    gain, `1.0` = 0 dB, can exceed 1.
  - **solo**: NOT reliably found. User confirms it PERSISTS, but no example set
    was saved with a solo, so the field is unconfirmed. `Solo`/`IsSoloed` seen
    in the data are Simpler-samplepart / rack-branch solos, NOT track solo.
    Candidate: track-mixer `SoloSink`. **TODO: get a set saved with a track
    solo'd, diff, wire it.** The renderer already has the audible/solo filter
    plumbed; only the parse of the field is missing.
- **Clips**: `AudioClip` / `MidiClip`. Exclude any with `ClipSlot` in the
  ancestor stack (those are session clips, not the arrangement).
  - Position: `CurrentStart` / `CurrentEnd` (arrangement BEATS).
  - `Loop > {LoopStart, LoopEnd, LoopOn}` (content beats); `Disabled @Value`
    (clip mute).
  - Audio clip sample: `SampleRef > FileRef > {Path (abs), RelativePath}`.
    `SampleVolume`, `Fades > {FadeInLength, FadeOutLength}` (beats), `IsWarped`,
    `WarpMarkers > WarpMarker @SecTime @BeatTime`.
  - MIDI notes: `MidiClip > Notes > KeyTracks > KeyTrack > { Notes >
    MidiNoteEvent @Time @Duration @Velocity @IsEnabled ; MidiKey @Value }`.
    `MidiKey` is the PITCH for all notes in that KeyTrack; it appears AFTER the
    note list, so buffer notes per KeyTrack and stamp the key on close. Note
    `Time` is **clip-relative** (verified). Skip `IsEnabled="false"`.
- **Instrument sample map** (Simpler/Sampler — gives MIDI its real sound):
  `OriginalSimpler|MultiSampler > … > MultiSampleMap > SampleParts >
  MultiSamplePart > { KeyRange{Min,Max}, RootKey, SampleStart, SampleEnd,
  SampleRef>FileRef>{Path,RelativePath} }`. A part may have an empty Path
  (placeholder) — skip those. Instruments may sit inside an
  `InstrumentGroupDevice` rack (multiple parts/layers); collect all, match a
  note to parts whose `KeyRange` covers its pitch.

## 4. Algorithm details (per the prototype)

- **Sample resolution / relink** (`resolve_sample` + `build_sample_index`):
  abs `Path` → `project_dir`/`RelativePath` → **library-wide basename index**
  (walk a library root once; skip `/Backup`) → walk project dir. Mirrors the
  exporter's cross-project relink. In Rust, **reuse `ops::sample_index` /
  `ops::places`** (the real relink that indexes Ableton Places/Core/User
  Library) instead of this basename walk.
- **Audio placement**: `start = CurrentStart*spb`; slot length =
  `(eff_end − CurrentStart)*spb` (eff_end from overlap resolution); read
  `slot` seconds of content from the offset; ×(SampleVolume × trackVol); clip
  fades; de-click; sum. **No warp time-stretch** (plays native rate) — the open
  fidelity gap.
- **MIDI → instrument sample**: for each note, semitones = `pitch − RootKey`;
  repitch the part's `[SampleStart:SampleEnd]` region by playback-rate change
  (`ratio = 2^(semi/12)`, linear resample — up = shorter/higher, like Simpler
  Classic). Cache decoded base per (sample,start,end) and pitched voice per
  (…,semitone) — drums repeat the same pitch, so this is the speed win. ×(vel/127
  × 0.6 × trackVol). One-shot (full sliced sample), not gated to note length.
- **Synth fallback** (`synth_note`): tonal = 3 sine harmonics + quick-attack
  exp decay; `perc` (snare/clap/hat/…) = noise burst; `kick` = pitch-drop sine.
  Chosen by track-name keywords. (808-named → tonal, correct: 808 is pitched.)
- **808**: NO auto-tuner (built then removed — user: "replacements sound dumb").
  808 sample just repitches by `note−RootKey` like any instrument.
- **Overlap resolution** (`resolve_overlaps`): per track, sort by (start, doc
  order); each clip's `eff_end = min(end, next.start)`; same-start dupes collapse
  to the last in doc order. Audio uses eff_end for slot length; MIDI drops notes
  with `arr_beat ≥ eff_end`.
- **De-click** (`declick`): raised-cosine fade, ~1.5 ms attack (keep transients)
  / ~5 ms release, on every placed buffer (synth, cached instrument voice, audio
  segment). All voice edges verified == 0.
- **Loop tiling**: MIDI clip with `LoopOn` tiles the `[LoopStart,LoopEnd]` region
  across `[CurrentStart,CurrentEnd]`.

## 5. Validation status

- Verified in-sandbox on example library: parsing, instrument-part extraction
  (correct sample names per track), overlap trim (king st MIDI 1498→770, big guy
  2582→816), repitch exactness (+12 st → exactly half length), 60 s render in
  ~1–1.8 s, de-click edges == 0, mute/volume applied.
- **NOT verifiable in-sandbox**: the actual *sound* of the real drum/instrument
  samples — the example library doesn't bundle them (they live in the user's
  Ableton/User library). On the host with real sample folders indexed,
  `real-sample` count > 0. **Needs a host run to ear-check drum fidelity.**

## 6. Open items

- **Warp time-stretch** (biggest remaining fidelity gap): audio chops play
  native-rate; heavily warped clips drift. Implement via `WarpMarker`
  (SecTime↔BeatTime) piecewise mapping + time-stretch to the slot.
- **Track solo** parse (see §3) — needs a solo'd example `.als`.
- **m4a/mp3** decode: stdlib can't (prototype limit); symphonia handles it in
  the Rust port.
- **Take lanes**: overlap resolution handles the symptom; a cleaner model would
  read the active comp explicitly.
- Possible knobs the user may want: per-track vs overall normalize; attack-ramp
  length; whether one-shots should gate to note length for sustained instruments.

## 7. Rust port plan

- **Where**: a new module in `crates/previews` (it already owns decode + peaks
  via symphonia), e.g. `previews::sketch`. Pure library code; no frontend deps.
  Add a thin workflow entry in `ops` if it needs DB (it can run purely from the
  `.als` path + a relink index).
- **Reuse**: symphonia for decode (replaces stdlib aifc/wave; fixes m4a/mp3);
  `ops::sample_index` + `ops::places` for the real cross-library relink (replaces
  `build_sample_index`); existing peaks extractor for the resulting wav.
- **Parsing**: do NOT extend `als-core::parse_set` (it's locked to the
  `tools/reference_extract.py` oracle and deliberately SKIPS Notes/Events). Write
  a SEPARATE pass (own iterator over the same gzip+quick-xml stream) that reads
  the clip/instrument data in §3. Keep it independent so the oracle stays green.
- **Output**: write a wav (e.g. `hound` or reuse symphonia), then run the
  existing peaks extractor; attach as a preview with `source="sketch"`,
  `confidence` LOW (e.g. 0.25) and a fidelity note, so it sorts below real
  previews and is visibly distinct.
- **CLI**: add `ableton-scan sketch <set.als> -o out.wav [--max-seconds]`
  (mirrors the Python tool) for host validation before UI work.
- **Perf**: same caching strategy (decoded base per sample; pitched voice per
  semitone). Target sub-second for a 60 s preview so it can render on play.

## 8. UI integration spec (user's intent)

The user wants the sketch as a **dynamically-generated fallback preview**:

- **When**: a row/set has NO real preview (discovery/worker/manual). On the
  user pressing play (or hovering), generate the sketch **on demand** and stream
  it. Cache the result (keyed by set content hash) so it's instant next time.
- **Length**: ~1 minute cap (already a render param).
- **Distinct control**: the play button for a sketch preview must be a
  **different color** than real-preview rows, so it's never mistaken for the
  real render. Add an honest label/tooltip ("approximate sketch — no plugins/FX").
- **Backend**: a Tauri command (e.g. `sketch_preview(set_id) -> audio_path`,
  async + spawn_blocking) that renders to the app cache dir and returns the path
  for the asset protocol / PlayerBar. Reuse the export queue's relink/Places
  setup for sample resolution.
- **Don't** persist a sketch as a set's primary preview or let it suppress
  Auto-Export — it's a stopgap until a real render exists.

## 9. Files

- `tools/sketch_render.py` — the validated prototype (source of truth for the port).
- `ai/ARCHITECTURE.md` §3 — preview source (d).
- `ai/PROJECT_STATE.md` — running log of decisions/changes.
