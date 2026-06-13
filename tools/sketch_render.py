#!/usr/bin/env python3
"""Sketch renderer (PROTOTYPE) — approximate audio preview of an Ableton set
WITHOUT opening Ableton.

What it does: pulls the ARRANGEMENT audio clips out of a .als (their timeline
positions, sample references, gain and fades), decodes the referenced audio,
lays each clip on a timeline at its beat position, sums everything, and writes a
.wav. The audio content is EXACT (your real samples); the approximation is in
what we *don't* reproduce.

KNOWN APPROXIMATIONS (v1, by design):
  * MIDI clips are ignored (no synth yet) — instrument/beat parts won't sound.
  * No device/plugin effects, no mixer automation, no sends/returns.
  * No warp time-stretch: clips play at native rate from their content offset
    (fine when warp≈1, e.g. recorded vocals/one-shots; drifts on heavily
    warped loops).
  * Track/clip volume beyond the clip's SampleVolume is not applied.

This is a recall/preview sketch, not a master. Live remains the only faithful
renderer.
"""
import sys
import os
import gzip
import wave
import aifc
import argparse
import xml.etree.ElementTree as ET
import numpy as np

OUT_SR = 44100
AUDIO_EXTS = (".wav", ".aif", ".aiff", ".flac", ".mp3", ".m4a", ".ogg")


# ---------------------------------------------------------------- .als parsing
def _v(el):
    """Value attribute as string, or None."""
    return el.get("Value") if el is not None else None


TRACK_TAGS = ("AudioTrack", "MidiTrack", "GroupTrack", "ReturnTrack")


