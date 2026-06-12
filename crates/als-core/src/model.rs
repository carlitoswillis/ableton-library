//! Data model: the JSON shape of a parsed Live Set.
//!
//! Must stay field-for-field compatible with tools/reference_extract.py,
//! which is the executable spec / test oracle for this crate.

use serde::Serialize;

/// One parsed `.als` file.
#[derive(Debug, Clone, Serialize)]
pub struct SetSnapshot {
    pub als_path: String,
    pub file_size: u64,
    /// ISO-8601 UTC, e.g. "2026-05-29T20:33:14+00:00"
    pub mtime: String,
    /// SHA-256 of the .als file bytes (gzipped form).
    pub content_hash: String,
    /// e.g. "Ableton Live 11.3.43" (root `Creator` attribute).
    pub live_version: Option<String>,
    /// e.g. "11.0_11300" (root `MinorVersion` attribute).
    pub schema_version: Option<String>,
    pub tempo: Option<f64>,
    pub tempos: Vec<f64>,
    /// e.g. "4/4"
    pub time_signature: Option<String>,
    pub tracks: Vec<Track>,
    pub devices: Vec<Device>,
    pub samples: Vec<SampleRef>,
    pub locators: Vec<Locator>,
    /// Lenient-extraction notes ("tempo not found", ...). Never a hard failure.
    pub warnings: Vec<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct Track {
    pub kind: TrackKind,
    pub name: Option<String>,
    pub color: Option<i32>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum TrackKind {
    Midi,
    Audio,
    Return,
    Group,
}

#[derive(Debug, Clone, Serialize)]
pub struct Device {
    /// Track index, "master", or null when context is unknown.
    pub track: Option<TrackRef>,
    pub kind: DeviceKind,
    /// Plugin display name, or the XML element name for native devices.
    pub name: Option<String>,
    pub manufacturer: Option<String>,
}

/// Serializes as a bare number (track index) or the string "master".
#[derive(Debug, Clone, Serialize)]
#[serde(untagged)]
pub enum TrackRef {
    Index(usize),
    Master(String),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum DeviceKind {
    Native,
    Au,
    Vst,
    Vst3,
}

#[derive(Debug, Clone, Serialize)]
pub struct SampleRef {
    /// Absolute path as stored by Live (may not exist on this machine).
    pub path: String,
    /// Path contains the project folder name (sample lives inside the project).
    pub in_project: bool,
    pub exists: bool,
}

#[derive(Debug, Clone, Serialize)]
pub struct Locator {
    pub name: Option<String>,
    /// Position in beats.
    pub time: Option<f64>,
}

/// A project folder: one or more sets plus Backup/ lineage.
#[derive(Debug, Clone, Serialize)]
pub struct ProjectSnapshot {
    pub folder_path: String,
    pub name: String,
    pub sets: Vec<SetSnapshot>,
    pub backups: Vec<BackupEntry>,
}

/// Lineage-only record of a Backup/*.als — never parsed (see PROJECT_STATE.md).
#[derive(Debug, Clone, Serialize)]
pub struct BackupEntry {
    pub file: String,
    pub size: u64,
    pub mtime: String,
}
