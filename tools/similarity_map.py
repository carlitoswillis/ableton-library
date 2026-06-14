#!/usr/bin/env python3
"""Set Similarity Map (PROTOTYPE) — an alternative "map" view of the catalog.

Reads the ableton-library catalog DB, places every set in 2D so that similar
sets sit close together, clusters + colorizes them, and writes a SELF-CONTAINED
interactive HTML you can open in a browser (pan/zoom/hover/click/search, with
color-by toggles).

This is the reference oracle for the eventual Rust (`ops::similarity`) port — the
similarity blend, kNN-via-inverted-index, layout, and clustering here are the
source of truth. See ai/SIMILARITY_GRAPH_DESIGN.md.

SIGNALS USED IN THIS PROTOTYPE (metadata blend, all from the DB — fast, no .als
parsing): shared samples (Jaccard), shared devices/plugins (Jaccard), tempo
(half/double-time-aware gaussian), artist/project prior (strong bond), and
name/vocabulary (TF-IDF cosine). NOT YET here (added in the real impl): MIDI
**key** (needs an .als note parse) and **audio sounds-alike** (needs decoding
REAL bounces — never sketches). Their weights are reserved below so the blend
stays consistent when they land.

Usage:
    python3 tools/similarity_map.py [--db PATH] [-o out.html] [-k 10]
Default --db: ./library.db, else <app data>/ableton-library/library.db.
"""
import argparse
import gzip
import math
import os
import re
import sqlite3
import sys
from collections import defaultdict

import numpy as np

# ---- similarity weights (absolute, so adding key/audio later is consistent) --
# Reserved (not computed in this prototype): "audio": 0.30, "key": 0.10.
W = {
    "sample": 0.20,
    "artist": 0.20,
    "tempo":  0.10,
    "name":   0.07,
    "device": 0.03,
}
TEMPO_SIGMA = 8.0   # BPM
NAME_DF_CAP = 60    # skip name tokens appearing in > this many sets (too common)
TOKEN_RE = re.compile(r"[a-z0-9]+")
STOP = {"the", "and", "for", "set", "als", "ableton", "project", "live", "copy",
        "final", "new", "untitled", "version", "wav", "mix", "master"}


# --------------------------------------------------------------- load the DB
def find_db(arg):
    if arg:
        return arg
    here = os.path.join(os.path.dirname(os.path.dirname(os.path.abspath(__file__))), "library.db")
    if os.path.exists(here):
        return here
    if os.path.exists("library.db"):
        return "library.db"
    # app data dir fallback (matches the CLI default)
    base = os.environ.get("XDG_DATA_HOME") or os.path.expanduser("~/Library/Application Support")
    cand = os.path.join(base, "ableton-library", "library.db")
    return cand


def basename_lower(p):
    return os.path.basename(p.replace("\\", "/")).lower()


