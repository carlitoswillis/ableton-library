// sketch/engine.rs
//! Sketch rendering engine.

use std::path::{Path, PathBuf};
use crate::sketch::parser::{SketchSetData};
use hound::{WavSpec, WavWriter};
use symphonia::core::audio::{AudioBufferRef, Signal};
use symphonia::core::io::MediaSourceStream;
use symphonia::core::probe::Hint;
use std::fs::File;

/// Renders the sketch preview into a buffer.
pub fn render_sketch<F>(
    data: &SketchSetData,
    _project_dir: &Path,
    max_seconds: f64,
    sample_rate: u32,
    resolve_sample: F,
) -> Result<Vec<f32>, String> 
where F: Fn(&Option<String>, &Option<String>) -> Option<PathBuf>
{
    // Total duration in seconds based on data.clips and tempo
    let duration_sec = max_seconds.min(data.clips.iter().map(|c| c.end).fold(0.0, f64::max));
    let num_samples = (duration_sec * sample_rate as f64) as usize;
    
    // Mix buffer (stereo)
    let mut mix = vec![0.0f32; num_samples * 2];
    
    for clip in &data.clips {
        if let Some(path) = resolve_sample(&clip.path, &clip.rel_path) {
            if let Ok(samples) = decode_audio(&path) {
                // Mix in clip
                let start_idx = (clip.start * sample_rate as f64) as usize;
                for i in 0..(samples.len() / 2).min(num_samples - start_idx.min(num_samples)) {
                    mix[(start_idx + i) * 2] += samples[i * 2] * clip.sample_volume;
                    mix[(start_idx + i) * 2 + 1] += samples[i * 2 + 1] * clip.sample_volume;
                }
            }
        }
    }
    
    Ok(mix)
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
                // If mono, duplicate to stereo
                if channels == 1 {
                    samples.push(buf.chan(0)[i]);
                }
            }
        }
    }
    Ok(samples)
}

/// Writes the rendered buffer to a WAV file.
pub fn write_wav_file(path: &Path, samples: &[f32], sample_rate: u32) -> Result<(), String> {
    let spec = WavSpec {
        channels: 2,
        sample_rate,
        bits_per_sample: 16,
        sample_format: hound::SampleFormat::Int,
    };
    
    let mut writer = WavWriter::create(path, spec).map_err(|e| e.to_string())?;
    
    for &sample in samples {
        // Convert f32 [-1.0, 1.0] to i16
        let sample_i16 = (sample.clamp(-1.0, 1.0) * 32767.0) as i16;
        writer.write_sample(sample_i16).map_err(|e| e.to_string())?;
    }
    
    writer.finalize().map_err(|e| e.to_string())?;
    Ok(())
}
