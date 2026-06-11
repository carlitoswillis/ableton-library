//! Streaming .als parser: gzip -> XML events -> SetSnapshot.
//!
//! Design rules (see ai/AGENTS.md):
//! - Streaming only. An .als can decompress to 100s of MB; never build a DOM.
//! - Lenient extraction. Unknown elements are ignored; missing fields become
//!   warnings on the snapshot, never errors. Must tolerate Live 9..12+.
//! - Bulk subtrees (automation points, MIDI notes, plugin binary state) are
//!   skipped wholesale — they are most of the file and contain nothing we want.
//!
//! Logic mirrors tools/reference_extract.py (the test oracle) exactly.

use std::fs::File;
use std::io::BufReader;
use std::path::Path;

use chrono::{DateTime, Utc};
use flate2::read::GzDecoder;
use quick_xml::events::{BytesStart, Event};
use quick_xml::Reader;
use sha2::{Digest, Sha256};

use crate::model::*;

/// Bulk subtrees skipped entirely. MUST stay in sync with the Python oracle.
const SKIP_SUBTREES: &[&str] = &[
    "AutomationEnvelopes",
    "KeyTracks",
    "Notes",
    "Events",
    "ParameterSettings",
    "ProcessorState",
    "Buffer",
    "Data",
    "AutomationTarget",
    "ModulationTarget",
];

/// "MainTrack" is the Live 12+ rename of "MasterTrack".
const MASTER_TAGS: &[&str] = &["MasterTrack", "MainTrack"];

/// Wrapper device elements whose real identity comes from the nested
/// *PluginInfo element — not reported as native devices.
const PLUGIN_WRAPPERS: &[&str] = &[
    "PluginDevice",
    "AuPluginDevice",
    "VstPluginDevice",
    "Vst3PluginDevice",
];

const AUDIO_EXTS: &[&str] = &[".wav", ".aif", ".aiff", ".mp3", ".flac", ".m4a", ".ogg"];

#[derive(thiserror::Error, Debug)]
pub enum ParseError {
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),
    #[error("xml error: {0}")]
    Xml(#[from] quick_xml::Error),
}

fn track_kind(tag: &str) -> Option<TrackKind> {
    match tag {
        "MidiTrack" => Some(TrackKind::Midi),
        "AudioTrack" => Some(TrackKind::Audio),
        "ReturnTrack" => Some(TrackKind::Return),
        "GroupTrack" => Some(TrackKind::Group),
        _ => None,
    }
}

fn plugin_kind(tag: &str) -> Option<DeviceKind> {
    match tag {
        "AuPluginInfo" => Some(DeviceKind::Au),
        "VstPluginInfo" => Some(DeviceKind::Vst),
        "Vst3PluginInfo" => Some(DeviceKind::Vst3),
        _ => None,
    }
}

/// Read one attribute as an owned String.
fn attr(e: &BytesStart, name: &str) -> Option<String> {
    e.try_get_attribute(name)
        .ok()
        .flatten()
        .and_then(|a| a.unescape_value().ok())
        .map(|v| v.into_owned())
}

/// Which mixer context are we inside? Decides whether a tempo/time-signature
/// reading is authoritative (master) or just a clip-level value.
#[derive(Clone, Copy, PartialEq)]
enum Ctx {
    None,
    Track(usize),
    Master,
}

impl Ctx {
    fn as_ref(self) -> Option<TrackRef> {
        match self {
            Ctx::None => None,
            Ctx::Track(i) => Some(TrackRef::Index(i)),
            Ctx::Master => Some(TrackRef::Master("master".into())),
        }
    }
}

struct PendingPlugin {
    tag: String, // the *PluginInfo element we are inside
    device: Device,
}

struct State {
    snap: SetSnapshot,
    project_name: String,
    ctx: Ctx,
    plugin: Option<PendingPlugin>,
    locator: Option<Locator>,
    sig_pair: Option<(Option<u32>, Option<u32>)>, // pending RemoteableTimeSignature
    tempo_cands: Vec<(f64, bool)>,                // (value, in_master_context)
    sig_cands: Vec<((u32, u32), bool)>,
    samples_seen: std::collections::HashSet<String>,
}

