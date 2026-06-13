use std::collections::HashMap;
use std::fs::File;
use std::io::{BufReader, BufWriter};
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use flate2::read::GzDecoder;
use flate2::write::GzEncoder;
use flate2::Compression;
use quick_xml::events::Event;
use quick_xml::Reader;
use quick_xml::Writer;
use rusqlite::Connection;

/// The proxy set relinking plan.
pub struct RelinkMap {
    /// dead_path -> new_path
    pub files: HashMap<String, PathBuf>,
}

pub fn get_proxy_cache_dir() -> Result<PathBuf> {
    let dir = dirs::cache_dir()
        .context("no cache dir")?
        .join("ableton-library")
        .join("proxies");
    std::fs::create_dir_all(&dir)?;
    Ok(dir)
}

/// Plan the relinking for a set by resolving missing samples via the catalog
/// and Ableton Places.
pub fn plan_relink(
    conn: &Connection,
    set_id: i64,
    places: &[PathBuf],
    log: &mut dyn FnMut(String),
) -> Result<RelinkMap> {
    let (_, samples_raw) = indexer::set_render_inputs(conn, set_id)?;
    let mut map = RelinkMap {
        files: HashMap::new(),
    };

    let als_path = indexer::set_path(conn, set_id)?;
    let als_pb = PathBuf::from(&als_path);
    let current_project_root = als_pb.parent().map(|p| p.to_path_buf());

    // Group missing samples by their (dead) parent dir for folder-move detection.
    struct MissingInfo {
        name: String,
    }
    let mut groups: HashMap<PathBuf, Vec<MissingInfo>> = HashMap::new();
    for (p, _in_project) in &samples_raw {
        if crate::triage::sample_state(p) != crate::triage::SampleState::Missing {
            continue;
        }
        let pb = Path::new(p);
        if let (Some(parent), Some(name)) = (pb.parent(), pb.file_name()) {
            groups
                .entry(parent.to_path_buf())
                .or_default()
                .push(MissingInfo {
                    name: name.to_string_lossy().into_owned(),
                });
        }
    }

    if groups.is_empty() {
        return Ok(map);
    }

    log(format!(
        "planning relink for {} missing sample(s) across {} folder(s)",
        groups.values().map(|v| v.len()).sum::<usize>(),
        groups.len()
    ));

    // One-pass fully-recursive index over Places (+ given roots): replaces
    // the old per-file depth-5 walks and adds Live-style relaxed matching
    // (alt extension, fuzzy stem). Built only when something is missing.
    let index = crate::sample_index::build_search_index(places, log);

    let mtime_secs = |c: &Path| {
        std::fs::metadata(c)
            .and_then(|m| m.modified())
            .ok()
            .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
            .map(|d| d.as_secs())
            .unwrap_or(0)
    };

    for (dead_parent, infos) in groups {
        let basenames: Vec<String> = infos.iter().map(|i| i.name.clone()).collect();
        
        // 0. Local project search (M4b redesign fallback)
        // If the sample was in_project, try to find it at the same relative
        // path from the current .als.
        let mut best_folder = None;
        if let Some(root) = &current_project_root {
            // Heuristic: Ableton usually has Samples/Recorded, Samples/Imported, etc.
            // If the dead_parent contains "Samples", try that suffix.
            let dead_str = dead_parent.to_string_lossy();
            if let Some(idx) = dead_str.find("/Samples") {
                let suffix = &dead_str[idx + 1..]; // "Samples/Recorded"
                let cand = root.join(suffix);
                if basenames.iter().all(|n| cand.join(n).exists()) {
                    best_folder = Some(cand);
                }
            } else {
                // Try the root itself (some people put files right in the project)
                if basenames.iter().all(|n| root.join(n).exists()) {
                    best_folder = Some(root.clone());
                }
            }
        }

        if best_folder.is_none() {
            // 1. Catalog voting (existing logic)
            let mut votes: HashMap<PathBuf, usize> = HashMap::new();
            for name in &basenames {
                for c in indexer::sample_paths_by_basename(conn, name)? {
                    let cp = Path::new(&c);
                    if cp.exists() {
                        if let Some(par) = cp.parent() {
                            if par != dead_parent.as_path() {
                                *votes.entry(par.to_path_buf()).or_default() += 1;
                            }
                        }
                    }
                }
            }
            
            let mut ranked: Vec<(PathBuf, usize)> = votes.into_iter().collect();
            ranked.sort_by_key(|(_, v)| std::cmp::Reverse(*v));
            
            best_folder = ranked
                .iter()
                .map(|(cand, _)| cand)
                .find(|cand| basenames.iter().all(|n| cand.join(n).exists()))
                .cloned();
        }

        if best_folder.is_none() {
            // 2. Index-driven folder search: vote living parents of each
            // basename (full recursion, fuzzy-aware), then verify the winner
            // holds ALL of the group's exact filenames. (Folder rewrites
            // need exact names; fuzzy hits are handled per-file below,
            // which may map to a differently-named file.)
            let mut votes: HashMap<PathBuf, usize> = HashMap::new();
            for name in &basenames {
                for par in index.parents_of(name) {
                    if par != dead_parent {
                        *votes.entry(par).or_default() += 1;
                    }
                }
            }
            let mut ranked: Vec<(PathBuf, usize)> = votes.into_iter().collect();
            ranked.sort_by_key(|(_, v)| std::cmp::Reverse(*v));
            best_folder = ranked
                .iter()
                .map(|(cand, _)| cand)
                .find(|cand| basenames.iter().all(|n| cand.join(n).exists()))
                .cloned();
        }

        if let Some(target) = best_folder {
            log(format!(
                "found moved folder: {} -> {}",
                dead_parent.display(),
                target.display()
            ));
            for name in basenames {
                let dead_path = dead_parent.join(&name).to_string_lossy().into_owned();
                map.files.insert(dead_path, target.join(name));
            }
        } else {
            // Per-file fallback
            for info in infos {
                let name = info.name;
                let mut candidates: Vec<PathBuf> = Vec::new();
                
                // From local project
                if let Some(root) = &current_project_root {
                    let dead_str = dead_parent.to_string_lossy();
                    if let Some(idx) = dead_str.find("/Samples") {
                        let suffix = &dead_str[idx+1..];
                        let cand = root.join(suffix).join(&name);
                        if cand.exists() {
                            candidates.push(cand);
                        }
                    }
                    let cand_root = root.join(&name);
                    if cand_root.exists() {
                        candidates.push(cand_root);
                    }
                }

                // From catalog (exact basename, mtime-ranked)
                let mut catalog: Vec<PathBuf> = indexer::sample_paths_by_basename(conn, &name)?
                    .into_iter()
                    .map(PathBuf::from)
                    .filter(|cp| cp.exists())
                    .collect();
                catalog.sort_by_key(|c| std::cmp::Reverse(mtime_secs(c)));
                candidates.extend(catalog);

                // From the index (Places, full recursion): exact filename
                // first, then alt extension, then fuzzy stem — already
                // tier+mtime ordered. This is what lets us find what Live's
                // browser search finds.
                candidates.extend(index.find(&name));

                // First candidate wins (priority: project-local > catalog
                // exact > index exact > alt-ext > fuzzy).
                if let Some(winner) = candidates.first() {
                    if winner.file_name().map_or(false, |n| n.to_string_lossy() != *name) {
                        log(format!(
                            "fuzzy relink: {name} -> {}",
                            winner.display()
                        ));
                    }
                    let dead_path = dead_parent.join(&name).to_string_lossy().into_owned();
                    map.files.insert(dead_path, winner.clone());
                } else {
                    log(format!("unresolved: {name}"));
                }
            }
        }
    }

    Ok(map)
}

