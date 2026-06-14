import { useEffect, useRef, useState } from "react";
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
  const fgRef = useRef<any>(null);

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

  // react-force-graph mutates link source/target into node refs, so hand it a
  // fresh copy each render is unnecessary — it keeps its own internal state.
  const data = graph
    ? { nodes: graph.nodes, links: graph.edges }
    : { nodes: [], links: [] };

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
        <div style={{ flex: 1 }} />
        <button onClick={load} title="Recompute from the current catalog">↻ Reload</button>
        <button onClick={onClose}>✕ Close</button>
      </div>
      {err && <div className="map-error">{err}</div>}
      <ForceGraph3D
        ref={fgRef}
        width={size.w}
        height={size.h}
        graphData={data as any}
        nodeId="id"
        nodeLabel={(n: any) =>
          `${n.name}  ·  ${n.tempo ? n.tempo.toFixed(0) + "bpm" : "—"}  ·  ${n.artist || "no artist"}`
        }
        nodeColor={(n: any) => nodeColor(n)}
        nodeRelSize={4}
        nodeOpacity={0.92}
        linkColor={() => "rgba(140,160,200,0.12)"}
        linkWidth={0.4}
        backgroundColor="#0d0f13"
        warmupTicks={40}
        cooldownTicks={120}
        onNodeClick={(n: any) => setSel(n as GNode)}
        onBackgroundClick={() => setSel(null)}
      />
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