def load(db_path):
    con = sqlite3.connect(db_path)
    con.row_factory = sqlite3.Row
    cur = con.cursor()

    sets = {}
    for r in cur.execute(
        "SELECT s.id, s.als_path, s.tempo, s.project_id, s.artist_override, "
        "p.artist AS p_artist, p.name AS p_name "
        "FROM sets s JOIN projects p ON p.id = s.project_id"
    ):
        artist = (r["artist_override"] or r["p_artist"] or "").strip()
        name = os.path.splitext(basename_lower(r["als_path"]))[0]
        sets[r["id"]] = {
            "id": r["id"], "path": r["als_path"], "tempo": r["tempo"],
            "project": r["project_id"], "artist": artist, "name": name,
            "samples": set(), "devices": set(), "tokens": set(),
            "has_preview": False, "n_tracks": 0,
        }

    for r in cur.execute("SELECT set_id, path FROM samples"):
        s = sets.get(r["set_id"])
        if s is not None and r["path"]:
            s["samples"].add(basename_lower(r["path"]))

    for r in cur.execute("SELECT set_id, manufacturer, name FROM devices"):
        s = sets.get(r["set_id"])
        if s is not None:
            key = f"{(r['manufacturer'] or '').lower()}:{(r['name'] or '').lower()}"
            if key != ":":
                s["devices"].add(key)

    for r in cur.execute("SELECT set_id, name FROM tracks"):
        s = sets.get(r["set_id"])
        if s is not None:
            s["n_tracks"] += 1
            if r["name"]:
                for t in TOKEN_RE.findall(r["name"].lower()):
                    if len(t) >= 2 and t not in STOP:
                        s["tokens"].add(t)

    # set-name tokens too
    for s in sets.values():
        for t in TOKEN_RE.findall(s["name"]):
            if len(t) >= 2 and t not in STOP:
                s["tokens"].add(t)

    try:
        for r in cur.execute(
            "SELECT DISTINCT set_id FROM previews "
            "WHERE set_id IS NOT NULL AND source <> 'sketch' AND audio_path <> ''"
        ):
            s = sets.get(r["set_id"])
            if s is not None:
                s["has_preview"] = True
    except sqlite3.OperationalError:
        pass  # previews table may not exist on an old DB

    con.close()
    return list(sets.values())


# ----------------------------------------------------------- TF-IDF for names
def build_tfidf(items):
    df = defaultdict(int)
    for s in items:
        for t in s["tokens"]:
            df[t] += 1
    n = len(items)
    idf = {t: math.log((1 + n) / (1 + d)) + 1.0 for t, d in df.items()}
    vecs, norms = [], []
    for s in items:
        v = {t: idf[t] for t in s["tokens"] if df[t] <= NAME_DF_CAP}
        nrm = math.sqrt(sum(w * w for w in v.values())) or 1.0
        vecs.append(v)
        norms.append(nrm)
    return vecs, norms, df


def name_cos(i, j, vecs, norms):
    a, b = vecs[i], vecs[j]
    if len(a) > len(b):
        a, b = b, a
    dot = sum(w * b.get(t, 0.0) for t, w in a.items())
    return dot / (norms[i] * norms[j])


# --------------------------------------------------------------- similarity
def jaccard(a, b):
    if not a or not b:
        return 0.0
    inter = len(a & b)
    if inter == 0:
        return 0.0
    return inter / len(a | b)


def tempo_kernel(a, b):
    if a is None or b is None:
        return 0.0
    best = 0.0
    for r in (1.0, 2.0, 0.5):
        d = a - b * r
        best = max(best, math.exp(-(d * d) / (2 * TEMPO_SIGMA * TEMPO_SIGMA)))
    return best


def artist_prior(si, sj):
    if si["artist"] and si["artist"] == sj["artist"]:
        return 1.0
    if si["project"] == sj["project"]:
        return 0.5
    return 0.0


def score(i, j, items, vecs, norms):
    si, sj = items[i], items[j]
    return (
        W["sample"] * jaccard(si["samples"], sj["samples"])
        + W["device"] * jaccard(si["devices"], sj["devices"])
        + W["tempo"] * tempo_kernel(si["tempo"], sj["tempo"])
        + W["artist"] * artist_prior(si, sj)
        + W["name"] * name_cos(i, j, vecs, norms)
    )