impl State {
    /// Handle an opening or self-closing element. `parent`/`gparent` are the
    /// enclosing element names (the element itself is not yet on the stack).
    fn element(&mut self, tag: &str, e: &BytesStart, parent: Option<&str>, gparent: Option<&str>) {
        let val = attr(e, "Value");

        if tag == "Ableton" {
            self.snap.live_version = attr(e, "Creator");
            self.snap.schema_version = attr(e, "MinorVersion");
        } else if let (Some(kind), Some("Tracks")) = (track_kind(tag), parent) {
            self.snap.tracks.push(Track { kind, name: None, color: None });
            self.ctx = Ctx::Track(self.snap.tracks.len() - 1);
        } else if MASTER_TAGS.contains(&tag) && parent == Some("LiveSet") {
            self.ctx = Ctx::Master;
        } else if tag == "EffectiveName"
            && parent == Some("Name")
            && gparent.map_or(false, |g| track_kind(g).is_some())
        {
            if let Ctx::Track(i) = self.ctx {
                let t = &mut self.snap.tracks[i];
                if t.name.is_none() {
                    t.name = val;
                }
            }
        } else if tag == "Color" && parent.map_or(false, |p| track_kind(p).is_some()) {
            if let Ctx::Track(i) = self.ctx {
                let t = &mut self.snap.tracks[i];
                if t.color.is_none() {
                    t.color = val.and_then(|v| v.parse().ok());
                }
            }
        } else if tag == "Manual" && parent == Some("Tempo") {
            if let Some(v) = val.and_then(|v| v.parse::<f64>().ok()) {
                self.tempo_cands.push((v, self.ctx == Ctx::Master));
            }
        } else if tag == "Manual" && parent == Some("TimeSignature") {
            // Encoded: value = 99 * log2(denominator) + (numerator - 1)
            if let Some(enc) = val.and_then(|v| v.parse::<u32>().ok()) {
                let sig = (enc % 99 + 1, 2u32.pow(enc / 99));
                self.sig_cands.push((sig, self.ctx == Ctx::Master));
            }
        } else if tag == "RemoteableTimeSignature" {
            self.sig_pair = Some((None, None));
        } else if parent == Some("RemoteableTimeSignature") && self.sig_pair.is_some() {
            let pair = self.sig_pair.as_mut().unwrap();
            let num = val.and_then(|v| v.parse::<u32>().ok());
            match tag {
                "Numerator" => pair.0 = num,
                "Denominator" => pair.1 = num,
                _ => {}
            }
        } else if let Some(kind) = plugin_kind(tag) {
            self.plugin = Some(PendingPlugin {
                tag: tag.to_owned(),
                device: Device {
                    track: self.ctx.as_ref(),
                    kind,
                    name: None,
                    manufacturer: None,
                },
            });
        } else if self
            .plugin
            .as_ref()
            .map_or(false, |p| parent == Some(p.tag.as_str()))
        {
            let p = self.plugin.as_mut().unwrap();
            match tag {
                "Name" | "PlugName" if p.device.name.is_none() => p.device.name = val,
                "Manufacturer" => p.device.manufacturer = val,
                _ => {}
            }
        } else if parent == Some("Devices")
            && attr(e, "Id").is_some()
            && !PLUGIN_WRAPPERS.contains(&tag)
        {
            self.snap.devices.push(Device {
                track: self.ctx.as_ref(),
                kind: DeviceKind::Native,
                name: Some(tag.to_owned()),
                manufacturer: Some("Ableton".into()),
            });
        } else if tag == "Path" && parent == Some("FileRef") {
            if let Some(path) = val {
                let lower = path.to_lowercase();
                if AUDIO_EXTS.iter().any(|ext| lower.ends_with(ext))
                    && self.samples_seen.insert(path.clone())
                {
                    let needle = format!("/{}/", self.project_name);
                    self.snap.samples.push(SampleRef {
                        in_project: path.contains(&needle),
                        exists: Path::new(&path).exists(),
                        path,
                    });
                }
            }
        } else if tag == "Locator" && parent == Some("Locators") {
            self.locator = Some(Locator { name: None, time: None });
        } else if let Some(loc) = self.locator.as_mut() {
            if parent == Some("Locator") {
                match tag {
                    "Name" => loc.name = val,
                    "Time" => loc.time = val.and_then(|v| v.parse().ok()),
                    _ => {}
                }
            }
        }
    }

