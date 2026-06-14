// sketch/engine.rs
//! Sketch rendering engine.
//!
//! Faithful port of the validated Python prototype `tools/sketch_render.py`
//! (the documented source of truth). Single-threaded: the prototype already
//! hits the sub-second / ~1.8s target, and a global mutex around the mix buffer
//! defeated the point of parallelism. Bounded LRU caches keep memory in check;
//! sample resolution / relink is delegated to the caller's closure (which uses
//! the real ops::sample_index + catalog DB).

use crate::sketch::parser::{AudioClip, MidiClip, SketchSetData, Track};
use hound::{WavSpec, WavWriter};
use lru::LruCache;
use std::collections::{HashMap, HashSet};
use std::fs::File;
use std::num::NonZeroUsize;
use std::path::{Path, PathBuf};
use std::rc::Rc;
use symphonia::core::audio::SampleBuffer;
use symphonia::core::errors::Error as SymError;
use symphonia::core::io::MediaSourceStream;
use symphonia::core::probe::Hint;

const PI: f64 = std::f64::consts::PI;

/// Decoded base sample identity: resolved path + Simpler slice bounds.
type BaseKey = (PathBuf, u64, Option<u64>);
/// Repitched voice identity: base + semitone offset.
type VoiceKey = (PathBuf, u64, Option<u64>, i32);

struct Caches {
    /// Full file decoded to interleaved stereo @ target sr (audio clips).
    audio: LruCache<PathBuf, Rc<Vec<f32>>>,
    /// Simpler `[SampleStart:SampleEnd]` region, stereo @ target sr.
    base: LruCache<BaseKey, Option<Rc<Vec<f32>>>>,
    /// Declicked repitched voice (per semitone).
    voice: LruCache<VoiceKey, Option<Rc<Vec<f32>>>>,
}

impl Caches {
    fn new() -> Self {
        Caches {
            audio: LruCache::new(NonZeroUsize::new(128).unwrap()),
            base: LruCache::new(NonZeroUsize::new(256).unwrap()),
            voice: LruCache::new(NonZeroUsize::new(1024).unwrap()),
        }
    }
}

