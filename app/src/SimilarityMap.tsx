import { useEffect, useMemo, useRef, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import ForceGraph3D from "react-force-graph-3d";

// Mirrors ops::similarity::{GraphNode, GraphEdge, GraphData}
type GNode = {
  id: number;
  name: string;
  tempo: number | null;
  artist: string;
  cluster: number;
  has_preview: boolean;
  n_samples: number;
  n_devices: number;
};
type GEdge = { source: number; target: number };
type Graph = { nodes: GNode[]; edges: GEdge[] };

type ColorMode = "cluster" | "tempo" | "artist" | "preview";
const MODES: ColorMode[] = ["cluster", "tempo", "artist", "preview"];

export default function SimilarityMap({
  visible,
  onOpen,
  onPlay,
  onClose,
}: {
  visible: boolean;
  onOpen: (id: number) => void;
  onPlay: (id: number, title: string) => void;
  onClose: () => void;
}) {
  const [graph, setGraph] = useState<Graph | null>(null);
  const [err, setErr] = useState<string | null>(null);
  const [mode, setMode] = useState<ColorMode>("cluster");
  const [sel, setSel] = useState<GNode | null>(null);
  const [size, setSize] = useState({ w: window.innerWidth, h: window.innerHeight - 46 });
  const [hover, setHover] = useState<GNode | null>(null); // nearest-to-cursor node
  const [showLinks, setShowLinks] = useState(false); // links are costly; off by default
  const fgRef = useRef<any>(null);
  const wrapRef = useRef<HTMLDivElement | null>(null);
  const mouseRef = useRef<{ x: number; y: number } | null>(null);
  const downRef = useRef<{ x: number; y: number } | null>(null);
  const draggedRef = useRef(false);
  const rafRef = useRef<number | null>(null);

  // Fetch + graph ONCE; the component stays mounted (hidden) between opens, so
  // it never re-fetches or re-simulates. Use ↻ Reload to recompute on demand.
  const load = () => {
    setSel(null);
    setErr(null);
    setGraph(null);
    invoke<Graph>("similarity_graph").then(setGraph).catch((e) => setErr(String(e)));
  };
  useEffect(() => {
    load();
  }, []);

  useEffect(() => {
    const onResize = () => setSize({ w: window.innerWidth, h: window.innerHeight - 46 });
    window.addEventListener("resize", onResize);
    return () => window.removeEventListener("resize", onResize);
  }, []);

  // The component stays mounted between opens, but react-force-graph keeps its
  // WebGL render loop running even while hidden — which drags the whole app
  // down. Pause the animation when not visible, resume when shown.
  useEffect(() => {
    const fg = fgRef.current;
    if (!fg) return;
    if (visible) fg.resumeAnimation?.();
    else fg.pauseAnimation?.();
  }, [visible]);

  const clusterColor = (c: number) => `hsl(${(c * 137.508) % 360},65%,60%)`;
  const artistHue = (a: string) => {
    let h = 0;
    for (let i = 0; i < a.length; i++) h = (h * 31 + a.charCodeAt(i)) % 360;
    return h;
  };
  const nodeColor = (n: GNode) => {
    if (mode === "cluster") return clusterColor(n.cluster);
    if (mode === "tempo") {
      if (n.tempo == null) return "#666";
      const t = Math.max(0, Math.min(1, (n.tempo - 70) / 90));
      return `hsl(${240 - t * 240},70%,58%)`;
    }
    if (mode === "artist") return n.artist ? `hsl(${artistHue(n.artist)},55%,58%)` : "#666";
    return n.has_preview ? "#36d07a" : "#caa23a"; // preview status
  };

  // Snap-to-nearest: project every node to screen and pick the closest to the
  // cursor within a generous radius, so you don't have to land on the tiny dot.
  const pickNearest = () => {
    rafRef.current = null;
    const fg = fgRef.current;
    const m = mouseRef.current;
    if (!fg || !m || !graph || typeof fg.graph2ScreenCoords !== "function") return;
    let bestNode: GNode | null = null;
    let bd = 34 * 34; // px radius²
    for (const n of graph.nodes as any[]) {
      if (n.x == null) continue; // not laid out yet
      const sc = fg.graph2ScreenCoords(n.x, n.y, n.z ?? 0);
      const dx = sc.x - m.x;
      const dy = sc.y - m.y;
      const d = dx * dx + dy * dy;
      if (d < bd) {
        bd = d;
        bestNode = n as GNode;
      }
    }
    setHover((prev) => (prev?.id === bestNode?.id ? prev : bestNode));
  };
  const onMove = (e: React.MouseEvent) => {
    const r = wrapRef.current?.getBoundingClientRect();
    if (!r) return;
    mouseRef.current = { x: e.clientX - r.left, y: e.clientY - r.top };
    if (downRef.current) {
      // Button held = orbiting. Skip the (expensive, all-nodes) nearest
      // projection entirely so rotation stays smooth.
      const dx = e.clientX - downRef.current.x;
      const dy = e.clientY - downRef.current.y;
      if (dx * dx + dy * dy > 16) draggedRef.current = true;
      return;
    }
    if (rafRef.current == null) rafRef.current = requestAnimationFrame(pickNearest);
  };
  const onDown = (e: React.MouseEvent) => {
    downRef.current = { x: e.clientX, y: e.clientY };
    draggedRef.current = false;
  };
  const onUp = () => {
    if (!draggedRef.current && hover) setSel(hover); // a click (not a drag) selects nearest
    downRef.current = null;
  };

  // Stable reference across re-renders (hover etc.) so react-force-graph never
  // re-ingests the data and restarts the 3D simulation. Only changes on reload.
  const data = useMemo(
    () => (graph ? { nodes: graph.nodes, links: graph.edges } : { nodes: [], links: [] }),
    [graph]
  );

  return (
    <div className="map-overlay" style={{ display: visible ? "flex" : "none" }}>
      <div className="map-toolbar">
        <strong>Similarity Map</strong>
        <span style={{ color: "#8b93a2" }}>
          {graph ? `${graph.nodes.length} sets · ${graph.edges.length} links` : err ? "error" : "loading…"}
        </span>
        <span style={{ color: "#8b93a2", marginLeft: 8 }}>color:</span>
        {MODES.map((m) => (
          <button key={m} className={mode === m ? "on" : ""} onClick={() => setMode(m)}>
            {m}
          </button>
        ))}
        <button
          className={showLinks ? "on" : ""}
          onClick={() => setShowLinks((v) => !v)}
          title="Show/hide similarity links (off is faster)"
        >
          links
        </button>
        <span
          title={sel?.name}
          style={{
            flex: "0 0 240px",
            width: 240,
            marginLeft: 10,
            color: "#fff",
            overflow: "hidden",
            textOverflow: "ellipsis",
            whiteSpace: "nowrap",
          }}
        >
          {sel ? sel.name : ""}
        </span>
        <div style={{ flex: 1 }} />
        <button onClick={load} title="Recompute from the current catalog">↻ Reload</button>
        <button onClick={onClose}>✕ Close</button>
      </div>
      {err && <div className="map-error">{err}</div>}
      <div
        ref={wrapRef}
        style={{ flex: 1, position: "relative" }}
        onMouseMove={onMove}
        onMouseDown={onDown}
        onMouseUp={onUp}
        onMouseLeave={() => setHover(null)}
      >
        <ForceGraph3D
          ref={fgRef}
          width={size.w}
          height={size.h}
          graphData={data as any}
          nodeId="id"
          nodeColor={(n: any) => (n.id === hover?.id ? "#ffffff" : nodeColor(n))}
          nodeVal={(n: any) => (n.id === hover?.id ? 6 : 1)}
          nodeRelSize={4}
          nodeOpacity={0.92}
          nodeResolution={6}
          linkVisibility={() => showLinks}
          linkColor={() => "rgba(140,160,200,0.14)"}
          linkWidth={0.4}
          backgroundColor="#0d0f13"
          warmupTicks={20}
          cooldownTicks={80}
          enablePointerInteraction={false}
          enableNodeDrag={false}
        />
      </div>
      {sel && (
        <div className="map-info">
          <div className="map-info-name">{sel.name}</div>
          <div className="map-info-meta">
            {sel.tempo ? `${sel.tempo.toFixed(1)} bpm` : "no tempo"} · {sel.artist || "no artist"}
            <br />
            cluster {sel.cluster} · {sel.n_samples} samples · {sel.n_devices} devices
            <br />
            {sel.has_preview ? (
              <span style={{ color: "#36d07a" }}>real preview</span>
            ) : (
              <span style={{ color: "#caa23a" }}>no preview · sketch on play</span>
            )}
          </div>
          <div className="map-info-actions">
            <button onClick={() => onPlay(sel.id, sel.name)}>▶ Play</button>
            <button
              onClick={() => {
                onOpen(sel.id);
                onClose();
              }}
            >
              Open detail
            </button>
          </div>
        </div>
      )}
    </div>
  );
}