def parse_set(als_path):
    """Stream the .als; return (tempo_bpm, audio_clips, tracks, midi_clips).

    audio clip: start/end (arrangement beats), loop_start (content beats),
    loop_on, sample_volume, fade_in/out (beats), path, rel_path, warp markers,
    owning track index, per-clip `disabled`.
    midi clip: start/end, loop_start/end, loop_on, disabled, track, and
    `notes` = [(beat, dur_beats, pitch, velocity)] in CONTENT beats.
    track: {mute, solo, kind, name}. Only ARRANGEMENT clips (not session
    ClipSlot clips) are collected.

    Mute is the TRACK-level mixer Speaker (false = muted); nested device mixers
    are excluded by stack path. Solo is read defensively from the track mixer.
    """
    tempo_cands = []
    clips = []
    midi_clips = []
    tracks = []
    cur_track = None
    stack = []
    ctx_master = 0
    cur = None          # audio clip accumulator
    mc = None           # midi clip accumulator
    kt = None           # current KeyTrack: {"key":int|None, "notes":[...]}
    in_loop = in_fileref = in_fades = 0

    with gzip.open(als_path, "rb") as fh:
        for ev, el in ET.iterparse(fh, events=("start", "end")):
            tag = el.tag
            if ev == "start":
                stack.append(tag)
                if tag in TRACK_TAGS and len(stack) >= 2 and stack[-2] == "Tracks":
                    tracks.append({"mute": False, "solo": False, "kind": tag, "name": None})
                    cur_track = len(tracks) - 1
                if tag in ("MasterTrack", "MainTrack"):
                    ctx_master += 1
                elif tag == "Loop":
                    in_loop += 1
                elif tag == "FileRef":
                    in_fileref += 1
                elif tag == "Fades":
                    in_fades += 1
                elif tag == "AudioClip" and "ClipSlot" not in stack:
                    cur = {
                        "start": None, "end": None, "loop_start": 0.0,
                        "loop_on": False, "sample_volume": 1.0,
                        "fade_in": 0.0, "fade_out": 0.0,
                        "path": None, "rel_path": None,
                        "warp": [], "is_warped": False, "name": None,
                        "track": cur_track, "disabled": False,
                    }
                elif tag == "MidiClip" and "ClipSlot" not in stack:
                    mc = {
                        "start": None, "end": None, "loop_start": 0.0,
                        "loop_end": 0.0, "loop_on": False, "disabled": False,
                        "track": cur_track, "notes": [],
                    }
                elif tag == "KeyTrack" and mc is not None:
                    kt = {"key": None, "notes": []}
                elif tag == "MidiNoteEvent" and kt is not None:
                    if el.get("IsEnabled", "true") != "false":
                        try:
                            kt["notes"].append((
                                float(el.get("Time", "0")),
                                float(el.get("Duration", "0")),
                                int(float(el.get("Velocity", "100"))),
                            ))
                        except (TypeError, ValueError):
                            pass
                elif tag == "WarpMarker" and cur is not None:
                    st, bt = el.get("SecTime"), el.get("BeatTime")
                    if st is not None and bt is not None:
                        cur["warp"].append((float(st), float(bt)))

                if tag == "MidiKey" and kt is not None:
                    try: kt["key"] = int(float(_v(el)))
                    except (TypeError, ValueError): pass

                # track-level mute / solo (path-qualified to the track mixer)
                if tag == "Manual" and len(stack) >= 5 and stack[-2] == "Speaker" \
                        and stack[-3] == "Mixer" and stack[-4] == "DeviceChain" \
                        and stack[-5] in TRACK_TAGS:
                    if cur_track is not None and _v(el) == "false":
                        tracks[cur_track]["mute"] = True
                if tag == "Solo" and len(stack) >= 4 and stack[-2] == "Mixer" \
                        and stack[-3] == "DeviceChain" and stack[-4] in TRACK_TAGS:
                    if cur_track is not None and _v(el) == "true":
                        tracks[cur_track]["solo"] = True
                # track name: Track > Name > EffectiveName
                if tag == "EffectiveName" and len(stack) >= 3 and stack[-2] == "Name" \
                        and stack[-3] in TRACK_TAGS and cur_track is not None \
                        and tracks[cur_track]["name"] is None:
                    tracks[cur_track]["name"] = _v(el)

                # audio clip field captures
                if cur is not None:
                    if tag == "CurrentStart" and cur["start"] is None:
                        cur["start"] = float(_v(el))
                    elif tag == "CurrentEnd" and cur["end"] is None:
                        cur["end"] = float(_v(el))
                    elif tag == "Name" and cur["name"] is None:
                        cur["name"] = _v(el)
                    elif tag == "IsWarped":
                        cur["is_warped"] = (_v(el) == "true")
                    elif tag == "Disabled" and len(stack) >= 2 and stack[-2] == "AudioClip":
                        cur["disabled"] = (_v(el) == "true")
                    elif tag == "SampleVolume":
                        try: cur["sample_volume"] = float(_v(el))
                        except (TypeError, ValueError): pass
                    elif in_loop:
                        if tag == "LoopStart": cur["loop_start"] = float(_v(el))
                        elif tag == "LoopOn": cur["loop_on"] = (_v(el) == "true")
                    elif in_fades:
                        if tag == "FadeInLength":
                            try: cur["fade_in"] = float(_v(el))
                            except (TypeError, ValueError): pass
                        elif tag == "FadeOutLength":
                            try: cur["fade_out"] = float(_v(el))
                            except (TypeError, ValueError): pass
                    elif in_fileref:
                        if tag == "Path" and cur["path"] is None:
                            cur["path"] = _v(el)
                        elif tag == "RelativePath" and cur["rel_path"] is None:
                            cur["rel_path"] = _v(el)
                # midi clip field captures
                elif mc is not None:
                    if tag == "CurrentStart" and mc["start"] is None:
                        mc["start"] = float(_v(el))
                    elif tag == "CurrentEnd" and mc["end"] is None:
                        mc["end"] = float(_v(el))
                    elif tag == "Disabled" and len(stack) >= 2 and stack[-2] == "MidiClip":
                        mc["disabled"] = (_v(el) == "true")
                    elif in_loop:
                        if tag == "LoopStart": mc["loop_start"] = float(_v(el))
                        elif tag == "LoopEnd": mc["loop_end"] = float(_v(el))
                        elif tag == "LoopOn": mc["loop_on"] = (_v(el) == "true")

                if tag == "Manual" and len(stack) >= 2 and stack[-2] == "Tempo":
                    try: tempo_cands.append((float(_v(el)), ctx_master > 0))
                    except (TypeError, ValueError): pass
            else:  # end
                if tag in TRACK_TAGS and len(stack) >= 2 and stack[-2] == "Tracks":
                    cur_track = None
                if tag in ("MasterTrack", "MainTrack"):
                    ctx_master = max(0, ctx_master - 1)
                elif tag == "Loop":
                    in_loop = max(0, in_loop - 1)
                elif tag == "FileRef":
                    in_fileref = max(0, in_fileref - 1)
                elif tag == "Fades":
                    in_fades = max(0, in_fades - 1)
                elif tag == "KeyTrack" and kt is not None:
                    if kt["key"] is not None:
                        for (t, d, v) in kt["notes"]:
                            mc["notes"].append((t, d, kt["key"], v))
                    kt = None
                elif tag == "AudioClip" and cur is not None:
                    if cur["start"] is not None and cur["end"] is not None:
                        clips.append(cur)
                    cur = None
                elif tag == "MidiClip" and mc is not None:
                    if mc["start"] is not None and mc["end"] is not None and mc["notes"]:
                        midi_clips.append(mc)
                    mc = None
                if stack and stack[-1] == tag:
                    stack.pop()
                el.clear()

    tempo = next((v for v, m in tempo_cands if m), None)
    if tempo is None and tempo_cands:
        tempo = tempo_cands[0][0]
    return tempo or 120.0, clips, tracks, midi_clips


