//! Set similarity graph — blend, kNN, and clustering.
//!
//! Faithful Rust port of `tools/similarity_map.py` (the validated reference
//! oracle; see ai/SIMILARITY_GRAPH_DESIGN.md). Produces nodes + edges; the 3D
//! layout is done client-side by react-force-graph, so there is NO layout here.
//!
//! Signals (metadata blend): shared samples (Jaccard), shared devices (Jaccard),
//! tempo (half/double-aware gaussian), artist/project prior (strong bond), and
//! name/vocabulary (TF-IDF cosine). NOT YET wired (reserved weights): MIDI key
//! and audio "sounds-alike" (from REAL bounces only — never sketches).

use std::collections::{HashMap, HashSet};

use indexer::GraphSet;
use serde::Serialize;

// absolute weights (so adding key/audio later stays consistent)
const W_SAMPLE: f32 = 0.20;
const W_ARTIST: f32 = 0.20;
const W_TEMPO: f32 = 0.10;
const W_NAME: f32 = 0.07;
const W_DEVICE: f32 = 0.03;

const TEMPO_SIGMA: f64 = 8.0;
const NAME_DF_CAP: usize = 60; // skip name tokens in > this many sets (too common)
const LINK_CAP: usize = 400; // skip pathologically common keys (e.g. a stock kick)
const TEMPO_WINDOW: usize = 12;

const STOP: &[&str] = &[
    "the", "and", "for", "set", "als", "ableton", "project", "live", "copy", "final", "new",
    "untitled", "version", "wav", "mix", "master",
];

#[derive(Debug, Clone, Serialize)]
pub struct GraphNode {
    pub id: i64,
    pub name: String,
    pub tempo: Option<f64>,
    pub artist: String,
    pub cluster: u32,
    pub has_preview: bool,
    pub n_samples: usize,
    pub n_devices: usize,
}

#[derive(Debug, Clone, Serialize)]
pub struct GraphEdge {
    pub source: i64,
    pub target: i64,
}

#[derive(Debug, Clone, Serialize)]
pub struct GraphData {
    pub nodes: Vec<GraphNode>,
    pub edges: Vec<GraphEdge>,
}

fn tokenize(text: &str) -> HashSet<String> {
    let mut out = HashSet::new();
    let mut cur = String::new();
    let mut flush = |cur: &mut String, out: &mut HashSet<String>| {
        if cur.len() >= 2 && !STOP.contains(&cur.as_str()) {
            out.insert(cur.clone());
        }
        cur.clear();
    };
    for ch in text.chars() {
        if ch.is_ascii_alphanumeric() {
            cur.push(ch.to_ascii_lowercase());
        } else {
            flush(&mut cur, &mut out);
        }
    }
    flush(&mut cur, &mut out);
    out
}

fn jaccard(a: &HashSet<String>, b: &HashSet<String>) -> f32 {
    if a.is_empty() || b.is_empty() {
        return 0.0;
    }
    let (small, big) = if a.len() <= b.len() { (a, b) } else { (b, a) };
    let inter = small.iter().filter(|x| big.contains(*x)).count();
    if inter == 0 {
        return 0.0;
    }
    let union = a.len() + b.len() - inter;
    inter as f32 / union as f32
}

fn tempo_kernel(a: Option<f64>, b: Option<f64>) -> f32 {
    let (a, b) = match (a, b) {
        (Some(a), Some(b)) => (a, b),
        _ => return 0.0,
    };
    let mut best = 0.0f64;
    for r in [1.0, 2.0, 0.5] {
        let d = a - b * r;
        best = best.max((-(d * d) / (2.0 * TEMPO_SIGMA * TEMPO_SIGMA)).exp());
    }
    best as f32
}

fn artist_prior(a: &GraphSet, b: &GraphSet) -> f32 {
    if !a.artist.is_empty() && a.artist == b.artist {
        1.0
    } else if a.project_id == b.project_id {
        0.5
    } else {
        0.0
    }
}

fn name_cos(i: usize, j: usize, vecs: &[HashMap<String, f32>], norms: &[f32]) -> f32 {
    let (a, b) = (&vecs[i], &vecs[j]);
    let (small, big) = if a.len() <= b.len() { (a, b) } else { (b, a) };
    let mut dot = 0.0f32;
    for (t, w) in small {
        if let Some(w2) = big.get(t) {
            dot += w * w2;
        }
    }
    dot / (norms[i] * norms[j])
}