/// Render an approximate sketch of a set to interleaved stereo f32 @ `sample_rate`.
///
/// `resolve_sample(abs_path, rel_path) -> resolved file` is supplied by the
/// caller so the engine stays free of DB/Places concerns.
pub fn render_sketch<F>(
    data: &SketchSetData,
    _project_dir: &Path,
    max_seconds: f64,
    sample_rate: u32,
    resolve_sample: F,
) -> Result<Vec<f32>, String>
where
    F: Fn(&Option<String>, &Option<String>) -> Option<PathBuf>,
{
    let sr = sample_rate;
    let bpm = if data.tempo > 0.0 { data.tempo } else { 120.0 };
    let spb = 60.0 / bpm; // seconds per beat

    // Solo overrides mute; otherwise every non-muted track is audible.
    let (audible, _any_solo) = audible_tracks(&data.tracks);

    // Drop disabled clips and clips on inaudible (muted / non-solo'd) tracks.
    let kept_audio: Vec<&AudioClip> = data
        .clips
        .iter()
        .filter(|c| !c.disabled && audible.contains(&c.track))
        .collect();
    let kept_midi: Vec<&MidiClip> = data
        .midi_clips
        .iter()
        .filter(|m| !m.disabled && audible.contains(&m.track))
        .collect();

    if kept_audio.is_empty() && kept_midi.is_empty() {
        return Err("nothing audible to render (all clips muted/disabled, or empty set)".to_string());
    }

    // DAW grid: one clip per track at a time. eff_end truncates each clip at the
    // next clip's start on its track (kills doubled / take-lane stacks).
    let audio_items: Vec<(f64, f64, usize)> =
        kept_audio.iter().map(|c| (c.start, c.end, c.track)).collect();
    let midi_items: Vec<(f64, f64, usize)> =
        kept_midi.iter().map(|m| (m.start, m.end, m.track)).collect();
    let audio_eff = resolve_overlaps(&audio_items);
    let midi_eff = resolve_overlaps(&midi_items);

    // Timeline length (beats -> sec) + 1s tail, capped at max_seconds.
    let timeline_end = kept_audio
        .iter()
        .map(|c| c.end)
        .chain(kept_midi.iter().map(|m| m.end))
        .fold(0.0f64, f64::max);
    let mut total_sec = timeline_end * spb + 1.0;
    if max_seconds > 0.0 {
        total_sec = total_sec.min(max_seconds);
    }
    let frames = (total_sec * sr as f64) as usize + sr as usize;
    let mut mix = vec![0f32; frames * 2];

    let mut caches = Caches::new();

    // ---- audio clips -------------------------------------------------------
    for (ci, c) in kept_audio.iter().enumerate() {
        let start_sec = c.start * spb;
        if max_seconds > 0.0 && start_sec >= max_seconds {
            continue;
        }
        let dur_sec = ((audio_eff[ci] - c.start) * spb).max(0.0);
        if dur_sec <= 0.0 {
            continue;
        }
        let path = match resolve_sample(&c.path, &c.rel_path) {
            Some(p) => p,
            None => continue,
        };
        let audio = match decode_audio_stereo(&path, sr, &mut caches.audio) {
            Some(a) => a,
            None => continue,
        };
        let total_frames = audio.len() / 2;
        let off_sec = content_offset_sec(c, bpm);
        let s0 = (off_sec * sr as f64) as usize;
        if s0 >= total_frames {
            continue;
        }
        let want = (dur_sec * sr as f64) as usize;
        let avail = (total_frames - s0).min(want);
        if avail == 0 {
            continue;
        }

        let tvol = data.tracks.get(c.track).map(|t| t.vol).unwrap_or(1.0);
        let g = c.sample_volume * tvol;
        let mut seg = vec![0f32; avail * 2];
        for i in 0..avail {
            seg[i * 2] = audio[(s0 + i) * 2] * g;
            seg[i * 2 + 1] = audio[(s0 + i) * 2 + 1] * g;
        }

        // Clip fades (lengths are in beats).
        let fi = (c.fade_in * spb * sr as f64) as usize;
        let fo = (c.fade_out * spb * sr as f64) as usize;
        if fi > 1 {
            let n = fi.min(avail);
            if n > 1 {
                for i in 0..n {
                    let gain = i as f32 / (n as f32 - 1.0);
                    seg[i * 2] *= gain;
                    seg[i * 2 + 1] *= gain;
                }
            }
        }
        if fo > 1 {
            let n = fo.min(avail);
            if n > 1 {
                for i in 0..n {
                    let gain = 1.0 - i as f32 / (n as f32 - 1.0);
                    let f = avail - n + i;
                    seg[f * 2] *= gain;
                    seg[f * 2 + 1] *= gain;
                }
            }
        }
        declick(&mut seg, sr);

        let at = (start_sec * sr as f64) as usize;
        place(&mut mix, frames, at, &seg, 1.0);
    }

    // ---- MIDI clips: trigger the track's real instrument sample per note ----
    for (mi, m) in kept_midi.iter().enumerate() {
        let eff_end = midi_eff[mi];
        let track = &data.tracks[m.track]; // index valid: m.track ∈ audible
        let tvol = track.vol;
        let kind = synth_kind(&track.name);

        for (ab, dur_b, pitch, vel) in expand_midi_notes(m) {
            if ab >= eff_end {
                continue; // truncated by the next clip on this track
            }
            let start_sec = ab * spb;
            if max_seconds > 0.0 && start_sec >= max_seconds {
                continue;
            }
            let at = (start_sec * sr as f64) as usize;
            let vgain = (vel as f32 / 127.0) * 0.6 * tvol;

            let mut voiced = false;
            for part in &track.parts {
                if pitch < part.key_min || pitch > part.key_max {
                    continue;
                }
                let path = match resolve_sample(&part.path, &part.rel) {
                    Some(p) => p,
                    None => continue,
                };
                if let Some(voice) = instrument_voice(
                    &path, part.sstart, part.send, part.root, pitch, sr, &mut caches,
                ) {
                    place(&mut mix, frames, at, &voice, vgain);
                    voiced = true;
                }
            }
            if !voiced {
                // Generic synth only for true synths (no sample part matched).
                let freq = 440.0 * 2f64.powf((pitch as f64 - 69.0) / 12.0);
                if let Some(sig) = synth_note(freq, dur_b * spb, vel, kind, sr) {
                    place(&mut mix, frames, at, &sig, 1.0);
                }
            }
        }
    }

    // Normalize to -1.5 dBFS peak.
    let mut peak = 0f32;
    for &s in mix.iter() {
        let a = s.abs();
        if a > peak {
            peak = a;
        }
    }
    if peak > 0.0 {
        let target = 10f32.powf(-1.5 / 20.0); // ~0.8414
        let scale = target / peak;
        for s in mix.iter_mut() {
            *s *= scale;
        }
    }

    // Trim trailing silence (keep up to last loud frame + 0.5s).
    let mut last = 0usize;
    for f in 0..frames {
        if mix[f * 2].abs() > 1e-4 || mix[f * 2 + 1].abs() > 1e-4 {
            last = f;
        }
    }
    let keep_frames = (last + 1 + (sr as usize) / 2).min(frames);
    mix.truncate(keep_frames * 2);

    Ok(mix)
}