    /// Handle a closing element. `parent` is the enclosing element after pop.
    fn close(&mut self, tag: &str, parent: Option<&str>) {
        if track_kind(tag).is_some() && parent == Some("Tracks") {
            self.ctx = Ctx::None;
        } else if MASTER_TAGS.contains(&tag) && parent == Some("LiveSet") {
            self.ctx = Ctx::None;
        } else if self.plugin.as_ref().map_or(false, |p| p.tag == tag) {
            let p = self.plugin.take().unwrap();
            self.snap.devices.push(p.device);
        } else if tag == "RemoteableTimeSignature" {
            if let Some((Some(n), Some(d))) = self.sig_pair.take() {
                self.sig_cands.push(((n, d), self.ctx == Ctx::Master));
            }
        } else if tag == "Locator" {
            if let Some(loc) = self.locator.take() {
                self.snap.locators.push(loc);
            }
        }
    }
}

/// Prefer a master-context candidate; fall back to first seen (with warning).
fn resolve<T: Copy>(cands: &[(T, bool)], label: &str, warnings: &mut Vec<String>) -> Option<T> {
    if let Some((v, _)) = cands.iter().find(|(_, m)| *m) {
        return Some(*v);
    }
    if let Some((v, _)) = cands.first() {
        warnings.push(format!(
            "{label} not found in master-track context; using first occurrence"
        ));
        return Some(*v);
    }
    warnings.push(format!("{label} not found"));
    None
}

fn sha256_file(path: &Path) -> std::io::Result<String> {
    let mut hasher = Sha256::new();
    let mut f = File::open(path)?;
    std::io::copy(&mut f, &mut hasher)?;
    Ok(format!("{:x}", hasher.finalize()))
}

/// Parse one `.als` file into a SetSnapshot.
/// `project_dir` is the enclosing project folder (used for in_project checks).
pub fn parse_set(als_path: &Path, project_dir: &Path) -> Result<SetSnapshot, ParseError> {
    let meta = std::fs::metadata(als_path)?;
    let mtime: DateTime<Utc> = meta.modified()?.into();

    let mut st = State {
        snap: SetSnapshot {
            // Absolute, like the oracle's os.path.abspath (no symlink resolution).
            als_path: std::path::absolute(als_path)
                .unwrap_or_else(|_| als_path.to_path_buf())
                .to_string_lossy()
                .into_owned(),
            file_size: meta.len(),
            mtime: mtime.format("%Y-%m-%dT%H:%M:%S+00:00").to_string(),
            content_hash: sha256_file(als_path)?,
            live_version: None,
            schema_version: None,
            tempo: None,
            time_signature: None,
            tracks: Vec::new(),
            devices: Vec::new(),
            samples: Vec::new(),
            locators: Vec::new(),
            warnings: Vec::new(),
        },
        project_name: project_dir
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default(),
        ctx: Ctx::None,
        plugin: None,
        locator: None,
        sig_pair: None,
        tempo_cands: Vec::new(),
        sig_cands: Vec::new(),
        samples_seen: Default::default(),
    };

    let gz = GzDecoder::new(File::open(als_path)?);
    let mut reader = Reader::from_reader(BufReader::new(gz));
    let mut buf = Vec::new();
    let mut skip_buf = Vec::new();
    let mut stack: Vec<String> = Vec::new();

    loop {
        match reader.read_event_into(&mut buf)? {
            Event::Start(e) => {
                let tag = String::from_utf8_lossy(e.name().as_ref()).into_owned();
                if SKIP_SUBTREES.contains(&tag.as_str()) {
                    // Consume the whole subtree without processing it.
                    let end = e.to_end().into_owned();
                    skip_buf.clear();
                    reader.read_to_end_into(end.name(), &mut skip_buf)?;
                } else {
                    let parent = stack.len().checked_sub(1).map(|i| stack[i].as_str());
                    let gparent = stack.len().checked_sub(2).map(|i| stack[i].as_str());
                    st.element(&tag, &e, parent, gparent);
                    stack.push(tag);
                }
            }
            Event::Empty(e) => {
                let tag = String::from_utf8_lossy(e.name().as_ref()).into_owned();
                let parent = stack.len().checked_sub(1).map(|i| stack[i].as_str());
                let gparent = stack.len().checked_sub(2).map(|i| stack[i].as_str());
                st.element(&tag, &e, parent, gparent);
            }
            Event::End(_) => {
                if let Some(tag) = stack.pop() {
                    let parent = stack.last().map(|s| s.as_str());
                    st.close(&tag, parent);
                }
            }
            Event::Eof => break,
            _ => {}
        }
        buf.clear();
    }

    st.snap.tempo = resolve(&st.tempo_cands, "tempo", &mut st.snap.warnings);
    if let Some((n, d)) = resolve(&st.sig_cands, "time_signature", &mut st.snap.warnings) {
        st.snap.time_signature = Some(format!("{n}/{d}"));
    }
    Ok(st.snap)
}
