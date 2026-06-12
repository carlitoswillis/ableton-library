import { useCallback, useEffect, useRef, useState } from "react";
import { convertFileSrc, invoke } from "@tauri-apps/api/core";
import { open as openDialog } from "@tauri-apps/plugin-dialog";
import { listen } from "@tauri-apps/api/event";
import PlayerBar, { PlayerTrack } from "./PlayerBar";

type ScanSummary = {
  indexed: number;
  unchanged: number;
  errors: number;
  pruned: number;
  harvested: number;
};

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

type ExportJob = {
  id: number;
  set_id: number;
  als_path: string;
  project_name: string;
  status: "pending" | "processing" | "completed" | "failed";
  error: string | null;
  created_at: string;
  started_at: string | null;
  completed_at: string | null;
};

type Suggestion = {
  set_id: number;
  set_name: string;
  project_name: string;
  audio_path: string;
  file_name: string;
  confidence: number;
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
  has_preview?: boolean;
  preview_path?: string | null;
  preview_missing?: boolean;
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
  const [sortBy, setSortBy] = useState("modified");
  const [dateModified, setDateModified] = useState("");
  const [dateScanned, setDateScanned] = useState("");
  const [hasPreviewFilter, setHasPreviewFilter] = useState("all");
  const [scanning, setScanning] = useState(false);
  const [scanMsg, setScanMsg] = useState<string | null>(null);
  const [scanLogs, setScanLogs] = useState<string[]>([]);
  const [showProgressModal, setShowProgressModal] = useState(false);
  const [liveStats, setLiveStats] = useState({
    indexed: 0,
    unchanged: 0,
    previews: 0,
    errors: 0,
  });
  const logConsoleRef = useRef<HTMLDivElement | null>(null);

  const [queue, setQueue] = useState<ExportJob[]>([]);
  const [queueActive, setQueueActive] = useState(false);
  const [showQueueModal, setShowQueueModal] = useState(false);
  const [showWatchModal, setShowWatchModal] = useState(false);
  const [watchFolders, setWatchFolders] = useState<[number, string][]>([]);
  const [suggestions, setSuggestions] = useState<Suggestion[]>([]);
  const [loadingSuggestions, setLoadingSuggestions] = useState(false);


  const runSearch = useCallback(async () => {
    try {
      setError(null);
      const res = await invoke<SearchHit[]>("search", {
        text: text || null,
        min_bpm: minBpm ? parseFloat(minBpm) : null,
        max_bpm: maxBpm ? parseFloat(maxBpm) : null,
        plugin: plugin || null,
        sort_by: sortBy || null,
        date_modified: dateModified || null,
        date_scanned: dateScanned || null,
        has_preview: hasPreviewFilter || null,
      });
      setHits(res);
    } catch (e) {
      setError(String(e));
    }
  }, [text, minBpm, maxBpm, plugin, sortBy, dateModified, dateScanned, hasPreviewFilter]);

  // Debounced live search.
  useEffect(() => {
    const t = setTimeout(runSearch, 250);
    return () => clearTimeout(t);
  }, [runSearch]);

  const refreshStats = useCallback(() => {
    invoke<Stats>("stats").then(setStats).catch((e) => setError(String(e)));
  }, []);

  const refreshQueue = useCallback(async () => {
    try {
      const q = await invoke<ExportJob[]>("get_export_queue");
      setQueue(q);
      const active = await invoke<boolean>("get_export_queue_active");
      setQueueActive(active);
    } catch (e) {
      setError(String(e));
    }
  }, []);

  const refreshWatchFolders = useCallback(async () => {
    try {
      const res = await invoke<[number, string][]>("list_watch_folders");
      setWatchFolders(res);
    } catch (e) {
      setError(String(e));
    }
  }, []);

  const refreshSuggestions = useCallback(async () => {
    setLoadingSuggestions(true);
    try {
      const res = await invoke<Suggestion[]>("get_watch_suggestions");
      setSuggestions(res);
    } catch (e) {
      setError(String(e));
    } finally {
      setLoadingSuggestions(false);
    }
  }, []);


  const addToQueue = async (setId: number) => {
    try {
      setError(null);
      await invoke("add_to_export_queue", { set_id: setId });
      refreshQueue();
      refreshStats();
    } catch (e) {
      setError(String(e));
    }
  };

  const removeFromQueue = async (jobId: number) => {
    try {
      setError(null);
      await invoke("remove_from_export_queue", { job_id: jobId });
      refreshQueue();
      refreshStats();
    } catch (e) {
      setError(String(e));
    }
  };

  const clearCompleted = async () => {
    try {
      setError(null);
      await invoke("clear_completed_jobs");
      refreshQueue();
      refreshStats();
    } catch (e) {
      setError(String(e));
    }
  };

  const retryFailed = async () => {
    try {
      setError(null);
      await invoke("retry_failed_jobs");
      refreshQueue();
      refreshStats();
    } catch (e) {
      setError(String(e));
    }
  };

  const toggleQueue = async (active: boolean) => {
    try {
      setError(null);
      await invoke("toggle_export_queue", { active });
      setQueueActive(active);
    } catch (e) {
      setError(String(e));
    }
  };

  useEffect(() => {
    refreshStats();
    refreshQueue();
    refreshWatchFolders();
    refreshSuggestions();
  }, [refreshStats, refreshQueue, refreshWatchFolders, refreshSuggestions]);

  useEffect(() => {
    let active = true;
    let unsubscribed = false;
    let unlistenFn: (() => void) | null = null;

    listen<string>("scan-progress", (event) => {
      if (!active) return;
      const line = event.payload;
      setScanLogs((prev) => [...prev, line]);
      if (line.startsWith("indexed")) {
        setLiveStats((prev) => ({ ...prev, indexed: prev.indexed + 1 }));
      } else if (line.startsWith("ERROR")) {
        setLiveStats((prev) => ({ ...prev, errors: prev.errors + 1 }));
      } else if (line.startsWith("preview")) {
        setLiveStats((prev) => ({ ...prev, previews: prev.previews + 1 }));
      }
    }).then((unsub) => {
      unlistenFn = unsub;
      if (unsubscribed) {
        unsub();
      }
    });

    return () => {
      active = false;
      unsubscribed = true;
      if (unlistenFn) {
        unlistenFn();
      }
    };
  }, []);

  useEffect(() => {
    let active = true;
    let unsubscribed = false;
    let unlistenFn: (() => void) | null = null;

    listen<void>("export-queue-updated", () => {
      if (!active) return;
      refreshQueue();
      refreshStats();
    }).then((unsub) => {
      unlistenFn = unsub;
      if (unsubscribed) {
        unsub();
      }
    });

    return () => {
      active = false;
      unsubscribed = true;
      if (unlistenFn) {
        unlistenFn();
      }
    };
  }, [refreshQueue, refreshStats]);

  useEffect(() => {
    if (logConsoleRef.current) {
      logConsoleRef.current.scrollTop = logConsoleRef.current.scrollHeight;
    }
  }, [scanLogs]);

  const cancelScan = async () => {
    try {
      await invoke("cancel_scan");
    } catch (e) {
      setError(String(e));
    }
  };

  const pickAndScan = async () => {
    try {
      const dir = await openDialog({
        directory: true,
        multiple: false,
        title: "Choose a folder of Ableton projects",
      });
      if (!dir) return;
      setScanLogs([]);
      setLiveStats({ indexed: 0, unchanged: 0, previews: 0, errors: 0 });
      setShowProgressModal(true);
      setScanning(true);
      setError(null);
      setScanMsg(null);
      const s = await invoke<ScanSummary>("scan_folder", { root: dir });
      setLiveStats({
        indexed: s.indexed,
        unchanged: s.unchanged,
        previews: s.harvested,
        errors: s.errors,
      });
      setScanMsg(
        `${s.indexed} indexed, ${s.unchanged} unchanged, ${s.harvested} preview(s) harvested` +
          (s.errors ? `, ${s.errors} errors` : ""),
      );
      refreshStats();
      runSearch();
    } catch (e) {
      const msg = String(e);
      if (msg.includes("cancelled")) {
        setScanMsg("Scan cancelled");
      } else {
        setError(msg);
      }
      setShowProgressModal(false);
    } finally {
      setScanning(false);
    }
  };

  const addWatchFolder = async () => {
    try {
      const folder = await openDialog({
        directory: true,
        multiple: false,
        title: "Select Watch Folder",
      });
      if (folder) {
        await invoke("add_watch_folder", { path: folder as string });
        refreshWatchFolders();
        refreshSuggestions();
      }
    } catch (e) {
      setError(String(e));
    }
  };

  const removeWatchFolder = async (id: number) => {
    try {
      await invoke("remove_watch_folder", { id });
      refreshWatchFolders();
      refreshSuggestions();
    } catch (e) {
      setError(String(e));
    }
  };

  const linkSuggestion = async (setId: number, audioPath: string) => {
    try {
      await invoke("link_watch_suggestion", { set_id: setId, audio_path: audioPath });
      refreshSuggestions();
      runSearch();
      refreshStats();
    } catch (e) {
      setError(String(e));
    }
  };

  const ignoreSuggestion = async (setId: number, audioPath: string) => {
    try {
      await invoke("ignore_watch_suggestion", { set_id: setId, audio_path: audioPath });
      refreshSuggestions();
    } catch (e) {
      setError(String(e));
    }
  };

  const playSuggestionTrack = (s: Suggestion) => {
    setTrack({
      setId: s.set_id,
      title: s.set_name.replace(/\.als$/, ""),
      subtitle: `${s.project_name} · Bounce Match (${Math.round(s.confidence * 100)}% match)`,
      src: convertFileSrc(s.audio_path),
      peaks: [],
      duration: null,
    });
  };


  const openDetail = async (id: number) => {
    try {
      const d = await invoke<Detail>("inspect", { set_id: id });
      setDetail(d);
      if (d.preview_missing) {
        setError("Note: The preview file was missing from disk and has been removed from the database.");
        runSearch();
        refreshStats();
      }
    } catch (e) {
      setError(String(e));
    }
  };

  const removePreview = async (setId: number) => {
    try {
      setError(null);
      const deleted = await invoke<boolean>("remove_preview", { set_id: setId });
      if (deleted) {
        setTrack((prev) => (prev && prev.setId === setId ? null : prev));
        openDetail(setId);
        runSearch();
        refreshStats();
        setError("Note: Audio preview has been removed.");
      }
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

  const renderRowActions = (hit: SearchHit) => {
    const job = queue.find((j) => j.set_id === hit.set_id);

    if (hit.has_preview) {
      return (
        <div style={{ display: "flex", gap: "6px", alignItems: "center", justifyContent: "flex-end" }}>
          <button
            className="play-btn"
            title="Play preview"
            onClick={(e) => {
              e.stopPropagation();
              playPreview(hit);
            }}
          >
            ▶ Play
          </button>
          <button
            className="play-btn"
            style={{ borderColor: "var(--border)", color: "var(--dim)", fontSize: "11px", padding: "3px 6px" }}
            onClick={(e) => {
              e.stopPropagation();
              addToQueue(hit.set_id);
            }}
            title="Re-render/update audio preview"
          >
            Update ↻
          </button>
          <button
            className="open-btn"
            title="Open in Ableton Live"
            onClick={(e) => {
              e.stopPropagation();
              openInLive(hit.set_id);
            }}
          >
            Open
          </button>
        </div>
      );
    } else {
      if (job) {
        if (job.status === "processing") {
          return (
            <div style={{ display: "flex", gap: "6px", alignItems: "center", justifyContent: "flex-end" }}>
              <button
                className="play-btn"
                style={{ borderColor: "var(--accent)", color: "var(--accent)" }}
                onClick={(e) => {
                  e.stopPropagation();
                  removeFromQueue(job.id);
                }}
                title="Rendering in background. Click to cancel."
              >
                ⚙️ Rendering ×
              </button>
              <button
                className="open-btn"
                title="Open in Ableton Live"
                onClick={(e) => {
                  e.stopPropagation();
                  openInLive(hit.set_id);
                }}
              >
                Open
              </button>
            </div>
          );
        } else if (job.status === "pending") {
          return (
            <div style={{ display: "flex", gap: "6px", alignItems: "center", justifyContent: "flex-end" }}>
              <button
                className="play-btn"
                style={{ borderColor: "var(--dim)", color: "var(--dim)" }}
                onClick={(e) => {
                  e.stopPropagation();
                  removeFromQueue(job.id);
                }}
                title="Queued (Pending). Click to remove."
              >
                ⏳ Queued ×
              </button>
              <button
                className="open-btn"
                title="Open in Ableton Live"
                onClick={(e) => {
                  e.stopPropagation();
                  openInLive(hit.set_id);
                }}
              >
                Open
              </button>
            </div>
          );
        } else {
          return (
            <div style={{ display: "flex", gap: "6px", alignItems: "center", justifyContent: "flex-end" }}>
              <button
                className="play-btn"
                style={{ borderColor: "#ff8f8f", color: "#ff8f8f" }}
                onClick={(e) => {
                  e.stopPropagation();
                  addToQueue(hit.set_id);
                }}
                title={`Failed: ${job.error}. Click to retry.`}
              >
                ❌ Retry ↻
              </button>
              <button
                className="open-btn"
                title="Open in Ableton Live"
                onClick={(e) => {
                  e.stopPropagation();
                  openInLive(hit.set_id);
                }}
              >
                Open
              </button>
            </div>
          );
        }
      } else {
        return (
          <div style={{ display: "flex", gap: "6px", alignItems: "center", justifyContent: "flex-end" }}>
            <button
              className="play-btn"
              style={{ borderColor: "var(--border)", color: "var(--accent)" }}
              onClick={(e) => {
                e.stopPropagation();
                addToQueue(hit.set_id);
              }}
              title="Queue audio preview render"
            >
              Queue Render
            </button>
            <button
              className="open-btn"
              title="Open in Ableton Live"
              onClick={(e) => {
                e.stopPropagation();
                openInLive(hit.set_id);
              }}
            >
              Open
            </button>
          </div>
        );
      }
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
      runSearch();
      refreshStats();
    }
  };

  return (
    <div className="app">
      <header>
        <h1>Ableton Library</h1>
        {stats && (
          <span className="stats">
            {stats.projects} projects · {stats.sets} sets · {stats.previews} previews ·{" "}
            {stats.backups} backups
          </span>
        )}
        {scanMsg && <span className="scan-msg">{scanMsg}</span>}
        <button className="scan-btn" onClick={pickAndScan} disabled={scanning}>
          {scanning ? "Scanning…" : "Scan folder…"}
        </button>
        <button
          className="scan-btn"
          style={{ marginLeft: "10px" }}
          onClick={() => setShowQueueModal(true)}
        >
          {queue.some(j => j.status === 'processing') ? "Rendering…" : "Render Queue"} ({queue.filter(j => j.status === 'pending' || j.status === 'processing').length})
        </button>
        <button
          className="scan-btn"
          style={{ marginLeft: "10px" }}
          onClick={() => {
            setShowWatchModal(true);
            refreshWatchFolders();
            refreshSuggestions();
          }}
        >
          Watch Folders {suggestions.length > 0 && `(${suggestions.length})`}
        </button>

      </header>

      {scanning && !showProgressModal && (
        <div className="scan-banner">
          <div className="scan-banner-status">
            <span className="pulse-dot" />
            <strong>Scanning in background...</strong>
          </div>
          <span className="scan-banner-stats">
            {liveStats.indexed} indexed · {liveStats.previews} preview(s) harvested
            {liveStats.errors > 0 ? ` · ${liveStats.errors} error(s)` : ""}
          </span>
          <div className="scan-banner-actions">
            <button className="banner-btn" onClick={() => setShowProgressModal(true)}>
              View Logs
            </button>
            <button className="banner-btn danger" onClick={cancelScan}>
              Cancel Scan
            </button>
          </div>
        </div>
      )}

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
        <select
          className="sort-select"
          value={sortBy}
          onChange={(e) => setSortBy(e.target.value)}
        >
          <option value="modified">Recently Edited</option>
          <option value="name">Alphabetical (A-Z)</option>
          <option value="bpm">Tempo (BPM)</option>
          <option value="previews">With Previews First</option>
        </select>
        <select
          className="sort-select"
          value={dateModified}
          onChange={(e) => setDateModified(e.target.value)}
          style={{ width: "135px" }}
        >
          <option value="">Any Time Created</option>
          <option value="today">Created Today</option>
          <option value="yesterday">Created Yesterday</option>
          <option value="week">Created This Week</option>
          <option value="month">Created This Month</option>
        </select>
        <select
          className="sort-select"
          value={dateScanned}
          onChange={(e) => setDateScanned(e.target.value)}
          style={{ width: "135px" }}
        >
          <option value="">Any Time Scanned</option>
          <option value="today">Scanned Today</option>
          <option value="yesterday">Scanned Yesterday</option>
          <option value="week">Scanned This Week</option>
          <option value="month">Scanned This Month</option>
        </select>
        <select
          className="sort-select"
          value={hasPreviewFilter}
          onChange={(e) => setHasPreviewFilter(e.target.value)}
          style={{ width: "135px" }}
        >
          <option value="all">All Previews</option>
          <option value="yes">Has Preview</option>
          <option value="no">No Preview</option>
        </select>
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
                    {renderRowActions(h)}
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
              {(() => {
                const existingJob = queue.find((j) => j.set_id === detail.set_id);
                if (existingJob) {
                  if (existingJob.status === "processing") {
                    return (
                      <button className="open-btn warning" disabled>
                        Rendering...
                      </button>
                    );
                  } else if (existingJob.status === "pending") {
                    return (
                      <button
                        className="open-btn ghost"
                        onClick={() => removeFromQueue(existingJob.id)}
                        title="Click to remove from queue"
                      >
                        Queued (Pending) ×
                      </button>
                    );
                  } else if (existingJob.status === "failed") {
                    return (
                      <button
                        className="open-btn danger-btn"
                        onClick={() => addToQueue(detail.set_id)}
                        title={`Failed: ${existingJob.error}. Click to retry.`}
                      >
                        Retry Render ↻
                      </button>
                    );
                  } else {
                    return (
                      <button className="open-btn ghost" onClick={() => addToQueue(detail.set_id)}>
                        Re-render ↻
                      </button>
                    );
                  }
                } else {
                  return (
                    <button className="open-btn ghost" onClick={() => addToQueue(detail.set_id)}>
                      Queue Render
                    </button>
                  );
                }
              })()}
              {detail.has_preview && (
                <button className="open-btn danger-btn" onClick={() => removePreview(detail.set_id)}>
                  Remove Preview
                </button>
              )}
            </div>
            <p className="meta">
              {detail.project} · {detail.tempo ?? "?"} bpm · {detail.time_signature ?? "?"} ·{" "}
              {detail.live_version ?? "unknown version"}
            </p>
            {detail.preview_missing && (
              <p className="warn">⚠ The preview file was missing from disk and has been removed from the database.</p>
            )}
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

      {showProgressModal && (
        <div className="modal-overlay" onClick={() => setShowProgressModal(false)}>
          <div className="modal-content" onClick={(e) => e.stopPropagation()}>
            <div className="modal-header">
              <h2 className="modal-title">
                {scanning ? "Scanning Library..." : "Scan Complete"}
              </h2>
              <button
                className="modal-close-btn"
                onClick={() => setShowProgressModal(false)}
                title={scanning ? "Run in background" : "Close"}
              >
                ×
              </button>
            </div>

            <div className="scan-stats-row">
              <div className="scan-stat">
                <span className="scan-stat-label">Indexed</span>
                <span className="scan-stat-value">{liveStats.indexed}</span>
              </div>
              <div className="scan-stat">
                <span className="scan-stat-label">Unchanged</span>
                <span className="scan-stat-value">{liveStats.unchanged}</span>
              </div>
              <div className="scan-stat">
                <span className="scan-stat-label">Previews</span>
                <span className="scan-stat-value">{liveStats.previews}</span>
              </div>
              <div className="scan-stat">
                <span className="scan-stat-label">Errors</span>
                <span className="scan-stat-value error-text">{liveStats.errors}</span>
              </div>
            </div>

            <div className="scan-log-console" ref={logConsoleRef}>
              {scanLogs.length === 0 && (
                <div style={{ color: "var(--dim)" }}>Waiting for progress updates...</div>
              )}
              {scanLogs.map((log, index) => {
                let className = "";
                if (log.startsWith("ERROR")) className = "log-error";
                else if (log.startsWith("preview")) className = "log-preview";
                else if (log.startsWith("indexed")) className = "log-indexed";
                return (
                  <div key={index} className={`log-line ${className}`}>
                    {log}
                  </div>
                );
              })}
            </div>

            <div className="modal-footer">
              {scanning && (
                <button
                  className="open-btn ghost"
                  style={{ marginRight: "auto" }}
                  onClick={cancelScan}
                >
                  Cancel Scan
                </button>
              )}
              <button
                className="open-btn"
                disabled={scanning}
                onClick={() => setShowProgressModal(false)}
              >
                {scanning ? "Scanning..." : "Done"}
              </button>
            </div>
          </div>
        </div>
      )}

      {showQueueModal && (
        <div className="modal-overlay" onClick={() => setShowQueueModal(false)}>
          <div className="modal-content" onClick={(e) => e.stopPropagation()}>
            <div className="modal-header">
              <h2 className="modal-title">Automated Render Queue</h2>
              <button
                className="modal-close-btn"
                onClick={() => setShowQueueModal(false)}
                title="Close"
              >
                ×
              </button>
            </div>

            <div className="queue-toggle-container">
              <div className="queue-toggle-info">
                <span className="queue-toggle-title">
                  {queueActive ? "Auto-Export: Active" : "Auto-Export: Paused"}
                </span>
                <span className="queue-toggle-desc">
                  When active, a background worker runs Ableton Live to render previews.
                </span>
              </div>
              <label className="switch">
                <input
                  type="checkbox"
                  checked={queueActive}
                  onChange={(e) => toggleQueue(e.target.checked)}
                />
                <span className="slider" />
              </label>
            </div>

            <div className="queue-warning-box">
              <strong>⚠️ Automation Notice:</strong> This uses AppleScript to drive Ableton Live.
              Ableton Live will open and run in the foreground while rendering.
              Please do not use your mouse or keyboard while a render is in progress.
            </div>

            <div className="queue-list-container">
              {queue.length === 0 ? (
                <div className="empty" style={{ padding: "32px 16px" }}>
                  <p>Queue is empty.</p>
                  <p className="hint" style={{ fontSize: "11px" }}>
                    Select a set from the list and click "Queue Render" to add jobs.
                  </p>
                </div>
              ) : (
                <table className="queue-table">
                  <thead>
                    <tr>
                      <th>Project / Set</th>
                      <th>Status</th>
                      <th className="job-actions">Action</th>
                    </tr>
                  </thead>
                  <tbody>
                    {queue.map((job) => (
                      <tr key={job.id}>
                        <td>
                          <div className="queue-job-title">{job.project_name}</div>
                          <div className="queue-job-path" title={job.als_path}>
                            {job.als_path.split("/").pop()}
                          </div>
                          {job.error && (
                            <div className="job-error-text">
                              Error: {job.error}
                            </div>
                          )}
                        </td>
                        <td>
                          <span className={`status-badge ${job.status}`}>
                            {job.status}
                          </span>
                        </td>
                        <td className="job-actions">
                          {(job.status === "failed" || job.status === "processing") && (
                            <button
                              className="remove-job-btn"
                              onClick={() => addToQueue(job.set_id)}
                              title="Retry render"
                              style={{ marginRight: "6px", color: "var(--accent)" }}
                            >
                              ↻
                            </button>
                          )}
                          <button
                            className="remove-job-btn"
                            onClick={() => removeFromQueue(job.id)}
                            title="Remove from queue"
                          >
                            ×
                          </button>
                        </td>
                      </tr>
                    ))}
                  </tbody>
                </table>
              )}
            </div>

            <div className="modal-footer">
              <button
                className="open-btn ghost"
                style={{ marginRight: "10px" }}
                onClick={retryFailed}
                disabled={!queue.some(j => j.status === 'failed')}
              >
                Retry Failed ↻
              </button>
              <button
                className="open-btn ghost"
                style={{ marginRight: "auto" }}
                onClick={clearCompleted}
                disabled={!queue.some(j => j.status === 'completed' || j.status === 'failed')}
              >
                Clear Done / Failed
              </button>
              <button
                className="open-btn"
                onClick={() => setShowQueueModal(false)}
              >
                Close
              </button>
            </div>
          </div>
        </div>
      )}

      {showWatchModal && (
        <div className="modal-overlay" onClick={() => setShowWatchModal(false)}>
          <div className="modal-content" onClick={(e) => e.stopPropagation()}>
            <div className="modal-header">
              <h2 className="modal-title">Watch Folders</h2>
              <button
                className="modal-close-btn"
                onClick={() => setShowWatchModal(false)}
                title="Close"
              >
                ×
              </button>
            </div>

            <div className="watch-section">
              <h3>Managed Folders</h3>
              <p className="hint">
                Add folders containing bounces/exports (e.g., your generic Music/Bounces directory).
                We will match these audio files against sets in your library that don't have previews.
              </p>
              
              <div className="watch-folders-list">
                {watchFolders.length === 0 ? (
                  <div className="empty-small">No watch folders added yet.</div>
                ) : (
                  <ul className="watch-list">
                    {watchFolders.map(([id, path]) => (
                      <li key={id} className="watch-item">
                        <span className="watch-path" title={path}>{path}</span>
                        <button
                          className="watch-remove-btn"
                          onClick={() => removeWatchFolder(id)}
                          title="Remove this folder"
                        >
                          ×
                        </button>
                      </li>
                    ))}
                  </ul>
                )}
              </div>
              <button className="open-btn" onClick={addWatchFolder}>
                + Add Watch Folder
              </button>
            </div>

            <div className="watch-section" style={{ marginTop: "20px", borderTop: "1px solid var(--border)", paddingTop: "15px" }}>
              <div style={{ display: "flex", justifyContent: "space-between", alignItems: "center" }}>
                <h3>Suggested Bounce Matches</h3>
                <button
                  className="open-btn ghost"
                  style={{ padding: "4px 8px", fontSize: "11px" }}
                  onClick={refreshSuggestions}
                  disabled={loadingSuggestions}
                >
                  {loadingSuggestions ? "Scanning..." : "Scan/Refresh ↻"}
                </button>
              </div>

              <div className="suggestions-list-container" style={{ marginTop: "8px" }}>
                {loadingSuggestions ? (
                  <div className="loading-container">
                    <span className="pulse-dot" /> Scanning watch folders for matching audio files...
                  </div>
                ) : suggestions.length === 0 ? (
                  <div className="empty" style={{ padding: "32px 16px" }}>
                    <p>No suggested matches found.</p>
                    <p className="hint" style={{ fontSize: "11px" }}>
                      Make sure your watch folders contain audio files that match set filenames or project folder names.
                    </p>
                  </div>
                ) : (
                  <table className="queue-table">
                    <thead>
                      <tr>
                        <th>Ableton Set</th>
                        <th>Bounce / Match</th>
                        <th className="job-actions" style={{ width: "160px" }}>Actions</th>
                      </tr>
                    </thead>
                    <tbody>
                      {suggestions.map((s) => (
                        <tr key={`${s.set_id}-${s.audio_path}`}>
                          <td>
                            <div className="queue-job-title">{s.project_name}</div>
                            <div className="queue-job-path">{s.set_name}</div>
                          </td>
                          <td>
                            <div className="queue-job-title" title={s.audio_path}>{s.file_name}</div>
                            <div className="queue-job-path" style={{ color: "var(--accent)" }}>
                              {Math.round(s.confidence * 100)}% match
                            </div>
                          </td>
                          <td className="job-actions" style={{ width: "160px" }}>
                            <button
                              className="remove-job-btn"
                              style={{ color: "var(--accent)", marginRight: "10px" }}
                              onClick={() => playSuggestionTrack(s)}
                              title="Play bounce to check match"
                            >
                              ▶ Play
                            </button>
                            <button
                              className="remove-job-btn"
                              style={{ color: "#85e3b2", marginRight: "10px" }}
                              onClick={() => linkSuggestion(s.set_id, s.audio_path)}
                              title="Use this bounce as the set preview"
                            >
                              ✓ Link
                            </button>
                            <button
                              className="remove-job-btn"
                              style={{ color: "#e38585" }}
                              onClick={() => ignoreSuggestion(s.set_id, s.audio_path)}
                              title="Don't suggest this match again"
                            >
                              × Ignore
                            </button>
                          </td>
                        </tr>
                      ))}
                    </tbody>
                  </table>
                )}
              </div>
            </div>

            <div className="modal-footer">
              <button className="open-btn" onClick={() => setShowWatchModal(false)}>
                Done
              </button>
            </div>
          </div>
        </div>
      )}

      {track && <PlayerBar track={track} onClose={() => setTrack(null)} />}

    </div>
  );
}