# ---------------------------- candidate generation (inverted index = the speedup)
def candidates(items, vecs):
    n = len(items)
    by_sample = defaultdict(list)
    by_device = defaultdict(list)
    by_artist = defaultdict(list)
    by_project = defaultdict(list)
    by_token = defaultdict(list)
    for i, s in enumerate(items):
        for x in s["samples"]:
            by_sample[x].append(i)
        for x in s["devices"]:
            by_device[x].append(i)
        if s["artist"]:
            by_artist[s["artist"]].append(i)
        by_project[s["project"]].append(i)
        for t in vecs[i]:
            by_token[t].append(i)

    # tempo-window neighbours so isolated sets still get linked
    order = sorted(range(n), key=lambda i: (items[i]["tempo"] is None, items[i]["tempo"] or 0.0))
    tempo_win = 12

    cand = [set() for _ in range(n)]

    def link(group):
        if len(group) > 400:   # skip pathologically common keys (e.g. a stock kick)
            return
        for a in group:
            for b in group:
                if a != b:
                    cand[a].add(b)

    for g in by_sample.values():
        link(g)
    for g in by_device.values():
        link(g)
    for g in by_artist.values():
        link(g)
    for g in by_project.values():
        link(g)
    for t, g in by_token.items():
        link(g)
    for pos, i in enumerate(order):
        for d in range(1, tempo_win + 1):
            for k in (pos - d, pos + d):
                if 0 <= k < n:
                    cand[i].add(order[k])
    return cand


def build_knn(items, vecs, norms, k):
    cand = candidates(items, vecs)
    n = len(items)
    knn = [[] for _ in range(n)]
    for i in range(n):
        scored = [(score(i, j, items, vecs, norms), j) for j in cand[i]]
        scored.sort(reverse=True)
        knn[i] = [(j, w) for (w, j) in scored[:k] if w > 0.0]
    return knn


# --------------------------------------------------------------- layout (FR)
def layout_fr(n, edges, iters=240, seed=42):
    rng = np.random.default_rng(seed)
    # start clustered near the origin so weakly-linked sets don't get flung out;
    # repulsion then spreads everything apart over the iterations.
    pos = (rng.standard_normal((n, 2)) * 0.01).astype(np.float32)
    if n <= 1:
        return pos
    k = 2.2 * math.sqrt(1.0 / n)   # larger ideal distance => more spread
    e = np.array(edges, dtype=np.int64) if edges else np.zeros((0, 2), np.int64)
    temp = 0.13
    for _ in range(iters):
        diff = pos[:, None, :] - pos[None, :, :]          # n,n,2
        dist2 = (diff * diff).sum(-1) + 1e-4
        inv = (k * k) / dist2
        disp = (diff * inv[:, :, None]).sum(1)             # repulsion
        if len(e):
            d = pos[e[:, 0]] - pos[e[:, 1]]
            dl = np.sqrt((d * d).sum(-1, keepdims=True)) + 1e-6
            f = (dl / k) * d                                # attraction (dl^2/k)
            np.add.at(disp, e[:, 0], -f)
            np.add.at(disp, e[:, 1], f)
        dlen = np.sqrt((disp * disp).sum(-1, keepdims=True)) + 1e-9
        pos += (disp / dlen) * np.minimum(dlen, temp)
        temp *= 0.987                                       # slow cool
    # center on the median and scale by a percentile radius so a handful of
    # outliers don't shrink the whole map into a dot.
    pos -= np.median(pos, axis=0)
    r = float(np.percentile(np.sqrt((pos * pos).sum(-1)), 96)) + 1e-6
    pos /= r
    return pos


# ----------------------------------------------------- clusters (label prop)
def cluster(n, knn, iters=25):
    labels = list(range(n))
    nbrs = [[(j, w) for (j, w) in knn[i]] for i in range(n)]
    for _ in range(iters):
        changed = 0
        for i in range(n):
            if not nbrs[i]:
                continue
            cnt = defaultdict(float)
            for j, w in nbrs[i]:
                cnt[labels[j]] += w
            best = max(cnt.items(), key=lambda kv: (kv[1], -kv[0]))[0]
            if labels[i] != best:
                labels[i] = best
                changed += 1
        if changed == 0:
            break
    remap = {l: i for i, l in enumerate(sorted(set(labels)))}
    return [remap[l] for l in labels]