/// Create a transformed copy of the .als file with missing samples relinked.
pub fn create_proxy_set(
    conn: &Connection,
    set_id: i64,
    log: &mut dyn FnMut(String),
) -> Result<PathBuf> {
    let als_path = indexer::set_path(conn, set_id)?;
    let als = Path::new(&als_path);
    let stem = als.file_stem().context("no stem")?.to_string_lossy();
    let proxy_path = get_proxy_cache_dir()?.join(format!("{} (preview proxy).als", stem));

    let places = crate::places::get_ableton_places();
    let relink = plan_relink(conn, set_id, &places, log)?;

    if relink.files.is_empty() {
        log("nothing to relink, set is already healthy".into());
        // We still create a copy if requested, or just return original?
        // Actually, the goal of M4b proper is to also bypass plugins later.
        // For now, let's always write the proxy.
    }

    let gz_in = GzDecoder::new(File::open(als)?);
    let mut reader = Reader::from_reader(BufReader::new(gz_in));
    
    let gz_out = GzEncoder::new(File::create(&proxy_path)?, Compression::default());
    let mut writer = Writer::new(BufWriter::new(gz_out));
    
    let mut buf = Vec::new();
    let mut stack: Vec<String> = Vec::new();
    
    let mut in_file_ref = false;

    loop {
        match reader.read_event_into(&mut buf)? {
            Event::Start(e) => {
                let tag = String::from_utf8_lossy(e.name().as_ref()).into_owned();
                if tag == "FileRef" {
                    in_file_ref = true;
                }
                
                let mut e_new = e.clone();
                if in_file_ref && tag == "Path" {
                    if let Some(val) = e.try_get_attribute("Value").ok().flatten() {
                        let path_str = val.unescape_value()?.into_owned();
                        if let Some(new_path) = relink.files.get(&path_str) {
                            e_new.clear_attributes();
                            e_new.push_attribute(("Value", new_path.to_string_lossy().as_ref()));
                        }
                    }
                } else if in_file_ref && tag == "RelativePathType" {
                    // Always force absolute (0) in the proxy, because its 
                    // location in the cache breaks all relative paths.
                    e_new.clear_attributes();
                    e_new.push_attribute(("Value", "0"));
                } else if in_file_ref && tag == "RelativePath" {
                    // Clear relative path to force fallback to the absolute Path.
                    e_new.clear_attributes();
                    e_new.push_attribute(("Value", ""));
                }

                writer.write_event(Event::Start(e_new))?;
                stack.push(tag);
            }
            Event::Empty(e) => {
                let tag = String::from_utf8_lossy(e.name().as_ref()).into_owned();
                let mut e_new = e.clone();
                if tag == "FileRef" {
                    // Self-closing FileRef? Unlikely but handle it.
                    in_file_ref = false; 
                }
                
                if in_file_ref && tag == "Path" {
                    if let Some(val) = e.try_get_attribute("Value").ok().flatten() {
                        let path_str = val.unescape_value()?.into_owned();
                        if let Some(new_path) = relink.files.get(&path_str) {
                            e_new.clear_attributes();
                            e_new.push_attribute(("Value", new_path.to_string_lossy().as_ref()));
                        }
                    }
                } else if in_file_ref && tag == "RelativePathType" {
                    e_new.clear_attributes();
                    e_new.push_attribute(("Value", "0"));
                } else if in_file_ref && tag == "RelativePath" {
                    e_new.clear_attributes();
                    e_new.push_attribute(("Value", ""));
                }
                writer.write_event(Event::Empty(e_new))?;
            }
            Event::End(e) => {
                if let Some(tag) = stack.pop() {
                    if tag == "FileRef" {
                        in_file_ref = false;
                    }
                }
                writer.write_event(Event::End(e))?;
            }
            Event::Eof => break,
            e => {
                writer.write_event(e)?;
            }
        }
        buf.clear();
    }
    
    Ok(proxy_path)
}