/// Tracks that should sound: solo'd ones if any solo is set, else non-muted.
fn audible_tracks(tracks: &[Track]) -> (HashSet<usize>, bool) {
    let any_solo = tracks.iter().any(|t| t.solo);
    let mut out = HashSet::new();
    for (i, t) in tracks.iter().enumerate() {
        if any_solo {
            if t.solo {
                out.insert(i);
            }
        } else if !t.mute {
            out.insert(i);
        }
    }
    (out, any_solo)
}

/// Per track, truncate each clip's end at the next clip's start (document order
/// breaks same-start ties). Returns eff_end aligned to `items`.
fn resolve_overlaps(items: &[(f64, f64, usize)]) -> Vec<f64> {
    let mut eff: Vec<f64> = items.iter().map(|&(_, e, _)| e).collect();
    let mut by_track: HashMap<usize, Vec<usize>> = HashMap::new();
    for (i, &(_, _, t)) in items.iter().enumerate() {
        by_track.entry(t).or_default().push(i);
    }
    for (_t, mut grp) in by_track {
        grp.sort_by(|&a, &b| {
            items[a]
                .0
                .partial_cmp(&items[b].0)
                .unwrap_or(std::cmp::Ordering::Equal)
                .then(a.cmp(&b))
        });
        for j in 0..grp.len() {
            let ci = grp[j];
            let end = items[ci].1;
            eff[ci] = if j + 1 < grp.len() {
                end.min(items[grp[j + 1]].0)
            } else {
                end
            };
        }
    }
    eff
}

/// Where in the sample (seconds) the clip begins playing.
fn content_offset_sec(clip: &AudioClip, bpm: f64) -> f64 {
    if clip.is_warped && clip.warp.len() >= 2 {
        let w0 = &clip.warp[0];
        let w1 = &clip.warp[1];
        if w1.beat_time != w0.beat_time {
            let off = w0.sec_time
                + (clip.loop_start - w0.beat_time) * (w1.sec_time - w0.sec_time)
                    / (w1.beat_time - w0.beat_time);
            return off.max(0.0);
        }
    }
    (clip.loop_start * 60.0 / bpm).max(0.0)
}

/// Expand a midi clip's notes to (arr_beat, dur_beats, pitch, vel), tiling the
/// loop region across the clip span when LoopOn is set.
fn expand_midi_notes(mc: &MidiClip) -> Vec<(f64, f64, u8, u8)> {
    let mut out = Vec::new();
    let span = mc.end - mc.start;
    if mc.loop_on && mc.loop_end > mc.loop_start {
        let loop_len = mc.loop_end - mc.loop_start;
        for note in &mc.notes {
            let rel = note.time - mc.loop_start;
            if rel < 0.0 || rel >= loop_len {
                continue;
            }
            let mut k = 0.0;
            while k * loop_len < span {
                let ab = mc.start + k * loop_len + rel;
                if ab >= mc.end {
                    break;
                }
                out.push((ab, note.duration, note.pitch, note.velocity));
                k += 1.0;
            }
        }
    } else {
        for note in &mc.notes {
            let ab = mc.start + (note.time - mc.loop_start);
            if ab >= mc.start && ab < mc.end {
                out.push((ab, note.duration, note.pitch, note.velocity));
            }
        }
    }
    out
}

const PERC_WORDS: &[&str] = &[
    "snare", "clap", "hat", "hi-hat", "hihat", "rim", "perc", "crash", "cymbal", "shaker", "tom",
    "snap", "conga", "bongo",
];

fn synth_kind(name: &Option<String>) -> &'static str {
    let n = name.as_deref().unwrap_or("").to_lowercase();
    if n.contains("kick") {
        return "kick";
    }
    if PERC_WORDS.iter().any(|w| n.contains(w)) {
        return "perc";
    }
    "tonal"
}