# --------------------------------------------------------------- HTML output
def emit_html(items, pos, labels, knn, out_path):
    nodes = []
    for i, s in enumerate(items):
        nodes.append({
            "x": round(float(pos[i, 0]), 4), "y": round(float(pos[i, 1]), 4),
            "c": int(labels[i]), "t": s["tempo"], "a": s["artist"],
            "p": 1 if s["has_preview"] else 0, "n": s["name"],
            "ns": len(s["samples"]), "nt": s["n_tracks"],
        })
    seen = set()
    edges = []
    for i in range(len(items)):
        for j, _w in knn[i]:
            key = (i, j) if i < j else (j, i)
            if key not in seen:
                seen.add(key)
                edges.append([key[0], key[1]])
    import json
    payload = json.dumps({"nodes": nodes, "edges": edges}, separators=(",", ":"))
    html = _HTML_TEMPLATE.replace("/*DATA*/", payload)
    with open(out_path, "w") as f:
        f.write(html)


_HTML_TEMPLATE = r"""<!doctype html><html><head><meta charset="utf-8">
<title>Set Similarity Map</title>
<style>
 html,body{margin:0;height:100%;background:#0d0f13;color:#cdd3dc;font:13px -apple-system,system-ui,sans-serif;overflow:hidden}
 #c{display:block;cursor:grab} #c:active{cursor:grabbing}
 #ui{position:fixed;top:10px;left:10px;display:flex;gap:6px;flex-wrap:wrap;align-items:center;z-index:5}
 #ui button{background:#1a1e26;color:#cdd3dc;border:1px solid #2a3140;border-radius:6px;padding:5px 9px;cursor:pointer}
 #ui button.on{background:#2d6cdf;border-color:#2d6cdf;color:#fff}
 #ui input{background:#1a1e26;color:#cdd3dc;border:1px solid #2a3140;border-radius:6px;padding:5px 9px;width:160px}
 #tip{position:fixed;pointer-events:none;background:#11151c;border:1px solid #2a3140;border-radius:7px;padding:7px 9px;max-width:280px;display:none;z-index:6;box-shadow:0 6px 20px #0009}
 #tip b{color:#fff} #tip .m{color:#8b93a2}
 #info{position:fixed;right:10px;top:10px;width:240px;background:#11151c;border:1px solid #2a3140;border-radius:8px;padding:10px;display:none;z-index:6}
 #legend{position:fixed;bottom:10px;left:10px;color:#8b93a2;z-index:5}
 label{color:#8b93a2;margin-left:6px}
</style></head><body>
<canvas id="c"></canvas>
<div id="ui">
 <span style="color:#8b93a2">color:</span>
 <button data-m="cluster" class="on">cluster</button>
 <button data-m="tempo">tempo</button>
 <button data-m="artist">artist</button>
 <button data-m="preview">preview</button>
 <input id="q" placeholder="search name…">
 <label><input type="checkbox" id="edges"> edges</label>
</div>
<div id="tip"></div><div id="info"></div>
<div id="legend">drag = pan · wheel = zoom · hover = peek · click = pin</div>
<script>
const DATA=/*DATA*/;
const cv=document.getElementById('c'),ctx=cv.getContext('2d'),tip=document.getElementById('tip'),info=document.getElementById('info');
let W=0,H=0;function size(){W=cv.width=innerWidth*devicePixelRatio;H=cv.height=innerHeight*devicePixelRatio;cv.style.width=innerWidth+'px';cv.style.height=innerHeight+'px';if(!fitted){fit();fitted=true;}draw();}
addEventListener('resize',size);
let mode='cluster',query='',showEdges=false,pinned=-1;
let scale=400,ox=0,oy=0,fitted=false; // world->screen
function fit(){
 const xs=DATA.nodes.map(n=>n.x),ys=DATA.nodes.map(n=>n.y);
 const minx=Math.min(...xs),maxx=Math.max(...xs),miny=Math.min(...ys),maxy=Math.max(...ys);
 const ex=Math.max(maxx-minx,maxy-miny,1e-3);
 scale=0.86*Math.min(W,H)/ex; ox=-((minx+maxx)/2)*scale; oy=-((miny+maxy)/2)*scale;
}
function sx(x){return W/2+ (x*scale)+ox} function sy(y){return H/2+(y*scale)+oy}
function golden(i){return (i*137.508)%360}
function hsl(h,s,l){return `hsl(${h},${s}%,${l}%)`}
let artistHue={};function ahue(a){if(!(a in artistHue)){let h=0;for(let i=0;i<a.length;i++)h=(h*31+a.charCodeAt(i))%360;artistHue[a]=h;}return artistHue[a];}
function color(n,dim){
 let c;
 if(mode==='cluster')c=hsl(golden(n.c),62,dim?22:60);
 else if(mode==='tempo'){if(n.t==null)c=dim?'#333':'#666';else{let h=240-Math.max(0,Math.min(1,(n.t-70)/90))*240;c=hsl(h,70,dim?24:58);}}
 else if(mode==='artist')c=n.a?hsl(ahue(n.a),55,dim?22:58):(dim?'#333':'#666');
 else {c = n.p? (dim?'#1d5e35':'#36d07a') : (dim?'#3a2e10':'#caa23a');} // green=real preview, amber=none
 return c;
}
function draw(){
 ctx.clearRect(0,0,W,H);
 if(showEdges){ctx.strokeStyle='rgba(120,140,180,0.10)';ctx.lineWidth=devicePixelRatio;ctx.beginPath();
  for(const e of DATA.edges){const a=DATA.nodes[e[0]],b=DATA.nodes[e[1]];ctx.moveTo(sx(a.x),sy(a.y));ctx.lineTo(sx(b.x),sy(b.y));}ctx.stroke();}
 const r=Math.max(2.2,scale*0.012);
 for(let i=0;i<DATA.nodes.length;i++){const n=DATA.nodes[i];
  const dim = query && !n.n.includes(query);
  ctx.beginPath();ctx.arc(sx(n.x),sy(n.y),i===pinned?r*2:r,0,7);
  ctx.fillStyle=color(n,dim);ctx.fill();
  if(n.p&&mode!=='preview'&&!dim){ctx.lineWidth=devicePixelRatio*1.3;ctx.strokeStyle='#36d07a';ctx.stroke();}
  if(i===pinned){ctx.lineWidth=devicePixelRatio*2;ctx.strokeStyle='#fff';ctx.stroke();}
 }
 // labels when zoomed in
 if(scale>900){ctx.fillStyle='#9aa3b2';ctx.font=(11*devicePixelRatio)+'px system-ui';
  for(const n of DATA.nodes){if(query&&!n.n.includes(query))continue;ctx.fillText(n.n.slice(0,22),sx(n.x)+r+2,sy(n.y)+3);}}
}
function nearest(mx,my){let best=-1,bd=1e9;const r=Math.max(2.2,scale*0.012)+6*devicePixelRatio;
 for(let i=0;i<DATA.nodes.length;i++){const n=DATA.nodes[i];const dx=sx(n.x)-mx,dy=sy(n.y)-my;const d=dx*dx+dy*dy;if(d<bd){bd=d;best=i;}}
 return bd<=(r*r)?best:-1;}
let drag=false,px=0,py=0,moved=false;
cv.addEventListener('mousedown',e=>{drag=true;moved=false;px=e.clientX;py=e.clientY;});
addEventListener('mouseup',()=>drag=false);
addEventListener('mousemove',e=>{const mx=e.clientX*devicePixelRatio,my=e.clientY*devicePixelRatio;
 if(drag){ox+=(e.clientX-px)*devicePixelRatio;oy+=(e.clientY-py)*devicePixelRatio;px=e.clientX;py=e.clientY;moved=true;draw();tip.style.display='none';return;}
 const i=nearest(mx,my);
 if(i>=0){const n=DATA.nodes[i];tip.style.display='block';tip.style.left=(e.clientX+12)+'px';tip.style.top=(e.clientY+12)+'px';
  tip.innerHTML=`<b>${esc(n.n)}</b><br><span class="m">${n.t?n.t.toFixed(1)+' bpm':'—'} · ${esc(n.a)||'no artist'} · cl ${n.c}</span><br><span class="m">${n.ns} samples · ${n.nt} tracks · ${n.p?'real preview':'no preview'}</span>`;}
 else tip.style.display='none';});
cv.addEventListener('click',e=>{if(moved)return;const i=nearest(e.clientX*devicePixelRatio,e.clientY*devicePixelRatio);pinned=i;
 if(i>=0){const n=DATA.nodes[i];info.style.display='block';info.innerHTML=`<b style="color:#fff">${esc(n.n)}</b><br><br>${n.t?n.t.toFixed(2)+' bpm':'no tempo'}<br>${esc(n.a)||'no artist'}<br>cluster ${n.c}<br>${n.ns} samples · ${n.nt} tracks<br>${n.p?'<span style="color:#36d07a">real preview</span>':'<span style="color:#caa23a">no preview (sketch on play)</span>'}`;}
 else info.style.display='none';draw();});
cv.addEventListener('wheel',e=>{e.preventDefault();const f=Math.exp(-e.deltaY*0.0015);const mx=e.clientX*devicePixelRatio,my=e.clientY*devicePixelRatio;
 ox=mx-(mx-ox)*f-(W/2)*(f-1);oy=my-(my-oy)*f-(H/2)*(f-1);scale*=f;draw();},{passive:false});
function esc(s){return (s||'').replace(/[&<>]/g,c=>({'&':'&amp;','<':'&lt;','>':'&gt;'}[c]));}
document.querySelectorAll('#ui button[data-m]').forEach(b=>b.onclick=()=>{document.querySelectorAll('#ui button[data-m]').forEach(x=>x.classList.remove('on'));b.classList.add('on');mode=b.dataset.m;draw();});
document.getElementById('q').oninput=e=>{query=e.target.value.toLowerCase();draw();};
document.getElementById('edges').onchange=e=>{showEdges=e.target.checked;draw();};
size();
</script></body></html>"""


