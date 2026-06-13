// sketch/engine.rs
//! Sketch rendering engine.

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use crate::sketch::parser::{SketchSetData, MidiClip};
use hound::{WavSpec, WavWriter};
use symphonia::core::audio::{AudioBufferRef, Signal};
use symphonia::core::io::MediaSourceStream;
use symphonia::core::probe::Hint;
use std::fs::File;
use lru::LruCache;
use std::num::NonZeroUsize;
use rayon::prelude::*;
use std::f32::consts::PI;

// Cache for decoded samples: Path -> Vec<f32>
type SampleCache = LruCache<PathBuf, Arc<Vec<f32>>>;
// Cache for repitched voices: (Path, pitch_diff) -> Vec<f32>
type VoiceCache = LruCache<(PathBuf, i8), Arc<Vec<f32>>>;

pub fn render_sketch<F>(
    data: &SketchSetData,
    _project_dir: &Path,
    max_seconds: f64,
    sample_rate: u32,
    resolve_sample: F,
) -> Result<Vec<f32>, String> 
where F: Fn(&Option<String>, &Option<String>) -> Option<PathBuf> + Sync + Send
{
    let duration_sec = max_seconds.min(data.clips.iter().map(|c| c.end).chain(data.midi_clips.iter().map(|c| c.end)).fold(0.0, f64::max));
    let num_samples = (duration_sec * sample_rate as f64) as usize;
    
    // Shared cache
    let cache = Arc::new(Mutex::new(SampleCache::new(NonZeroUsize::new(100).unwrap())));
    let voice_cache = Arc::new(Mutex::new(VoiceCache::new(NonZeroUsize::new(500).unwrap())));
    
    // Parallel mixing
    let mix_arc = Arc::new(Mutex::new(vec![0.0f32; num_samples * 2]));
    let bpm = data.tempo;
    let spb = 60.0 / bpm;

    // 1. Mix Audio Clips
    data.clips.par_iter().for_each(|clip| {
        if let Some(path) = resolve_sample(&clip.path, &clip.rel_path) {
            if let Ok(samples) = decode_and_cache(&path, &cache) {
                let start_idx = (clip.start * spb * sample_rate as f64) as usize;
                let mut mix = mix_arc.lock().unwrap();
                let frames_to_mix = (samples.len() / 2).min(num_samples - start_idx.min(num_samples));
                for i in 0..frames_to_mix {
                    let mix_idx = (start_idx + i) * 2;
                    if mix_idx + 1 < mix.len() {
                        mix[mix_idx] += samples[i * 2] * clip.sample_volume;
                        mix[mix_idx + 1] += samples[i * 2 + 1] * clip.sample_volume;
                    }
                }
            }
        }
    });

    // 2. Mix MIDI Clips
    data.midi_clips.par_iter().for_each(|mc| {
        let track = &data.tracks[mc.track];
        if track.mute { return; }
        
        let notes = expand_midi_notes(mc);
        for (ab, dur_b, pitch, vel) in notes {
            let at = ((ab * spb) * sample_rate as f64) as usize;
            let vel_gain = (vel as f32 / 127.0) * track.vol;
            
            let mut voiced = false;
            for part in &track.parts {
                if pitch >= part.key_min && pitch <= part.key_max {
                    if let Some(path) = resolve_sample(&part.path, &part.rel) {
                        if let Some(voice) = get_pitched_voice(&path, part.root, pitch, &cache, &voice_cache) {
                            let mut mix = mix_arc.lock().unwrap();
                            for i in 0..(voice.len() / 2).min(num_samples - at.min(num_samples)) {
                                let mix_idx = (at + i) * 2;
                                mix[mix_idx] += voice[i * 2] * vel_gain;
                                mix[mix_idx + 1] += voice[i * 2 + 1] * vel_gain;
                            }
                            voiced = true;
                            break;
                        }
                    }
                }
            }
            if !voiced {
                // Fallback synth
                let freq = 440.0 * (2.0f32.powf((pitch as f32 - 69.0) / 12.0));
                let synth_sig = synth_note(freq, (dur_b * spb) as f32, vel, sample_rate);
                let mut mix = mix_arc.lock().unwrap();
                for i in 0..synth_sig.len().min((num_samples - at.min(num_samples)) * 2) {
                    if at * 2 + i < mix.len() {
                        mix[at * 2 + i] += synth_sig[i] * vel_gain * 0.3;
                    }
                }
            }
        }
    });
    
    Ok(Arc::try_unwrap(mix_arc).unwrap().into_inner().unwrap())
}

