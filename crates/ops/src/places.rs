use std::path::{Path, PathBuf};
use quick_xml::Reader;
use quick_xml::events::Event;

/// Extract user-pinned "Places" from Ableton Live's preferences.
/// Looks for Library.cfg in ~/Library/Preferences/Ableton/Live x.x.x/
pub fn get_ableton_places() -> Vec<PathBuf> {
    let mut places = Vec::new();
    
    let home = match dirs::home_dir() {
        Some(h) => h,
        None => return places,
    };
    
    let pref_root = home.join("Library/Preferences/Ableton");
    if !pref_root.exists() {
        return places;
    }
    
    // Scan all Live version folders
    let entries = match std::fs::read_dir(&pref_root) {
        Ok(e) => e,
        Err(_) => return places,
    };
    
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() && path.file_name().map_or(false, |n| n.to_string_lossy().starts_with("Live ")) {
            let lib_cfg = path.join("Library.cfg");
            if lib_cfg.exists() {
                if let Ok(p) = parse_library_cfg(&lib_cfg) {
                    places.extend(p);
                }
            }
        }
    }
    
    // Dedup and filter non-existent paths
    places.sort();
    places.dedup();
    places.retain(|p| p.exists());
    
    places
}

fn parse_library_cfg(path: &Path) -> Result<Vec<PathBuf>, Box<dyn std::error::Error>> {
    let mut places = Vec::new();
    let mut reader = Reader::from_file(path)?;
    let mut buf = Vec::new();
    
    loop {
        match reader.read_event_into(&mut buf)? {
            Event::Start(e) | Event::Empty(e) => {
                let tag = e.name();
                if tag.as_ref() == b"UserFolderInfo" || tag.as_ref() == b"ProjectPath" {
                    for attr in e.attributes().flatten() {
                        if attr.key.as_ref() == b"Path" || attr.key.as_ref() == b"Value" {
                            let val = attr.unescape_value()?;
                            if !val.is_empty() {
                                places.push(PathBuf::from(val.into_owned()));
                            }
                        }
                    }
                }
            }
            Event::Eof => break,
            _ => {}
        }
        buf.clear();
    }
    
    Ok(places)
}
