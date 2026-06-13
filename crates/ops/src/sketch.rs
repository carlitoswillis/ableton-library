// sketch.rs
//! Sketch rendering workflow in the operations layer.

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use previews::sketch::parser::parse_sketch_data;
use previews::sketch::engine::{render_sketch, write_wav_file};
use indexer;
use crate::sample_index::build_search_index;

/// Parse and render a sketch preview for a set file.
pub fn render_sketch_file(
    db_path: &Path,
    als_path: &Path,
    out_path: &Path,
    max_seconds: f64,
    lib_root: Option<&Path>,
    log: &mut dyn FnMut(String),
) -> Result<(), String> {
    log(format!("Parsing set {}…", als_path.display()));
    let data = parse_sketch_data(als_path)?;
    
    let conn = indexer::open(db_path).map_err(|e| e.to_string())?;
    
    let project_dir = als_path.parent().ok_or_else(|| "No parent directory for set".to_string())?;
    
    // Build SampleIndex for this run (Places + project/lib root)
    let mut extra_roots = Vec::new();
    if let Some(r) = lib_root {
        extra_roots.push(r.to_path_buf());
    }
    let sample_idx = build_search_index(&extra_roots, log);

    // Closure for sample resolution
    let conn = Arc::new(Mutex::new(conn));
    let resolve_sample = |path_abs: &Option<String>, rel_path: &Option<String>| -> Option<PathBuf> {
        // 1. Absolute path check
        if let Some(ref p) = path_abs {
            let pb = PathBuf::from(p);
            if pb.exists() {
                return Some(pb);
            }
        }

        // 2. Project-relative check
        if let Some(ref r) = rel_path {
            let pb = project_dir.join(r);
            if pb.exists() {
                return Some(pb);
            }
        }

        // 3. SampleIndex lookup (Places + extra roots)
        let filename = rel_path.as_ref()
            .and_then(|r| Path::new(r).file_name())
            .or_else(|| path_abs.as_ref().and_then(|p| Path::new(p).file_name()))
            .map(|n| n.to_string_lossy().into_owned());

        if let Some(ref fname) = filename {
            let hits = sample_idx.find(fname);
            if let Some(h) = hits.first() {
                return Some(h.clone());
            }

            // 4. Catalog-wide basename lookup (where else is this file used?)
            let conn = conn.lock().unwrap();
            if let Ok(hits) = indexer::sample_paths_by_basename(&conn, fname) {
                for h in hits {
                    let pb = PathBuf::from(h);
                    if pb.exists() {
                        return Some(pb);
                    }
                }
            }
        }

        None
    };

    
    log("Rendering sketch preview…".to_string());
    let samples = render_sketch(&data, project_dir, max_seconds, 44100, resolve_sample)?;
    
    log(format!("Writing output WAV file to {}…", out_path.display()));
    write_wav_file(out_path, &samples, 44100)?;
    
    log("Sketch rendering completed successfully.".to_string());
    Ok(())
}