fn expand_midi_notes(mc: &MidiClip) -> Vec<(f64, f64, u8, u8)> {
    let mut out = Vec::new();
    let span = mc.end - mc.start;
    if mc.loop_on && mc.loop_end > mc.loop_start {
        let loop_len = mc.loop_end - mc.loop_start;
        for note in &mc.notes {
            let rel = note.time - mc.loop_start;
            if rel < 0.0 || rel >= loop_len { continue; }
            let mut k = 0.0;
            while k * loop_len < span {
                let ab = mc.start + k * loop_len + rel;
                if ab >= mc.end { break; }
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

fn synth_note(freq: f32, dur_sec: f32, vel: u8, sr: u32) -> Vec<f32> {
    let n = (dur_sec * sr as f32) as usize;
    (0..n).map(|i| {
        let t = i as f32 / sr as f32;
        let env = (-t * 1.6).exp();
        (2.0 * PI * freq * t).sin() * (vel as f32 / 127.0) * env
    }).collect()
}

fn decode_audio(path: &Path) -> Result<Vec<f32>, String> {
    let src = File::open(path).map_err(|e| e.to_string())?;
    let mss = MediaSourceStream::new(Box::new(src), Default::default());
    let probe = symphonia::default::get_probe();
    let mut format = probe.format(&Hint::default(), mss, &Default::default(), &Default::default())
        .map_err(|e| e.to_string())?.format;
    
    let track = format.tracks().iter().find(|t| t.codec_params.codec != symphonia::core::codecs::CODEC_TYPE_NULL)
        .ok_or("No track")?;
    let mut decoder = symphonia::default::get_codecs().make(&track.codec_params, &Default::default())
        .map_err(|e| e.to_string())?;
    
    let mut samples = Vec::new();
    loop {
        let packet = match format.next_packet() {
            Ok(p) => p,
            Err(_) => break,
        };
        let decoded = decoder.decode(&packet).map_err(|e| e.to_string())?;
        if let AudioBufferRef::F32(buf) = decoded {
            let spec = *buf.spec();
            let channels = spec.channels.count();
            let frames = buf.frames();
            for i in 0..frames {
                for ch in 0..channels {
                    samples.push(buf.chan(ch)[i]);
                }
                if channels == 1 {
                    samples.push(buf.chan(0)[i]);
                }
            }
        }
    }
    Ok(samples)
}

fn decode_and_cache(path: &Path, cache: &Arc<Mutex<SampleCache>>) -> Result<Arc<Vec<f32>>, String> {
    let mut c = cache.lock().unwrap();
    if let Some(s) = c.get(path) {
        Ok(s.clone())
    } else {
        let s = Arc::new(decode_audio(path)?);
        c.put(path.to_path_buf(), s.clone());
        Ok(s)
    }
}

fn get_pitched_voice(path: &Path, root: u8, pitch: u8, cache: &Arc<Mutex<SampleCache>>, vcache: &Arc<Mutex<VoiceCache>>) -> Option<Arc<Vec<f32>>> {
    let semi = (pitch as i8) - (root as i8);
    let mut vc = vcache.lock().unwrap();
    if let Some(v) = vc.get(&(path.to_path_buf(), semi)) {
        return Some(v.clone());
    }
    
    let base = decode_and_cache(path, cache).ok()?;
    let pitched = Arc::new(pitch_resample(&base, semi));
    vc.put((path.to_path_buf(), semi), pitched.clone());
    Some(pitched)
}

fn pitch_resample(audio: &[f32], semitones: i8) -> Vec<f32> {
    if semitones == 0 { return audio.to_vec(); }
    let ratio = 2.0f32.powf(semitones as f32 / 12.0);
    let n = audio.len() / 2;
    let new_n = (n as f32 / ratio) as usize;
    let mut out = vec![0.0f32; new_n * 2];
    for i in 0..new_n {
        let old_i = (i as f32 * ratio) as usize;
        if old_i * 2 + 1 < audio.len() {
            out[i * 2] = audio[old_i * 2];
            out[i * 2 + 1] = audio[old_i * 2 + 1];
        }
    }
    out
}

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
