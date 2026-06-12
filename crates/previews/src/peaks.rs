//! Decode audio (symphonia) and produce compact waveform peaks.
//!
//! Output: up to TARGET_BINS floats in 0..1 (max |sample| per bin across all
//! channels), serialized as JSON for direct canvas drawing in the UI.

use std::fs::File;
use std::path::Path;

use anyhow::{anyhow, Context, Result};
use symphonia::core::audio::SampleBuffer;
use symphonia::core::errors::Error as SymError;
use symphonia::core::io::MediaSourceStream;
use symphonia::core::probe::Hint;

pub const TARGET_BINS: usize = 1500;
/// Coarse accumulation granularity before downsampling to TARGET_BINS.
const SAMPLES_PER_COARSE_BIN: usize = 2048;

pub struct PeaksResult {
    pub duration_secs: f64,
    /// 0..1, length <= TARGET_BINS
    pub peaks: Vec<f32>,
}

pub fn extract(path: &Path) -> Result<PeaksResult> {
    let file = File::open(path).with_context(|| format!("opening {}", path.display()))?;
    let mss = MediaSourceStream::new(Box::new(file), Default::default());
    let mut hint = Hint::new();
    if let Some(ext) = path.extension() {
        hint.with_extension(&ext.to_string_lossy());
    }
    let probed = symphonia::default::get_probe()
        .format(&hint, mss, &Default::default(), &Default::default())
        .with_context(|| format!("unrecognized audio format: {}", path.display()))?;
    let mut format = probed.format;
    let track = format
        .default_track()
        .ok_or_else(|| anyhow!("no audio track in {}", path.display()))?;
    let track_id = track.id;
    let sample_rate = track.codec_params.sample_rate.unwrap_or(44_100) as f64;
    let mut decoder = symphonia::default::get_codecs()
        .make(&track.codec_params, &Default::default())
        .context("unsupported codec")?;

    let mut coarse: Vec<f32> = Vec::new();
    let mut cur_max = 0f32;
    let mut cur_count = 0usize;
    let mut total_frames = 0u64;
    let mut sample_buf: Option<SampleBuffer<f32>> = None;

    loop {
        let packet = match format.next_packet() {
            Ok(p) => p,
            Err(SymError::IoError(e)) if e.kind() == std::io::ErrorKind::UnexpectedEof => break,
            Err(SymError::ResetRequired) => break,
            Err(e) => return Err(anyhow!("read error in {}: {e}", path.display())),
        };
        if packet.track_id() != track_id {
            continue;
        }
        let decoded = match decoder.decode(&packet) {
            Ok(d) => d,
            Err(SymError::DecodeError(_)) => continue, // skip corrupt frame
            Err(SymError::IoError(e)) if e.kind() == std::io::ErrorKind::UnexpectedEof => break,
            Err(e) => return Err(anyhow!("decode error in {}: {e}", path.display())),
        };
        let spec = *decoded.spec();
        let channels = spec.channels.count().max(1);
        let buf = sample_buf.get_or_insert_with(|| {
            SampleBuffer::<f32>::new(decoded.capacity() as u64, spec)
        });
        buf.copy_interleaved_ref(decoded);
        let samples = buf.samples();
        for frame in samples.chunks(channels) {
            let mut m = 0f32;
            for s in frame {
                m = m.max(s.abs());
            }
            cur_max = cur_max.max(m);
            cur_count += 1;
            if cur_count == SAMPLES_PER_COARSE_BIN {
                coarse.push(cur_max.min(1.0));
                cur_max = 0.0;
                cur_count = 0;
            }
            total_frames += 1;
        }
    }
    if cur_count > 0 {
        coarse.push(cur_max.min(1.0));
    }
    if total_frames == 0 {
        return Err(anyhow!("no audio decoded from {}", path.display()));
    }

    // Downsample coarse bins to TARGET_BINS by max-grouping.
    let peaks = if coarse.len() <= TARGET_BINS {
        coarse
    } else {
        let mut out = Vec::with_capacity(TARGET_BINS);
        for i in 0..TARGET_BINS {
            let start = i * coarse.len() / TARGET_BINS;
            let end = ((i + 1) * coarse.len() / TARGET_BINS).max(start + 1);
            out.push(coarse[start..end].iter().cloned().fold(0f32, f32::max));
        }
        out
    };

    Ok(PeaksResult {
        duration_secs: total_frames as f64 / sample_rate,
        peaks,
    })
}

/// Peaks as compact JSON (3 decimal places), ready for the previews table.
pub fn to_json(p: &[f32]) -> String {
    let mut s = String::with_capacity(p.len() * 6 + 2);
    s.push('[');
    for (i, v) in p.iter().enumerate() {
        if i > 0 {
            s.push(',');
        }
        s.push_str(&format!("{:.3}", v));
    }
    s.push(']');
    s
}