/// Generic voice for notes whose track has no sample part: a few sine harmonics
/// for tonal, a noise burst for perc, a pitch-drop sine for kick. Returns
/// declicked interleaved stereo (gain already baked in).
fn synth_note(freq: f64, dur_sec: f64, vel: u8, kind: &str, sr: u32) -> Option<Vec<f32>> {
    let n = (dur_sec.max(0.04) * sr as f64) as usize;
    if n == 0 {
        return None;
    }
    let amp = vel as f64 / 127.0;
    let mut mono = vec![0f64; n];
    match kind {
        "perc" => {
            // Deterministic xorshift noise (avoids an rng dependency).
            let mut seed = 0x2545_F491_4F6C_DD1Du64 ^ (n as u64).wrapping_mul(0x9E37_79B9_7F4A_7C15);
            for (i, m) in mono.iter_mut().enumerate() {
                seed ^= seed << 13;
                seed ^= seed >> 7;
                seed ^= seed << 17;
                let u = (seed >> 11) as f64 / 9_007_199_254_740_992.0; // 2^53 -> [0,1)
                let noise = u * 2.0 - 1.0;
                let t = i as f64 / sr as f64;
                *m = noise * (-t * 35.0).exp();
            }
        }
        "kick" => {
            let mut phase = 0f64;
            for (i, m) in mono.iter_mut().enumerate() {
                let t = i as f64 / sr as f64;
                let fenv = 110.0 * (-t * 30.0).exp() + 45.0;
                phase += fenv;
                *m = (2.0 * PI * phase / sr as f64).sin() * (-t * 9.0).exp();
            }
        }
        _ => {
            for (i, m) in mono.iter_mut().enumerate() {
                let t = i as f64 / sr as f64;
                let env = (t / 0.006).min(1.0) * (-t * 1.6).exp();
                *m = ((2.0 * PI * freq * t).sin()
                    + 0.35 * (2.0 * PI * 2.0 * freq * t).sin()
                    + 0.18 * (2.0 * PI * 3.0 * freq * t).sin())
                    / 1.53
                    * env;
            }
        }
    }
    let mut out = Vec::with_capacity(n * 2);
    for &s in &mono {
        let v = (s * amp * 0.32) as f32;
        out.push(v);
        out.push(v);
    }
    declick(&mut out, sr);
    Some(out)
}

/// Tiny raised-cosine fade on the first/last few ms (in place) so voices don't
/// start/end on a non-zero sample. Short attack keeps transients punchy.
fn declick(buf: &mut [f32], sr: u32) {
    let frames = buf.len() / 2;
    if frames < 8 {
        return;
    }
    let fi = ((sr as f64 * 1.5 / 1000.0) as usize).min(frames / 2);
    let fo = ((sr as f64 * 5.0 / 1000.0) as usize).min(frames / 2);
    if fi > 1 {
        for k in 0..fi {
            let theta = PI * k as f64 / (fi as f64 - 1.0);
            let g = (0.5 * (1.0 - theta.cos())) as f32;
            buf[k * 2] *= g;
            buf[k * 2 + 1] *= g;
        }
    }
    if fo > 1 {
        for k in 0..fo {
            let theta = PI * (1.0 - k as f64 / (fo as f64 - 1.0));
            let g = (0.5 * (1.0 - theta.cos())) as f32;
            let f = frames - fo + k;
            buf[f * 2] *= g;
            buf[f * 2 + 1] *= g;
        }
    }
}

/// Add `seg` (interleaved stereo) into `mix` at `at_frame`, scaled by `gain`.
fn place(mix: &mut [f32], frames: usize, at_frame: usize, seg: &[f32], gain: f32) {
    if at_frame >= frames {
        return;
    }
    let n = (seg.len() / 2).min(frames - at_frame);
    for i in 0..n {
        mix[(at_frame + i) * 2] += seg[i * 2] * gain;
        mix[(at_frame + i) * 2 + 1] += seg[i * 2 + 1] * gain;
    }
}

// ---- decode / resample -----------------------------------------------------