# ------------------------------------------------------------- audio decoding
def _pcm_to_float(raw, width, nchan, big_endian):
    """Raw PCM bytes -> float32 array shape (frames, nchan), range ~[-1,1]."""
    if width == 2:
        dt = np.dtype(">i2" if big_endian else "<i2")
        a = np.frombuffer(raw, dtype=dt).astype(np.float32) / 32768.0
    elif width == 4:
        dt = np.dtype(">i4" if big_endian else "<i4")
        a = np.frombuffer(raw, dtype=dt).astype(np.float32) / 2147483648.0
    elif width == 3:
        b = np.frombuffer(raw, dtype=np.uint8).reshape(-1, 3).astype(np.int32)
        if big_endian:
            ints = (b[:, 0] << 16) | (b[:, 1] << 8) | b[:, 2]
        else:
            ints = (b[:, 2] << 16) | (b[:, 1] << 8) | b[:, 0]
        ints = np.where(ints & 0x800000, ints - 0x1000000, ints)
        a = ints.astype(np.float32) / 8388608.0
    elif width == 1:  # 8-bit unsigned
        a = (np.frombuffer(raw, dtype=np.uint8).astype(np.float32) - 128.0) / 128.0
    else:
        raise ValueError(f"unsupported sample width {width}")
    if nchan > 1:
        a = a.reshape(-1, nchan)
    else:
        a = a.reshape(-1, 1)
    return a


def load_audio(path):
    """Decode a .wav or .aif/.aiff file -> (float32 (frames,ch), sr).

    mp3/flac/m4a are not decodable with stdlib here -> raises.
    """
    low = path.lower()
    if low.endswith((".aif", ".aiff")):
        with aifc.open(path, "rb") as a:
            nchan, width, sr, n = (a.getnchannels(), a.getsampwidth(),
                                   a.getframerate(), a.getnframes())
            raw = a.readframes(n)
        return _pcm_to_float(raw, width, nchan, big_endian=True), sr
    if low.endswith(".wav"):
        with wave.open(path, "rb") as w:
            nchan, width, sr, n = (w.getnchannels(), w.getsampwidth(),
                                   w.getframerate(), w.getnframes())
            raw = w.readframes(n)
        return _pcm_to_float(raw, width, nchan, big_endian=False), sr
    raise ValueError(f"cannot decode (stdlib) {os.path.basename(path)}")


def to_stereo_sr(audio, sr, target_sr=OUT_SR):
    """Resample (linear) to target_sr and force 2 channels."""
    n, ch = audio.shape
    if sr != target_sr and n > 1:
        new_n = max(1, int(round(n * target_sr / sr)))
        src_idx = np.linspace(0.0, n - 1, new_n)
        base = np.arange(n)
        audio = np.stack([np.interp(src_idx, base, audio[:, c]) for c in range(ch)], axis=1)
    if ch == 1:
        audio = np.repeat(audio, 2, axis=1)
    elif ch > 2:
        audio = audio[:, :2]
    return audio


# ------------------------------------------------------------------ rendering
def resolve_sample(clip, project_dir):
    """Find the real audio file: absolute Path, else project_dir/RelativePath."""
    p = clip.get("path")
    if p and os.path.exists(p):
        return p
    rel = clip.get("rel_path")
    if rel:
        cand = os.path.join(project_dir, rel)
        if os.path.exists(cand):
            return cand
        # also try basename anywhere under project_dir (moved sample)
        base = os.path.basename(rel)
        for root, _dirs, files in os.walk(project_dir):
            if base in files:
                return os.path.join(root, base)
    return None


def content_offset_sec(clip, bpm):
    """Where in the sample (seconds) the clip begins playing."""
    warp = clip["warp"]
    ls = clip["loop_start"]
    if clip["is_warped"] and len(warp) >= 2:
        (s0, b0), (s1, b1) = warp[0], warp[1]
        if b1 != b0:
            return max(0.0, s0 + (ls - b0) * (s1 - s0) / (b1 - b0))
    # unwarped (or no markers): treat content beats at project tempo
    return max(0.0, ls * 60.0 / bpm)


