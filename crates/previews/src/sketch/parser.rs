// sketch/parser.rs
//! Independent parsing pass for sketch renderer.
//! Must NOT modify `als-core`.
//! Mirrors required subset of `.als` structure.

use quick_xml::events::{BytesStart, Event};
use quick_xml::Reader;
use std::fs::File;
use std::io::BufReader;
use std::path::Path;
use flate2::read::GzDecoder;

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct MultiSamplePart {
    pub key_min: u8,
    pub key_max: u8,
    pub root: u8,
    pub sstart: u64,
    pub send: Option<u64>,
    pub path: Option<String>,
    pub rel: Option<String>,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct Track {
    pub mute: bool,
    pub solo: bool,
    pub kind: String,
    pub name: Option<String>,
    pub parts: Vec<MultiSamplePart>,
    pub vol: f32,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct WarpMarker {
    pub sec_time: f64,
    pub beat_time: f64,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct AudioClip {
    pub start: f64,
    pub end: f64,
    pub loop_start: f64,
    pub loop_on: bool,
    pub sample_volume: f32,
    pub fade_in: f64,
    pub fade_out: f64,
    pub path: Option<String>,
    pub rel_path: Option<String>,
    pub warp: Vec<WarpMarker>,
    pub is_warped: bool,
    pub name: Option<String>,
    pub track: usize,
    pub disabled: bool,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct MidiNote {
    pub time: f64,
    pub duration: f64,
    pub pitch: u8,
    pub velocity: u8,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct MidiClip {
    pub start: f64,
    pub end: f64,
    pub loop_start: f64,
    pub loop_end: f64,
    pub loop_on: bool,
    pub disabled: bool,
    pub track: usize,
    pub notes: Vec<MidiNote>,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct SketchSetData {
    pub tempo: f64,
    pub clips: Vec<AudioClip>,
    pub tracks: Vec<Track>,
    pub midi_clips: Vec<MidiClip>,
}

fn get_attr(e: &BytesStart, name: &str) -> Option<String> {
    e.try_get_attribute(name)
        .ok()
        .flatten()
        .and_then(|a| a.unescape_value().ok().map(|v| v.into_owned()))
}

fn get_val_string(e: &BytesStart) -> Option<String> {
    get_attr(e, "Value")
}

fn get_val_f64(e: &BytesStart) -> Option<f64> {
    get_val_string(e).and_then(|s| s.parse().ok())
}

fn get_val_f32(e: &BytesStart) -> Option<f32> {
    get_val_string(e).and_then(|s| s.parse().ok())
}

fn get_val_u8(e: &BytesStart) -> Option<u8> {
    get_val_string(e).and_then(|s| s.parse::<f64>().ok().map(|f| f as u8))
}

fn get_val_u64(e: &BytesStart) -> Option<u64> {
    get_val_string(e).and_then(|s| s.parse::<f64>().ok().map(|f| f as u64))
}

fn get_val_bool(e: &BytesStart) -> Option<bool> {
    get_val_string(e).map(|s| s == "true")
}

struct KeyTrackAcc {
    key: Option<u8>,
    notes: Vec<(f64, f64, u8)>,
}

/// Parse essential data for sketch rendering.
/// Streaming pass only.
pub fn parse_sketch_data(als_path: &Path) -> Result<SketchSetData, String> {
    let gz = GzDecoder::new(File::open(als_path).map_err(|e| e.to_string())?);
    let mut reader = Reader::from_reader(BufReader::new(gz));
    reader.trim_text(true);

    let mut buf = Vec::new();
    let mut stack: Vec<String> = Vec::new();

    let mut tempo_cands = Vec::new();
    let mut clips = Vec::new();
    let mut midi_clips = Vec::new();
    let mut tracks = Vec::new();
    let mut cur_track: Option<usize> = None;
    let mut ctx_master: usize = 0;

    let mut cur_audio_clip: Option<AudioClip> = None;
    let mut cur_midi_clip: Option<MidiClip> = None;
    let mut cur_key_track: Option<KeyTrackAcc> = None;
    let mut cur_part: Option<MultiSamplePart> = None;

    let mut in_keyrange: usize = 0;
    let mut in_loop: usize = 0;
    let mut in_fileref: usize = 0;
    let mut in_fades: usize = 0;

    let track_tags = ["AudioTrack", "MidiTrack", "GroupTrack", "ReturnTrack"];

    loop {
        match reader.read_event_into(&mut buf) {
            Ok(Event::Start(ref e)) => {
                let tag = String::from_utf8_lossy(e.name().as_ref()).into_owned();
                stack.push(tag.clone());

                // Process start logic
                if track_tags.contains(&tag.as_str()) && stack.len() >= 2 && stack[stack.len() - 2] == "Tracks" {
                    tracks.push(Track {
                        mute: false,
                        solo: false,
                        kind: tag.clone(),
                        name: None,
                        parts: Vec::new(),
                        vol: 1.0,
                    });
                    cur_track = Some(tracks.len() - 1);
                }

                if tag == "MasterTrack" || tag == "MainTrack" {
                    ctx_master += 1;
                } else if tag == "Loop" {
                    in_loop += 1;
                } else if tag == "FileRef" {
                    in_fileref += 1;
                } else if tag == "Fades" {
                    in_fades += 1;
                } else if tag == "AudioClip" && !stack.contains(&"ClipSlot".to_string()) {
                    cur_audio_clip = Some(AudioClip {
                        start: 0.0,
                        end: 0.0,
                        loop_start: 0.0,
                        loop_on: false,
                        sample_volume: 1.0,
                        fade_in: 0.0,
                        fade_out: 0.0,
                        path: None,
                        rel_path: None,
                        warp: Vec::new(),
                        is_warped: false,
                        name: None,
                        track: cur_track.unwrap_or(0),
                        disabled: false,
                    });
                } else if tag == "MidiClip" && !stack.contains(&"ClipSlot".to_string()) {
                    cur_midi_clip = Some(MidiClip {
                        start: 0.0,
                        end: 0.0,
                        loop_start: 0.0,
                        loop_end: 0.0,
                        loop_on: false,
                        disabled: false,
                        track: cur_track.unwrap_or(0),
                        notes: Vec::new(),
                    });
                } else if tag == "KeyTrack" && cur_midi_clip.is_some() {
                    cur_key_track = Some(KeyTrackAcc {
                        key: None,
                        notes: Vec::new(),
                    });
                } else if tag == "MidiNoteEvent" && cur_key_track.is_some() {
                    let is_enabled = get_attr(e, "IsEnabled").unwrap_or_else(|| "true".to_string());
                    if is_enabled != "false" {
                        let time = get_attr(e, "Time").and_then(|s| s.parse().ok()).unwrap_or(0.0);
                        let duration = get_attr(e, "Duration").and_then(|s| s.parse().ok()).unwrap_or(0.0);
                        let velocity = get_attr(e, "Velocity").and_then(|s| s.parse::<f64>().ok().map(|f| f as u8)).unwrap_or(100);
                        if let Some(ref mut kt) = cur_key_track {
                            kt.notes.push((time, duration, velocity));
                        }
                    }
                } else if tag == "WarpMarker" && cur_audio_clip.is_some() {
                    let sec_time = get_attr(e, "SecTime").and_then(|s| s.parse().ok());
                    let beat_time = get_attr(e, "BeatTime").and_then(|s| s.parse().ok());
                    if let (Some(st), Some(bt)) = (sec_time, beat_time) {
                        if let Some(ref mut clip) = cur_audio_clip {
                            clip.warp.push(WarpMarker { sec_time: st, beat_time: bt });
                        }
                    }
                }

                if tag == "MidiKey" && cur_key_track.is_some() {
                    if let Some(k) = get_val_u8(e) {
                        if let Some(ref mut kt) = cur_key_track {
                            kt.key = Some(k);
                        }
                    }
                }

                if tag == "MultiSamplePart" && cur_track.is_some() {
                    cur_part = Some(MultiSamplePart {
                        key_min: 0,
                        key_max: 127,
                        root: 60,
                        sstart: 0,
                        send: None,
                        path: None,
                        rel: None,
                    });
                } else if tag == "KeyRange" && cur_part.is_some() {
                    in_keyrange += 1;
                }

                if let Some(ref mut part) = cur_part {
                    if tag == "RootKey" && stack.len() >= 2 && stack[stack.len() - 2] == "MultiSamplePart" {
                        if let Some(val) = get_val_u8(e) {
                            part.root = val;
                        }
                    } else if tag == "SampleStart" && stack.len() >= 2 && stack[stack.len() - 2] == "MultiSamplePart" {
                        if let Some(val) = get_val_u64(e) {
                            part.sstart = val;
                        }
                    } else if tag == "SampleEnd" && stack.len() >= 2 && stack[stack.len() - 2] == "MultiSamplePart" {
                        part.send = get_val_u64(e);
                    } else if in_keyrange > 0 && tag == "Min" {
                        if let Some(val) = get_val_u8(e) {
                            part.key_min = val;
                        }
                    } else if in_keyrange > 0 && tag == "Max" {
                        if let Some(val) = get_val_u8(e) {
                            part.key_max = val;
                        }
                    } else if in_fileref > 0 && tag == "Path" && part.path.is_none() {
                        part.path = get_val_string(e);
                    } else if in_fileref > 0 && tag == "RelativePath" && part.rel.is_none() {
                        part.rel = get_val_string(e);
                    }
                }

                // Track Speaker (mute)
                if tag == "Manual" && stack.len() >= 5
                    && stack[stack.len() - 2] == "Speaker"
                    && stack[stack.len() - 3] == "Mixer"
                    && stack[stack.len() - 4] == "DeviceChain"
                    && track_tags.contains(&stack[stack.len() - 5].as_str())
                {
                    if let Some(ct) = cur_track {
                        if let Some(val) = get_val_bool(e) {
                            if !val {
                                tracks[ct].mute = true;
                            }
                        }
                    }
                }

                // Track Solo
                if tag == "Solo" && stack.len() >= 4
                    && stack[stack.len() - 2] == "Mixer"
                    && stack[stack.len() - 3] == "DeviceChain"
                    && track_tags.contains(&stack[stack.len() - 4].as_str())
                {
                    if let Some(ct) = cur_track {
                        if let Some(val) = get_val_bool(e) {
                            if val {
                                tracks[ct].solo = true;
                            }
                        }
                    }
                }

                // Track Volume
                if tag == "Manual" && stack.len() >= 5
                    && stack[stack.len() - 2] == "Volume"
                    && stack[stack.len() - 3] == "Mixer"
                    && stack[stack.len() - 4] == "DeviceChain"
                    && track_tags.contains(&stack[stack.len() - 5].as_str())
                {
                    if let Some(ct) = cur_track {
                        if let Some(val) = get_val_f32(e) {
                            tracks[ct].vol = val;
                        }
                    }
                }

                // Track EffectiveName
                if tag == "EffectiveName" && stack.len() >= 3
                    && stack[stack.len() - 2] == "Name"
                    && track_tags.contains(&stack[stack.len() - 3].as_str())
                {
                    if let Some(ct) = cur_track {
                        if tracks[ct].name.is_none() {
                            tracks[ct].name = get_val_string(e);
                        }
                    }
                }

                // Audio clip field captures
                if let Some(ref mut clip) = cur_audio_clip {
                    if tag == "CurrentStart" {
                        if let Some(val) = get_val_f64(e) {
                            clip.start = val;
                        }
                    } else if tag == "CurrentEnd" {
                        if let Some(val) = get_val_f64(e) {
                            clip.end = val;
                        }
                    } else if tag == "Name" {
                        clip.name = get_val_string(e);
                    } else if tag == "IsWarped" {
                        if let Some(val) = get_val_bool(e) {
                            clip.is_warped = val;
                        }
                    } else if tag == "Disabled" && stack.len() >= 2 && stack[stack.len() - 2] == "AudioClip" {
                        if let Some(val) = get_val_bool(e) {
                            clip.disabled = val;
                        }
                    } else if tag == "SampleVolume" {
                        if let Some(val) = get_val_f32(e) {
                            clip.sample_volume = val;
                        }
                    } else if in_loop > 0 {
                        if tag == "LoopStart" {
                            if let Some(val) = get_val_f64(e) {
                                clip.loop_start = val;
                            }
                        } else if tag == "LoopOn" {
                            if let Some(val) = get_val_bool(e) {
                                clip.loop_on = val;
                            }
                        }
                    } else if in_fades > 0 {
                        if tag == "FadeInLength" {
                            if let Some(val) = get_val_f64(e) {
                                clip.fade_in = val;
                            }
                        } else if tag == "FadeOutLength" {
                            if let Some(val) = get_val_f64(e) {
                                clip.fade_out = val;
                            }
                        }
                    } else if in_fileref > 0 {
                        if tag == "Path" && clip.path.is_none() {
                            clip.path = get_val_string(e);
                        } else if tag == "RelativePath" && clip.rel_path.is_none() {
                            clip.rel_path = get_val_string(e);
                        }
                    }
                }

                // MIDI clip field captures
                if let Some(ref mut mc) = cur_midi_clip {
                    if tag == "CurrentStart" {
                        if let Some(val) = get_val_f64(e) {
                            mc.start = val;
                        }
                    } else if tag == "CurrentEnd" {
                        if let Some(val) = get_val_f64(e) {
                            mc.end = val;
                        }
                    } else if tag == "Disabled" && stack.len() >= 2 && stack[stack.len() - 2] == "MidiClip" {
                        if let Some(val) = get_val_bool(e) {
                            mc.disabled = val;
                        }
                    } else if in_loop > 0 {
                        if tag == "LoopStart" {
                            if let Some(val) = get_val_f64(e) {
                                mc.loop_start = val;
                            }
                        } else if tag == "LoopEnd" {
                            if let Some(val) = get_val_f64(e) {
                                mc.loop_end = val;
                            }
                        } else if tag == "LoopOn" {
                            if let Some(val) = get_val_bool(e) {
                                mc.loop_on = val;
                            }
                        }
                    }
                }

                // Tempo Manual
                if tag == "Manual" && stack.len() >= 2 && stack[stack.len() - 2] == "Tempo" {
                    if let Some(val) = get_val_f64(e) {
                        tempo_cands.push((val, ctx_master > 0));
                    }
                }
            }
            Ok(Event::Empty(ref e)) => {
                let tag = String::from_utf8_lossy(e.name().as_ref()).into_owned();
                stack.push(tag.clone());

                // Treat Empty as both Start and End:

                // Process start logic
                if track_tags.contains(&tag.as_str()) && stack.len() >= 2 && stack[stack.len() - 2] == "Tracks" {
                    tracks.push(Track {
                        mute: false,
                        solo: false,
                        kind: tag.clone(),
                        name: None,
                        parts: Vec::new(),
                        vol: 1.0,
                    });
                    cur_track = Some(tracks.len() - 1);
                }

                if tag == "MasterTrack" || tag == "MainTrack" {
                    ctx_master += 1;
                } else if tag == "Loop" {
                    in_loop += 1;
                } else if tag == "FileRef" {
                    in_fileref += 1;
                } else if tag == "Fades" {
                    in_fades += 1;
                } else if tag == "AudioClip" && !stack.contains(&"ClipSlot".to_string()) {
                    cur_audio_clip = Some(AudioClip {
                        start: 0.0,
                        end: 0.0,
                        loop_start: 0.0,
                        loop_on: false,
                        sample_volume: 1.0,
                        fade_in: 0.0,
                        fade_out: 0.0,
                        path: None,
                        rel_path: None,
                        warp: Vec::new(),
                        is_warped: false,
                        name: None,
                        track: cur_track.unwrap_or(0),
                        disabled: false,
                    });
                } else if tag == "MidiClip" && !stack.contains(&"ClipSlot".to_string()) {
                    cur_midi_clip = Some(MidiClip {
                        start: 0.0,
                        end: 0.0,
                        loop_start: 0.0,
                        loop_end: 0.0,
                        loop_on: false,
                        disabled: false,
                        track: cur_track.unwrap_or(0),
                        notes: Vec::new(),
                    });
                } else if tag == "KeyTrack" && cur_midi_clip.is_some() {
                    cur_key_track = Some(KeyTrackAcc {
                        key: None,
                        notes: Vec::new(),
                    });
                } else if tag == "MidiNoteEvent" && cur_key_track.is_some() {
                    let is_enabled = get_attr(e, "IsEnabled").unwrap_or_else(|| "true".to_string());
                    if is_enabled != "false" {
                        let time = get_attr(e, "Time").and_then(|s| s.parse().ok()).unwrap_or(0.0);
                        let duration = get_attr(e, "Duration").and_then(|s| s.parse().ok()).unwrap_or(0.0);
                        let velocity = get_attr(e, "Velocity").and_then(|s| s.parse::<f64>().ok().map(|f| f as u8)).unwrap_or(100);
                        if let Some(ref mut kt) = cur_key_track {
                            kt.notes.push((time, duration, velocity));
                        }
                    }
                } else if tag == "WarpMarker" && cur_audio_clip.is_some() {
                    let sec_time = get_attr(e, "SecTime").and_then(|s| s.parse().ok());
                    let beat_time = get_attr(e, "BeatTime").and_then(|s| s.parse().ok());
                    if let (Some(st), Some(bt)) = (sec_time, beat_time) {
                        if let Some(ref mut clip) = cur_audio_clip {
                            clip.warp.push(WarpMarker { sec_time: st, beat_time: bt });
                        }
                    }
                }

                if tag == "MidiKey" && cur_key_track.is_some() {
                    if let Some(k) = get_val_u8(e) {
                        if let Some(ref mut kt) = cur_key_track {
                            kt.key = Some(k);
                        }
                    }
                }

                if tag == "MultiSamplePart" && cur_track.is_some() {
                    cur_part = Some(MultiSamplePart {
                        key_min: 0,
                        key_max: 127,
                        root: 60,
                        sstart: 0,
                        send: None,
                        path: None,
                        rel: None,
                    });
                } else if tag == "KeyRange" && cur_part.is_some() {
                    in_keyrange += 1;
                }

                if let Some(ref mut part) = cur_part {
                    if tag == "RootKey" && stack.len() >= 2 && stack[stack.len() - 2] == "MultiSamplePart" {
                        if let Some(val) = get_val_u8(e) {
                            part.root = val;
                        }
                    } else if tag == "SampleStart" && stack.len() >= 2 && stack[stack.len() - 2] == "MultiSamplePart" {
                        if let Some(val) = get_val_u64(e) {
                            part.sstart = val;
                        }
                    } else if tag == "SampleEnd" && stack.len() >= 2 && stack[stack.len() - 2] == "MultiSamplePart" {
                        part.send = get_val_u64(e);
                    } else if in_keyrange > 0 && tag == "Min" {
                        if let Some(val) = get_val_u8(e) {
                            part.key_min = val;
                        }
                    } else if in_keyrange > 0 && tag == "Max" {
                        if let Some(val) = get_val_u8(e) {
                            part.key_max = val;
                        }
                    } else if in_fileref > 0 && tag == "Path" && part.path.is_none() {
                        part.path = get_val_string(e);
                    } else if in_fileref > 0 && tag == "RelativePath" && part.rel.is_none() {
                        part.rel = get_val_string(e);
                    }
                }

                // Track Speaker (mute)
                if tag == "Manual" && stack.len() >= 5
                    && stack[stack.len() - 2] == "Speaker"
                    && stack[stack.len() - 3] == "Mixer"
                    && stack[stack.len() - 4] == "DeviceChain"
                    && track_tags.contains(&stack[stack.len() - 5].as_str())
                {
                    if let Some(ct) = cur_track {
                        if let Some(val) = get_val_bool(e) {
                            if !val {
                                tracks[ct].mute = true;
                            }
                        }
                    }
                }

                // Track Solo
                if tag == "Solo" && stack.len() >= 4
                    && stack[stack.len() - 2] == "Mixer"
                    && stack[stack.len() - 3] == "DeviceChain"
                    && track_tags.contains(&stack[stack.len() - 4].as_str())
                {
                    if let Some(ct) = cur_track {
                        if let Some(val) = get_val_bool(e) {
                            if val {
                                tracks[ct].solo = true;
                            }
                        }
                    }
                }

                // Track Volume
                if tag == "Manual" && stack.len() >= 5
                    && stack[stack.len() - 2] == "Volume"
                    && stack[stack.len() - 3] == "Mixer"
                    && stack[stack.len() - 4] == "DeviceChain"
                    && track_tags.contains(&stack[stack.len() - 5].as_str())
                {
                    if let Some(ct) = cur_track {
                        if let Some(val) = get_val_f32(e) {
                            tracks[ct].vol = val;
                        }
                    }
                }

                // Track EffectiveName
                if tag == "EffectiveName" && stack.len() >= 3
                    && stack[stack.len() - 2] == "Name"
                    && track_tags.contains(&stack[stack.len() - 3].as_str())
                {
                    if let Some(ct) = cur_track {
                        if tracks[ct].name.is_none() {
                            tracks[ct].name = get_val_string(e);
                        }
                    }
                }

                // Audio clip field captures
                if let Some(ref mut clip) = cur_audio_clip {
                    if tag == "CurrentStart" {
                        if let Some(val) = get_val_f64(e) {
                            clip.start = val;
                        }
                    } else if tag == "CurrentEnd" {
                        if let Some(val) = get_val_f64(e) {
                            clip.end = val;
                        }
                    } else if tag == "Name" {
                        clip.name = get_val_string(e);
                    } else if tag == "IsWarped" {
                        if let Some(val) = get_val_bool(e) {
                            clip.is_warped = val;
                        }
                    } else if tag == "Disabled" && stack.len() >= 2 && stack[stack.len() - 2] == "AudioClip" {
                        if let Some(val) = get_val_bool(e) {
                            clip.disabled = val;
                        }
                    } else if tag == "SampleVolume" {
                        if let Some(val) = get_val_f32(e) {
                            clip.sample_volume = val;
                        }
                    } else if in_loop > 0 {
                        if tag == "LoopStart" {
                            if let Some(val) = get_val_f64(e) {
                                clip.loop_start = val;
                            }
                        } else if tag == "LoopOn" {
                            if let Some(val) = get_val_bool(e) {
                                clip.loop_on = val;
                            }
                        }
                    } else if in_fades > 0 {
                        if tag == "FadeInLength" {
                            if let Some(val) = get_val_f64(e) {
                                clip.fade_in = val;
                            }
                        } else if tag == "FadeOutLength" {
                            if let Some(val) = get_val_f64(e) {
                                clip.fade_out = val;
                            }
                        }
                    } else if in_fileref > 0 {
                        if tag == "Path" && clip.path.is_none() {
                            clip.path = get_val_string(e);
                        } else if tag == "RelativePath" && clip.rel_path.is_none() {
                            clip.rel_path = get_val_string(e);
                        }
                    }
                }

                // MIDI clip field captures
                if let Some(ref mut mc) = cur_midi_clip {
                    if tag == "CurrentStart" {
                        if let Some(val) = get_val_f64(e) {
                            mc.start = val;
                        }
                    } else if tag == "CurrentEnd" {
                        if let Some(val) = get_val_f64(e) {
                            mc.end = val;
                        }
                    } else if tag == "Disabled" && stack.len() >= 2 && stack[stack.len() - 2] == "MidiClip" {
                        if let Some(val) = get_val_bool(e) {
                            mc.disabled = val;
                        }
                    } else if in_loop > 0 {
                        if tag == "LoopStart" {
                            if let Some(val) = get_val_f64(e) {
                                mc.loop_start = val;
                            }
                        } else if tag == "LoopEnd" {
                            if let Some(val) = get_val_f64(e) {
                                mc.loop_end = val;
                            }
                        } else if tag == "LoopOn" {
                            if let Some(val) = get_val_bool(e) {
                                mc.loop_on = val;
                            }
                        }
                    }
                }

                // Tempo Manual
                if tag == "Manual" && stack.len() >= 2 && stack[stack.len() - 2] == "Tempo" {
                    if let Some(val) = get_val_f64(e) {
                        tempo_cands.push((val, ctx_master > 0));
                    }
                }

                // Process end logic
                if track_tags.contains(&tag.as_str()) && stack.len() >= 2 && stack[stack.len() - 2] == "Tracks" {
                    cur_track = None;
                }
                if tag == "MasterTrack" || tag == "MainTrack" {
                    ctx_master = ctx_master.saturating_sub(1);
                } else if tag == "Loop" {
                    in_loop = in_loop.saturating_sub(1);
                } else if tag == "FileRef" {
                    in_fileref = in_fileref.saturating_sub(1);
                } else if tag == "Fades" {
                    in_fades = in_fades.saturating_sub(1);
                } else if tag == "KeyRange" && cur_part.is_some() {
                    in_keyrange = in_keyrange.saturating_sub(1);
                } else if tag == "MultiSamplePart" && cur_part.is_some() {
                    if let Some(part) = cur_part.take() {
                        if (part.path.is_some() || part.rel.is_some()) && cur_track.is_some() {
                            if let Some(ct) = cur_track {
                                tracks[ct].parts.push(part);
                            }
                        }
                    }
                } else if tag == "KeyTrack" && cur_key_track.is_some() {
                    if let Some(kt) = cur_key_track.take() {
                        if let Some(key) = kt.key {
                            if let Some(ref mut mc) = cur_midi_clip {
                                for (t, d, v) in kt.notes {
                                    mc.notes.push(MidiNote { time: t, duration: d, pitch: key, velocity: v });
                                }
                            }
                        }
                    }
                } else if tag == "AudioClip" && cur_audio_clip.is_some() {
                    if let Some(clip) = cur_audio_clip.take() {
                        clips.push(clip);
                    }
                } else if tag == "MidiClip" && cur_midi_clip.is_some() {
                    if let Some(mc) = cur_midi_clip.take() {
                        if !mc.notes.is_empty() {
                            midi_clips.push(mc);
                        }
                    }
                }

                stack.pop();
            }
            Ok(Event::End(ref e)) => {
                let tag = String::from_utf8_lossy(e.name().as_ref()).into_owned();

                // Check stack consistency
                if !stack.is_empty() && stack[stack.len() - 1] == tag {
                    // Process end logic
                    if track_tags.contains(&tag.as_str()) && stack.len() >= 2 && stack[stack.len() - 2] == "Tracks" {
                        cur_track = None;
                    }
                    if tag == "MasterTrack" || tag == "MainTrack" {
                        ctx_master = ctx_master.saturating_sub(1);
                    } else if tag == "Loop" {
                        in_loop = in_loop.saturating_sub(1);
                    } else if tag == "FileRef" {
                        in_fileref = in_fileref.saturating_sub(1);
                    } else if tag == "Fades" {
                        in_fades = in_fades.saturating_sub(1);
                    } else if tag == "KeyRange" && cur_part.is_some() {
                        in_keyrange = in_keyrange.saturating_sub(1);
                    } else if tag == "MultiSamplePart" && cur_part.is_some() {
                        if let Some(part) = cur_part.take() {
                            if (part.path.is_some() || part.rel.is_some()) && cur_track.is_some() {
                                if let Some(ct) = cur_track {
                                    tracks[ct].parts.push(part);
                                }
                            }
                        }
                    } else if tag == "KeyTrack" && cur_key_track.is_some() {
                        if let Some(kt) = cur_key_track.take() {
                            if let Some(key) = kt.key {
                                if let Some(ref mut mc) = cur_midi_clip {
                                    for (t, d, v) in kt.notes {
                                        mc.notes.push(MidiNote { time: t, duration: d, pitch: key, velocity: v });
                                    }
                                }
                            }
                        }
                    } else if tag == "AudioClip" && cur_audio_clip.is_some() {
                        if let Some(clip) = cur_audio_clip.take() {
                            clips.push(clip);
                        }
                    } else if tag == "MidiClip" && cur_midi_clip.is_some() {
                        if let Some(mc) = cur_midi_clip.take() {
                            if !mc.notes.is_empty() {
                                midi_clips.push(mc);
                            }
                        }
                    }

                    stack.pop();
                }
            }
            Ok(Event::Eof) => break,
            Err(e) => return Err(e.to_string()),
            _ => {}
        }
        buf.clear();
    }

    let tempo = tempo_cands
        .iter()
        .find(|(_, is_master)| *is_master)
        .map(|(v, _)| *v)
        .unwrap_or_else(|| {
            tempo_cands.first().map(|(v, _)| *v).unwrap_or(120.0)
        });

    Ok(SketchSetData {
        tempo,
        clips,
        tracks,
        midi_clips,
    })
}