/// Decode (any symphonia format) to interleaved f32 at the file's native rate,
/// returning (samples, channels, native_sr).
fn decode_audio(path: &Path) -> Result<(Vec<f32>, usize, u32), String> {
    let file = File::open(path).map_err(|e| e.to_string())?;
    let mss = MediaSourceStream::new(Box::new(file), Default::default());
    let mut hint = Hint::new();
    if let Some(ext) = path.extension() {
        hint.with_extension(&ext.to_string_lossy());
    }
    let probed = symphonia::default::get_probe()
        .format(&hint, mss, &Default::default(), &Default::default())
        .map_err(|e| e.to_string())?;
    let mut format = probed.format;
    let track = format
        .default_track()
        .ok_or_else(|| "no audio track".to_string())?;
    let track_id = track.id;
    let native_sr = track.codec_params.sample_rate.unwrap_or(44_100);
    let mut decoder = symphonia::default::get_codecs()
        .make(&track.codec_params, &Default::default())
        .map_err(|e| e.to_string())?;

    let mut out: Vec<f32> = Vec::new();
    let mut channels = 0usize;
    let mut sbuf: Option<SampleBuffer<f32>> = None;
    loop {
        let packet = match format.next_packet() {
            Ok(p) => p,
            Err(SymError::IoError(e)) if e.kind() == std::io::ErrorKind::UnexpectedEof => break,
            Err(SymError::ResetRequired) => break,
            Err(_) => break,
        };
        if packet.track_id() != track_id {
            continue;
        }
        match decoder.decode(&packet) {
            Ok(decoded) => {
                let spec = *decoded.spec();
                channels = spec.channels.count().max(1);
                // SampleBuffer<f32> converts ANY decoded format (S16/S24/S32/F32…)
                // to interleaved f32 — the hand-rolled F32-only path missed PCM.
                let buf = sbuf
                    .get_or_insert_with(|| SampleBuffer::<f32>::new(decoded.capacity() as u64, spec));
                buf.copy_interleaved_ref(decoded);
                out.extend_from_slice(buf.samples());
            }
            Err(SymError::DecodeError(_)) => continue, // skip a corrupt frame
            Err(SymError::IoError(e)) if e.kind() == std::io::ErrorKind::UnexpectedEof => break,
            Err(_) => break,
        }
    }
    if out.is_empty() {
        return Err(format!("no audio decoded from {}", path.display()));
    }
    Ok((out, channels.max(1), native_sr))
}

/// Linear resample (np.interp over linspace(0, n-1, new_n)).
fn resample_channel(samples: &[f32], new_n: usize) -> Vec<f32> {
    let n = samples.len();
    if n == 0 || new_n == 0 {
        return Vec::new();
    }
    if n == 1 {
        return vec![samples[0]; new_n];
    }
    if new_n == 1 {
        return vec![samples[0]];
    }
    let step = (n as f64 - 1.0) / (new_n as f64 - 1.0);
    let mut out = Vec::with_capacity(new_n);
    for i in 0..new_n {
        let x = i as f64 * step;
        let i0 = x.floor() as usize;
        let frac = x - i0 as f64;
        let v = if i0 + 1 < n {
            samples[i0] as f64 * (1.0 - frac) + samples[i0 + 1] as f64 * frac
        } else {
            samples[n - 1] as f64
        };
        out.push(v as f32);
    }
    out
}

/// Resample to `target_sr` (linear) and force interleaved stereo.
fn to_stereo_sr(native: &[f32], channels: usize, native_sr: u32, target_sr: u32) -> Vec<f32> {
    let channels = channels.max(1);
    let n = native.len() / channels;
    if n == 0 {
        return Vec::new();
    }
    let mut chans: Vec<Vec<f32>> = vec![Vec::with_capacity(n); channels];
    for i in 0..n {
        for (c, ch) in chans.iter_mut().enumerate() {
            ch.push(native[i * channels + c]);
        }
    }
    let new_n = if native_sr != target_sr && n > 1 {
        (((n as f64) * target_sr as f64 / native_sr as f64).round() as i64).max(1) as usize
    } else {
        n
    };
    if new_n != n {
        chans = chans.iter().map(|ch| resample_channel(ch, new_n)).collect();
    }
    let len = new_n;
    let mut out = Vec::with_capacity(len * 2);
    if channels == 1 {
        for i in 0..len {
            let s = chans[0][i];
            out.push(s);
            out.push(s);
        }
    } else {
        for i in 0..len {
            out.push(chans[0][i]);
            out.push(chans[1][i]);
        }
    }
    out
}