def audible_tracks(tracks):
    """Indices of tracks that should sound: solo'd ones if any solo is set,
    else every non-muted track. (Solo persistence TBD — see notes.)"""
    any_solo = any(t["solo"] for t in tracks)
    out = set()
    for i, t in enumerate(tracks):
        if any_solo:
            if t["solo"]:
                out.add(i)
        elif not t["mute"]:
            out.add(i)
    return out, any_solo


# ------------------------------------------------------------ MIDI synthesis
# MIDI clips reference the user's real instruments/plugins, which we can't run.
# So notes are voiced by a GENERIC synth — a deliberate approximation: a few
# sine harmonics for pitched parts, noise bursts for drums, a pitch-drop sine
# for kicks. It conveys the part (melody/rhythm/bass), not the real timbre.
_PERC_WORDS = ("snare", "clap", "hat", "hi-hat", "hihat", "rim", "perc",
               "crash", "cymbal", "shaker", "tom", "snap", "conga", "bongo")


def synth_kind(track_name):
    n = (track_name or "").lower()
    if "kick" in n:
        return "kick"
    if any(w in n for w in _PERC_WORDS):
        return "perc"
    return "tonal"


def synth_note(freq, dur_sec, vel, kind, sr):
    n = int(max(0.04, dur_sec) * sr)
    if n <= 0:
        return None
    t = np.arange(n) / sr
    amp = vel / 127.0
    if kind == "perc":
        sig = np.random.uniform(-1, 1, n) * np.exp(-t * 35.0)
    elif kind == "kick":
        fenv = 110.0 * np.exp(-t * 30.0) + 45.0
        sig = np.sin(2 * np.pi * np.cumsum(fenv) / sr) * np.exp(-t * 9.0)
    else:  # tonal: a few harmonics, quick attack + gentle decay
        env = np.minimum(1.0, t / 0.006) * np.exp(-t * 1.6)
        sig = (np.sin(2 * np.pi * freq * t)
               + 0.35 * np.sin(2 * np.pi * 2 * freq * t)
               + 0.18 * np.sin(2 * np.pi * 3 * freq * t)) / 1.53 * env
    return (sig * amp * 0.32).astype(np.float32)


def midi_note_positions(clip):
    """Expand a midi clip's notes to (arr_beat, dur_beats, pitch, vel),
    tiling the loop region across the clip span when LoopOn is set."""
    cs, ce = clip["start"], clip["end"]
    ls, le = clip["loop_start"], clip["loop_end"]
    span = ce - cs
    out = []
    if clip["loop_on"] and le > ls:
        loop_len = le - ls
        for (t, d, pitch, vel) in clip["notes"]:
            rel = t - ls
            if rel < 0 or rel >= loop_len:
                continue
            k = 0
            while k * loop_len < span:
                ab = cs + k * loop_len + rel
                if ab >= ce:
                    break
                out.append((ab, d, pitch, vel))
                k += 1
    else:
        for (t, d, pitch, vel) in clip["notes"]:
            ab = cs + (t - ls)
            if cs <= ab < ce:
                out.append((ab, d, pitch, vel))
    return out


def _audible_clips(items, audible):
    """Drop disabled clips and clips on muted/non-solo'd tracks."""
    kept = muted = disabled = 0
    out = []
    for c in items:
        if c["disabled"]:
            disabled += 1; continue
        if c["track"] is not None and c["track"] not in audible:
            muted += 1; continue
        out.append(c); kept += 1
    return out, muted, disabled