# --------------------------------------------------------------------- main
def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--db", default=None)
    ap.add_argument("-o", "--out", default=None)
    ap.add_argument("-k", type=int, default=10, help="neighbours per set")
    args = ap.parse_args()

    db = find_db(args.db)
    if not os.path.exists(db):
        sys.exit(f"DB not found: {db}\n(copy library.db into the repo root, or pass --db)")
    out = args.out or os.path.join(os.path.dirname(os.path.abspath(db)), "similarity_map.html")

    print(f"DB: {db}")
    items = load(db)
    print(f"sets: {len(items)}")
    if not items:
        sys.exit("no sets in catalog")

    vecs, norms, df = build_tfidf(items)
    knn = build_knn(items, vecs, norms, args.k)
    edges = [[i, j] for i in range(len(items)) for (j, _w) in knn[i] if i < j]
    print(f"knn edges (undirected-ish): {sum(len(x) for x in knn)} dir / {len(edges)} undir")

    print("laying out…")
    pos = layout_fr(len(items), edges)
    labels = cluster(len(items), knn)
    nclu = len(set(labels))
    sizes = sorted((labels.count(c) for c in set(labels)), reverse=True)
    print(f"clusters: {nclu}  (top sizes: {sizes[:12]})")
    n_iso = sum(1 for x in knn if not x)
    print(f"unlinked/sparse sets: {n_iso}")
    n_prev = sum(1 for s in items if s["has_preview"])
    print(f"sets with a real preview: {n_prev}")

    emit_html(items, pos, labels, knn, out)
    print(f"-> {out}\nOpen it in a browser.")


if __name__ == "__main__":
    main()