/// Repitch interleaved stereo by playback-rate change (Simpler Classic mode):
/// up = faster + shorter. Linear interpolation.
fn pitch_resample(audio: &[f32], semitones: i32) -> Vec<f32> {
    if semitones == 0 {
        return audio.to_vec();
    }
    let n = audio.len() / 2;
    if n == 0 {
        return Vec::new();
    }
    let ratio = 2f64.powf(semitones as f64 / 12.0);
    let new_n = ((n as f64 / ratio).round() as i64).max(1) as usize;
    let l: Vec<f32> = (0..n).map(|i| audio[i * 2]).collect();
    let r: Vec<f32> = (0..n).map(|i| audio[i * 2 + 1]).collect();
    let rl = resample_channel(&l, new_n);
    let rr = resample_channel(&r, new_n);
    let mut out = Vec::with_capacity(new_n * 2);
    for i in 0..new_n {
        out.push(rl[i]);
        out.push(rr[i]);
    }
    out
}

// ---- cached lookups --------------------------------------------------------

fn decode_audio_stereo(
    path: &Path,
    sr: u32,
    cache: &mut LruCache<PathBuf, Rc<Vec<f32>>>,
) -> Option<Rc<Vec<f32>>> {
    if let Some(a) = cache.get(path) {
        return Some(a.clone());
    }
    let (native, ch, native_sr) = decode_audio(path).ok()?;
    let stereo = Rc::new(to_stereo_sr(&native, ch, native_sr, sr));
    cache.put(path.to_path_buf(), stereo.clone());
    Some(stereo)
}

/// Decode + slice `[SampleStart:SampleEnd]` (native-rate frames) + resample to
/// stereo @ target sr. Cached per (path, start, end).
fn instrument_base(
    path: &Path,
    sstart: u64,
    send: Option<u64>,
    sr: u32,
    cache: &mut LruCache<BaseKey, Option<Rc<Vec<f32>>>>,
) -> Option<Rc<Vec<f32>>> {
    let key = (path.to_path_buf(), sstart, send);
    if let Some(v) = cache.get(&key) {
        return v.clone();
    }
    let result = (|| {
        let (native, ch, native_sr) = decode_audio(path).ok()?;
        let ch = ch.max(1);
        let total = native.len() / ch;
        let s = (sstart as usize).min(total);
        let mut e = match send {
            Some(x) if x > 0 => x as usize,
            _ => total,
        };
        e = e.min(total);
        if e <= s {
            e = total; // bad/empty bounds -> use whole file (matches prototype)
        }
        if e <= s {
            return None;
        }
        let sliced = native[s * ch..e * ch].to_vec();
        Some(Rc::new(to_stereo_sr(&sliced, ch, native_sr, sr)))
    })();
    cache.put(key, result.clone());
    result
}

/// The instrument's sample, repitched for `pitch` and declicked. Cached per
/// semitone so retriggers (drums) are cheap.
fn instrument_voice(
    path: &Path,
    sstart: u64,
    send: Option<u64>,
    root: u8,
    pitch: u8,
    sr: u32,
    caches: &mut Caches,
) -> Option<Rc<Vec<f32>>> {
    let semi = pitch as i32 - root as i32;
    let vkey = (path.to_path_buf(), sstart, send, semi);
    if let Some(v) = caches.voice.get(&vkey) {
        return v.clone();
    }
    let base = instrument_base(path, sstart, send, sr, &mut caches.base);
    let voiced = base.map(|b| {
        let mut p = pitch_resample(&b, semi);
        declick(&mut p, sr);
        Rc::new(p)
    });
    caches.voice.put(vkey, voiced.clone());
    voiced
}

// ---- output ----------------------------------------------------------------

/// Write interleaved stereo f32 to a 16-bit WAV.
pub fn write_wav_file(path: &Path, samples: &[f32], sample_rate: u32) -> Result<(), String> {
    let spec = WavSpec {
        channels: 2,
        sample_rate,
        bits_per_sample: 16,
        sample_format: hound::SampleFormat::Int,
    };
    let mut writer = WavWriter::create(path, spec).map_err(|e| e.to_string())?;
    for &sample in samples {
        let sample_i16 = (sample.clamp(-1.0, 1.0) * 32767.0) as i16;
        writer.write_sample(sample_i16).map_err(|e| e.to_string())?;
    }
    writer.finalize().map_err(|e| e.to_string())?;
    Ok(())
}