def render(als_path, out_path, target_sr=OUT_SR, max_seconds=60.0, verbose=True):
    project_dir = os.path.dirname(os.path.abspath(als_path))
    bpm, clips, tracks, midi_clips = parse_set(als_path)
    spb = 60.0 / bpm  # seconds per beat

    audible, any_solo = audible_tracks(tracks)
    clips, a_muted, a_disabled = _audible_clips(clips, audible)
    midi_clips, m_muted, m_disabled = _audible_clips(midi_clips, audible)

    if not clips and not midi_clips:
        print("Nothing audible to render (all clips muted/disabled, or empty set).")
        return None

    ends = [c["end"] for c in clips] + [m["end"] for m in midi_clips]
    timeline_end = max(ends) if ends else 0.0
    total_sec = timeline_end * spb + 1.0
    if max_seconds:
        total_sec = min(total_sec, max_seconds)
    cap_samples = int(total_sec * target_sr) + target_sr
    mix = np.zeros((cap_samples, 2), dtype=np.float32)

    placed = missing = undecodable = 0
    cache = {}
    for c in clips:
        if max_seconds and c["start"] * spb >= max_seconds:
            continue
        path = resolve_sample(c, project_dir)
        if not path:
            missing += 1
            continue
        try:
            if path not in cache:
                cache[path] = to_stereo_sr(*load_audio(path), target_sr)
            audio = cache[path]
        except Exception as e:
            undecodable += 1
            if verbose:
                print(f"  [skip] {os.path.basename(path)}: {e}")
            continue

        start_sec = c["start"] * spb
        dur_sec = max(0.0, (c["end"] - c["start"]) * spb)
        off_sec = content_offset_sec(c, bpm)

        s0 = int(off_sec * target_sr)
        seg = audio[s0:s0 + int(dur_sec * target_sr)]
        if seg.shape[0] == 0:
            continue
        seg = seg * float(c.get("sample_volume", 1.0))

        # linear fades (clip fade lengths are in beats)
        fi = int(c["fade_in"] * spb * target_sr)
        fo = int(c["fade_out"] * spb * target_sr)
        if fi > 1:
            n = min(fi, seg.shape[0]); seg[:n] *= np.linspace(0, 1, n)[:, None]
        if fo > 1:
            n = min(fo, seg.shape[0]); seg[-n:] *= np.linspace(1, 0, n)[:, None]

        at = int(start_sec * target_sr)
        end = at + seg.shape[0]
        if end > mix.shape[0]:
            seg = seg[:mix.shape[0] - at]
            end = mix.shape[0]
        mix[at:end] += seg
        placed += 1

    # --- MIDI clips -> generic synth ---
    notes_played = 0
    for m in midi_clips:
        ti = m["track"]
        kind = synth_kind(tracks[ti]["name"] if (ti is not None and ti < len(tracks)) else None)
        for (ab, dur_b, pitch, vel) in midi_note_positions(m):
            start_sec = ab * spb
            if max_seconds and start_sec >= max_seconds:
                continue
            freq = 440.0 * (2.0 ** ((pitch - 69) / 12.0))
            voice = synth_note(freq, dur_b * spb, vel, kind, target_sr)
            if voice is None:
                continue
            at = int(start_sec * target_sr)
            if at >= mix.shape[0]:
                continue
            end = min(at + voice.shape[0], mix.shape[0])
            v = voice[:end - at]
            mix[at:end, 0] += v
            mix[at:end, 1] += v
            notes_played += 1

    # normalize to -1.5 dBFS peak
    peak = float(np.max(np.abs(mix))) if mix.size else 0.0
    if peak > 0:
        mix *= (10 ** (-1.5 / 20)) / peak
    rms = float(np.sqrt(np.mean(mix ** 2)))

    # trim trailing silence
    nonzero = np.where(np.any(np.abs(mix) > 1e-4, axis=1))[0]
    if len(nonzero):
        mix = mix[: nonzero[-1] + target_sr // 2]

    out = (np.clip(mix, -1, 1) * 32767).astype("<i2")
    with wave.open(out_path, "wb") as w:
        w.setnchannels(2); w.setsampwidth(2); w.setframerate(target_sr)
        w.writeframes(out.tobytes())

    n_muted = sum(t["mute"] for t in tracks)
    print(f"\n{os.path.basename(als_path)}")
    print(f"  tempo={bpm:.2f}  tracks={len(tracks)} (muted={n_muted}, solo={'yes' if any_solo else 'no'})")
    print(f"  audio: kept={len(clips)} placed={placed} (dropped muted={a_muted} "
          f"disabled={a_disabled} missing={missing} undecodable={undecodable})")
    print(f"  midi:  clips={len(midi_clips)} notes_voiced={notes_played} "
          f"(dropped muted={m_muted} disabled={m_disabled})")
    print(f"  out={mix.shape[0]/target_sr:.1f}s (cap {max_seconds:.0f}s)  peak={peak:.3f} rms={rms:.4f}")
    print(f"  -> {out_path}")
    return out_path


if __name__ == "__main__":
    ap = argparse.ArgumentParser()
    ap.add_argument("als")
    ap.add_argument("-o", "--out", required=True)
    ap.add_argument("--max-seconds", type=float, default=60.0,
                    help="cap preview length (0 = full song)")
    args = ap.parse_args()
    render(args.als, args.out, max_seconds=args.max_seconds)
