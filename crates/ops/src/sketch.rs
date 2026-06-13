// sketch.rs
//! Sketch rendering workflow in the operations layer.

use std::path::{Path, PathBuf};
use previews::sketch::parser::parse_sketch_data;
use previews::sketch::engine::{render_sketch, write_wav_file};
use crate::sample_index::SampleIndex;

/// Parse and render a sketch preview for a set file.
pub fn render_sketch_file(
    als_path: &Path,
    out_path: &Path,
    max_seconds: f64,
    library_root: Option<&Path>,
    places: &[PathBuf],
    log: &mut dyn FnMut(String),
) -> Result<(), String> {
    log(format!("Parsing set {}…", als_path.display()));
    let data = parse_sketch_data(als_path)?;
    
    let project_dir = als_path.parent().ok_or_else(|| "No parent directory for set".to_string())?;
    
    // Gather search roots for sample resolution
    let mut roots = Vec::new();
    if let Some(lib_root) = library_root {
        roots.push(lib_root.to_path_buf());
    }
    roots.extend(places.iter().cloned());
    roots.push(project_dir.to_path_buf());
    
    // Build SampleIndex for relinking
    log("Building sample index for relinking…".to_string());
    let index = SampleIndex::build(&roots, log);
    log(format!("Indexed {} samples.", index.len()));
    
    // Closure for sample resolution
    let resolve_sample = |path_abs: &Option<String>, rel_path: &Option<String>| -> Option<PathBuf> {
        // 1. Absolute path
        if let Some(ref p) = path_abs {
            let pb = PathBuf::from(p);
            if pb.exists() {
                return Some(pb);
            }
        }
        
        // 2. Relative path
        if let Some(ref r) = rel_path {
            let pb = project_dir.join(r);
            if pb.exists() {
                return Some(pb);
            }
        }
        
        // 3. Basename lookup in search index
        let filename = rel_path.as_ref()
            .and_then(|r| Path::new(r).file_name())
            .or_else(|| path_abs.as_ref().and_then(|p| Path::new(p).file_name()))
            .map(|n| n.to_string_lossy().into_owned());
            
        if let Some(ref fname) = filename {
            let hits = index.find(fname);
            if let Some(h) = hits.first() {
                return Some(h.clone());
            }
        }
        
        // 4. Project dir walk
        if let Some(ref fname) = filename {
            for entry in walkdir::WalkDir::new(project_dir)
                .max_depth(3)
                .into_iter()
                .filter_map(|e| e.ok())
            {
                if entry.file_name().to_string_lossy() == *fname {
                    return Some(entry.path().to_path_buf());
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