/// Build the similarity graph: kNN edges + label-propagation clusters.
pub fn build_graph(sets: &[GraphSet], k: usize) -> GraphData {
    let n = sets.len();
    let sample_sets: Vec<HashSet<String>> =
        sets.iter().map(|s| s.samples.iter().cloned().collect()).collect();
    let device_sets: Vec<HashSet<String>> =
        sets.iter().map(|s| s.devices.iter().cloned().collect()).collect();
    let token_sets: Vec<HashSet<String>> = sets.iter().map(|s| tokenize(&s.text)).collect();

    // --- TF-IDF over name/track tokens ---
    let mut df: HashMap<&str, usize> = HashMap::new();
    for toks in &token_sets {
        for t in toks {
            *df.entry(t.as_str()).or_insert(0) += 1;
        }
    }
    let nf = n as f32;
    let mut vecs: Vec<HashMap<String, f32>> = Vec::with_capacity(n);
    let mut norms: Vec<f32> = Vec::with_capacity(n);
    for toks in &token_sets {
        let mut v = HashMap::new();
        for t in toks {
            let d = *df.get(t.as_str()).unwrap_or(&1);
            if d <= NAME_DF_CAP {
                let idf = ((1.0 + nf) / (1.0 + d as f32)).ln() + 1.0;
                v.insert(t.clone(), idf);
            }
        }
        let norm = v.values().map(|w| w * w).sum::<f32>().sqrt().max(1e-6);
        vecs.push(v);
        norms.push(norm);
    }

    // --- candidate generation (inverted index = the speedup) ---
    let mut by_sample: HashMap<&str, Vec<usize>> = HashMap::new();
    let mut by_device: HashMap<&str, Vec<usize>> = HashMap::new();
    let mut by_token: HashMap<&str, Vec<usize>> = HashMap::new();
    let mut by_artist: HashMap<&str, Vec<usize>> = HashMap::new();
    let mut by_project: HashMap<i64, Vec<usize>> = HashMap::new();
    for (i, s) in sets.iter().enumerate() {
        for x in &sample_sets[i] {
            by_sample.entry(x.as_str()).or_default().push(i);
        }
        for x in &device_sets[i] {
            by_device.entry(x.as_str()).or_default().push(i);
        }
        for t in vecs[i].keys() {
            by_token.entry(t.as_str()).or_default().push(i);
        }
        if !s.artist.is_empty() {
            by_artist.entry(s.artist.as_str()).or_default().push(i);
        }
        by_project.entry(s.project_id).or_default().push(i);
    }

    let mut cand: Vec<HashSet<usize>> = vec![HashSet::new(); n];
    fn link(group: &[usize], cand: &mut [HashSet<usize>]) {
        if group.len() > LINK_CAP {
            return;
        }
        for &a in group {
            for &b in group {
                if a != b {
                    cand[a].insert(b);
                }
            }
        }
    }
    for g in by_sample.values() {
        link(g, &mut cand);
    }
    for g in by_device.values() {
        link(g, &mut cand);
    }
    for g in by_token.values() {
        link(g, &mut cand);
    }
    for g in by_artist.values() {
        link(g, &mut cand);
    }
    for g in by_project.values() {
        link(g, &mut cand);
    }
    // tempo-window neighbours so isolated sets still get linked
    let mut order: Vec<usize> = (0..n).collect();
    order.sort_by(|&a, &b| {
        let ta = sets[a].tempo.unwrap_or(f64::INFINITY);
        let tb = sets[b].tempo.unwrap_or(f64::INFINITY);
        ta.partial_cmp(&tb).unwrap_or(std::cmp::Ordering::Equal)
    });
    for pos in 0..n {
        let i = order[pos];
        for d in 1..=TEMPO_WINDOW {
            if pos >= d {
                cand[i].insert(order[pos - d]);
            }
            if pos + d < n {
                cand[i].insert(order[pos + d]);
            }
        }
    }

    // --- score candidates -> kNN ---
    let score = |i: usize, j: usize| -> f32 {
        W_SAMPLE * jaccard(&sample_sets[i], &sample_sets[j])
            + W_DEVICE * jaccard(&device_sets[i], &device_sets[j])
            + W_TEMPO * tempo_kernel(sets[i].tempo, sets[j].tempo)
            + W_ARTIST * artist_prior(&sets[i], &sets[j])
            + W_NAME * name_cos(i, j, &vecs, &norms)
    };
    let mut knn: Vec<Vec<(usize, f32)>> = Vec::with_capacity(n);
    for i in 0..n {
        let mut scored: Vec<(usize, f32)> =
            cand[i].iter().map(|&j| (j, score(i, j))).filter(|&(_, w)| w > 0.0).collect();
        scored.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));
        scored.truncate(k);
        knn.push(scored);
    }

    // --- clusters: weighted label propagation over the kNN graph ---
    let mut labels: Vec<usize> = (0..n).collect();
    for _ in 0..25 {
        let mut changed = 0;
        for i in 0..n {
            if knn[i].is_empty() {
                continue;
            }
            let mut tally: HashMap<usize, f32> = HashMap::new();
            for &(j, w) in &knn[i] {
                *tally.entry(labels[j]).or_insert(0.0) += w;
            }
            // max weight, tie-break to the smaller label (deterministic)
            let best = tally
                .iter()
                .max_by(|a, b| {
                    a.1.partial_cmp(b.1)
                        .unwrap_or(std::cmp::Ordering::Equal)
                        .then(b.0.cmp(a.0))
                })
                .map(|(l, _)| *l)
                .unwrap();
            if labels[i] != best {
                labels[i] = best;
                changed += 1;
            }
        }
        if changed == 0 {
            break;
        }
    }
    // remap labels to 0..K
    let mut uniq: Vec<usize> = labels.iter().cloned().collect::<HashSet<_>>().into_iter().collect();
    uniq.sort_unstable();
    let remap: HashMap<usize, u32> =
        uniq.iter().enumerate().map(|(i, &l)| (l, i as u32)).collect();

    // --- assemble nodes + undirected unique edges ---
    let nodes: Vec<GraphNode> = sets
        .iter()
        .enumerate()
        .map(|(i, s)| GraphNode {
            id: s.id,
            name: s.name.clone(),
            tempo: s.tempo,
            artist: s.artist.clone(),
            cluster: remap[&labels[i]],
            has_preview: s.has_preview,
            n_samples: sample_sets[i].len(),
            n_devices: device_sets[i].len(),
        })
        .collect();

    let mut seen: HashSet<(usize, usize)> = HashSet::new();
    let mut edges: Vec<GraphEdge> = Vec::new();
    for i in 0..n {
        for &(j, _w) in &knn[i] {
            let key = if i < j { (i, j) } else { (j, i) };
            if seen.insert(key) {
                edges.push(GraphEdge {
                    source: sets[key.0].id,
                    target: sets[key.1].id,
                });
            }
        }
    }

    GraphData { nodes, edges }
}
