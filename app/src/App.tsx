import { useCallback, useEffect, useState } from "react";
import { convertFileSrc, invoke } from "@tauri-apps/api/core";
import PlayerBar, { PlayerTrack } from "./PlayerBar";

type SearchHit = {
  set_id: number;
  project: string;
  als_path: string;
  tempo: number | null;
  time_signature: string | null;
  live_version: string | null;
  has_preview: boolean;
  preview_duration: number | null;
};

type Stats = {
  projects: number;
  sets: number;
  tracks: number;
  devices: number;
  samples: number;
  backups: number;
  previews: number;
};

type PreviewInfo = {
  audio_path: string;
  duration: number | null;
  peaks: number[];
  confidence: number;
  source: string;
};

type Detail = {
  set_id: number;
  project: string;
  als_path: string;
  live_version: string | null;
  tempo: number | null;
  time_signature: string | null;
  warnings: string[] | null;
  tracks: { idx: number; kind: string; name: string | null; color: number | null }[];
  devices: { track: string | null; kind: string; name: string | null; manufacturer: string | null }[];
  samples: { path: string; in_project: number; exists_on_disk: number }[];
  locators: { name: string | null; time: number | null }[];
};

const fileName = (p: string) => p.split("/").pop() ?? p;

export default function App() {
  const [text, setText] = useState("");
  const [minBpm, setMinBpm] = useState("");
  const [maxBpm, setMaxBpm] = useState("");
  const [plugin, setPlugin] = useState("");
  const [hits, setHits] = useState<SearchHit[]>([]);
  const [stats, setStats] = useState<Stats | null>(null);
  const [detail, setDetail] = useState<Detail | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [track, setTrack] = useState<PlayerTrack | null>(null);

  const runSearch = useCallback(async () => {
    try {
      setError(null);
      const res = await invoke<SearchHit[]>("search", {
        text: text || null,
        min_bpm: minBpm ? parseFloat(minBpm) : null,
        max_bpm: maxBpm ? parseFloat(maxBpm) : null,
        plugin: plugin || null,
      });
      setHits(res);
    } catch (e) {
      setError(String(e));
    }
  }, [text, minBpm, maxBpm, plugin]);

  // Debounced live search.
  useEffect(() => {
    const t = setTimeout(runSearch, 250);
    return () => clearTimeout(t);
  }, [runSearch]);

  useEffect(() => {
    invoke<Stats>("stats").then(setStats).catch((e) => setError(String(e)));
  }, []);

  const openDetail = async (id: number) => {
    try {
      setDetail(await invoke<Detail>("inspect", { set_id: id }));
    } catch (e) {
      setError(String(e));
    }
  };

  const openInLive = async (id: number, reveal = false) => {
    try {
      setError(null);
      await invoke("open_set", { set_id: id, reveal });
    } catch (e) {
      setError(String(e));
    }
  };

  const playPreview = async (h: SearchHit) => {
    try {
      setError(null);
      const p = await invoke<PreviewInfo | null>("preview", { set_id: h.set_id });
      if (!p) {
        setError("No preview for this set yet — run `ableton-scan previews <folders>`.");
        return;
      }
      setTrack({
        setId: h.set_id,
        title: fileName(h.als_path).replace(/\.als$/, ""),
        subtitle: `${h.project} · ${p.source}${p.confidence < 0.85 ? ` (${Math.round(p.confidence * 100)}% match)` : ""}`,
        src: convertFileSrc(p.audio_path),
        peaks: p.peaks,
        duration: p.duration,
      });
    } catch (e) {
      setError(String(e));
    }
  };

  return (
    <div className="app">
      <header>
        <h1>Ableton Library</h1>
        {stats && (
          <span className="stats">
            {stats.projects} projects · {stats.sets} sets · {stats.devices} devices ·{" "}
            {stats.backups} backups
          </span>
        )}
      </header>

      <div className="filters">
        <input
          className="grow"
          placeholder="Search projects, sets, tracks, devices, samples…"
          value={text}
          onChange={(e) => setText(e.target.value)}
        />
        <input
          className="bpm"
          placeholder="min bpm"
          value={minBpm}
          onChange={(e) => setMinBpm(e.target.value)}
        />
        <input
          className="bpm"
          placeholder="max bpm"
          value={maxBpm}
          onChange={(e) => setMaxBpm(e.target.value)}
        />
        <input
          className="plugin"
          placeholder="plugin…"
          value={plugin}
          onChange={(e) => setPlugin(e.target.value)}
        />
      </div>

      {error && <div className="error">{error}</div>}

      <div className="main">
        <div className="results">
          {hits.length === 0 && !error && (
            <div className="empty">
              <p>No sets match.</p>
              <p className="hint">
                The catalog only contains what you've indexed so far — add more with{" "}
                <code>ableton-scan scan &lt;folder&gt;</code>.
              </p>
            </div>
          )}
          <table>
            <tbody>
              {hits.map((h) => (
                <tr
                  key={h.set_id}
                  className={detail?.set_id === h.set_id ? "selected" : ""}
                  onClick={() => openDetail(h.set_id)}
                >
                  <td className="proj">{h.project}</td>
                  <td className="set">{fileName(h.als_path)}</td>
                  <td className="num">{h.tempo ?? "?"} bpm</td>
                  <td className="num">{h.time_signature ?? "?"}</td>
                  <td className="ver">{h.live_version?.replace("Ableton Live ", "") ?? ""}</td>
                  <td className="act">
                    {h.has_preview && (
                      <button
                        className="play-btn"
                        title="Play preview"
                        onClick={(e) => {
                          e.stopPropagation();
                          playPreview(h);
                        }}
                      >
                        ▶
                      </button>
                    )}
                    <button
                      className="open-btn"
                      title="Open in Ableton Live"
                      onClick={(e) => {
                        e.stopPropagation();
                        openInLive(h.set_id);
                      }}
                    >
                      Open
                    </button>
                  </td>
                </tr>
              ))}
            </tbody>
          </table>
        </div>

        {detail && (
          <aside className="detail">
            <div className="detail-head">
              <h2>{fileName(detail.als_path).replace(/\.als$/, "")}</h2>
              <button onClick={() => setDetail(null)}>×</button>
            </div>
            <div className="detail-actions">
              <button className="open-btn" onClick={() => openInLive(detail.set_id)}>
                Open in Live
              </button>
              <button className="open-btn ghost" onClick={() => openInLive(detail.set_id, true)}>
                Reveal in Finder
              </button>
            </div>
            <p className="meta">
              {detail.project} · {detail.tempo ?? "?"} bpm · {detail.time_signature ?? "?"} ·{" "}
              {detail.live_version ?? "unknown version"}
            </p>
            {detail.warnings && detail.warnings.length > 0 && (
              <p className="warn">⚠ {detail.warnings.join("; ")}</p>
            )}

            <h3>Tracks ({detail.tracks.length})</h3>
            <ul>
              {detail.tracks.map((t) => (
                <li key={t.idx}>
                  <span className={`chip ${t.kind}`}>{t.kind}</span> {t.name ?? "(unnamed)"}
                </li>
              ))}
            </ul>

            <h3>Devices ({detail.devices.length})</h3>
            <ul>
              {detail.devices.map((d, i) => (
                <li key={i}>
                  <span className={`chip ${d.kind}`}>{d.kind}</span> {d.name}
                  {d.manufacturer && d.manufacturer !== "Ableton" && (
                    <span className="manu"> — {d.manufacturer}</span>
                  )}
                </li>
              ))}
            </ul>

            <h3>Samples ({detail.samples.length})</h3>
            <ul>
              {detail.samples.map((s, i) => (
                <li key={i} title={s.path}>
                  {s.exists_on_disk ? "" : "⚠ "}
                  {fileName(s.path)}
                  {s.in_project ? <span className="manu"> (in project)</span> : null}
                </li>
              ))}
            </ul>

            {detail.locators.length > 0 && (
              <>
                <h3>Locators ({detail.locators.length})</h3>
                <ul>
                  {detail.locators.map((l, i) => (
                    <li key={i}>
                      {l.name ?? "(unnamed)"} @ beat {l.time ?? "?"}
                    </li>
                  ))}
                </ul>
              </>
            )}
          </aside>
        )}
      </div>

      {track && <PlayerBar track={track} onClose={() => setTrack(null)} />}
    </div>
  );
}
