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
  artist: string | null;
  als_path: string;
  tempo: number | null;
  tempos: number[];
  time_signature: string | null;
  live_version: string | null;
  has_preview: boolean;
  preview_source?: string;
  preview_duration: number | null;
  in_list: boolean;
};

type ListInfo = [number, string, number]; // [id, name, item_count]

const formatTempo = (tempo: number | null, tempos: number[] | undefined) => {
  if (tempos && tempos.length > 1) {
    return tempos.join(", ");
  }
  return tempo ?? "?";
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
  fidelity: Fidelity | null;
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
  score: number | null;
  fidelity: string | null; // JSON Renderability report
};

// Queue display order: surface what needs attention first. Failed at the top
// (need a fix/retry), then completed (confirm what worked), then the upcoming
// pending jobs, then the one currently processing. Within a status group:
// pending sorts by renderability score (easy-first, matching the worker), the
// rest by newest first.
const QUEUE_STATUS_ORDER: Record<ExportJob["status"], number> = {
  failed: 0,
  completed: 1,
  pending: 2,
  processing: 3,
};

function byQueueStatus(a: ExportJob, b: ExportJob): number {
  const sa = QUEUE_STATUS_ORDER[a.status] ?? 99;
  const sb = QUEUE_STATUS_ORDER[b.status] ?? 99;
  if (sa !== sb) return sa - sb;
  if (a.status === "pending") {
    const ca = a.score ?? -1;
    const cb = b.score ?? -1;
    if (ca !== cb) return cb - ca; // higher renderability first
  }
  return b.id - a.id; // newest first within the group
}

type Fidelity = {
  score: number;
  plugins_total: number;
  missing_plugins: string[];
  samples_total: number;
  samples_missing: number;
  samples_evicted: number;
};

const parseFidelity = (j: string | null | undefined): Fidelity | null => {
  if (!j) return null;
  try {
    return JSON.parse(j) as Fidelity;
  } catch {
    return null;
  }
};

const fidelitySummary = (f: Fidelity): string => {
  const parts: string[] = [];
  if (f.missing_plugins.length)
    parts.push(`missing plugins: ${f.missing_plugins.join(", ")}`);
  if (f.samples_missing) parts.push(`${f.samples_missing} samples missing`);
  if (f.samples_evicted) parts.push(`${f.samples_evicted} samples in iCloud`);
  return parts.join(" · ");
};

type Suggestion = {
  set_id: number;
  set_name: string;
  project_name: string;
  audio_path: string;
  file_name: string;
  confidence: number;
  has_preview: boolean;
  current_preview: string | null;
};

type Detail = {
  set_id: number;
  project: string;
  artist?: string | null;
  artist_override?: string | null;
  project_artist?: string | null;
  als_path: string;
  live_version: string | null;
  tempo: number | null;
  tempos: number[];
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
  const [artist, setArtist] = useState("");
  const [artistDraft, setArtistDraft] = useState("");
  const [bulkArtist, setBulkArtist] = useState("");
  // Lists (favorites + collections)
  const [lists, setLists] = useState<ListInfo[]>([]);
  const [listFilter, setListFilter] = useState(""); // "" = all, else list id as string
  const [listMenuSet, setListMenuSet] = useState<number | null>(null); // star popover target
  const [menuPos, setMenuPos] = useState<{ x: number; y: number }>({ x: 0, y: 0 });
  const [menuMemberships, setMenuMemberships] = useState<Set<number>>(new Set());
  const [newListName, setNewListName] = useState("");
  const [showListsModal, setShowListsModal] = useState(false);
  const [listDrafts, setListDrafts] = useState<Record<number, string>>({}); // id -> edited name
  const [confirmDeleteList, setConfirmDeleteList] = useState<number | null>(null);
  const [modalNewList, setModalNewList] = useState("");
  const [detailInList, setDetailInList] = useState(false); // star fill in the detail pane
  const [hits, setHits] = useState<SearchHit[]>([]);
  const [stats, setStats] = useState<Stats | null>(null);
  const [detail, setDetail] = useState<Detail | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [track, setTrack] = useState<PlayerTrack | null>(null);
  const [sortBy, setSortBy] = useState("modified");
  const [dateModified, setDateModified] = useState("");
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

  const [selected, setSelected] = useState<Set<number>>(new Set());
  const [queue, setQueue] = useState<ExportJob[]>([]);
  const [queueActive, setQueueActive] = useState(false);
  const [showQueueModal, setShowQueueModal] = useState(false);
  const [showWatchModal, setShowWatchModal] = useState(false);
  const [watchFolders, setWatchFolders] = useState<[number, string][]>([]);
  const [suggestions, setSuggestions] = useState<Suggestion[]>([]);
  const [loadingSuggestions, setLoadingSuggestions] = useState(false);
  const [selectedSuggestions, setSelectedSuggestions] = useState<string[]>([]);
  const [linkProgress, setLinkProgress] = useState<[number, number] | null>(null);

  const runSearch = useCallback(async () => {
    try {
      setError(null);
      const res = await invoke<SearchHit[]>("search", {
        text: text || null,
        min_bpm: minBpm ? parseFloat(minBpm) : null,
        max_bpm: maxBpm ? parseFloat(maxBpm) : null,
        plugin: plugin || null,
        artist: artist || null,
        list_id: listFilter ? parseInt(listFilter) : null,
        sort_by: sortBy || null,
        date_modified: dateModified || null,
        date_scanned: null,
        has_preview: hasPreviewFilter || null,
      });
      setHits(res);
    } catch (e) {
      setError(String(e));
    }
  }, [text, minBpm, maxBpm, plugin, artist, listFilter, sortBy, dateModified, hasPreviewFilter]);

  const refreshLists = useCallback(() => {
    invoke<ListInfo[]>("get_lists").then(setLists).catch((e) => setError(String(e)));
  }, []);

  // Any filter switched off its default? Drives the Clear button.
  const anyFilterActive =
    !!text || !!minBpm || !!maxBpm || !!plugin || !!artist ||
    !!listFilter || sortBy !== "modified" || !!dateModified || hasPreviewFilter !== "all";

  const clearFilters = () => {
    setText(""); setMinBpm(""); setMaxBpm(""); setPlugin(""); setArtist("");
    setListFilter(""); setSortBy("modified"); setDateModified(""); setHasPreviewFilter("all");
  };

  // Open the star popover for a set: position it next to the clicked star
  // (a fixed-position popup so the table/row never clips it), load memberships.
  const openListMenu = async (setId: number, anchor: HTMLElement) => {
    const r = anchor.getBoundingClientRect();
    const W = 240;
    const x = Math.min(r.left, window.innerWidth - W - 12);
    const y = Math.min(r.bottom + 4, window.innerHeight - 240);
    setMenuPos({ x: Math.max(8, x), y: Math.max(8, y) });
    setNewListName("");
    try {
      const ids = await invoke<number[]>("lists_for_set", { set_id: setId });
      setMenuMemberships(new Set(ids));
      setListMenuSet(setId);
      refreshLists();
    } catch (e) {
      setError(String(e));
    }
  };

  // Refresh the detail-pane star's fill (is the open set in any list?).
  const refreshDetailInList = async (setId: number) => {
    try {
      const ids = await invoke<number[]>("lists_for_set", { set_id: setId });
      setDetailInList(ids.length > 0);
    } catch {
      /* non-fatal */
    }
  };

  // Toggle a set's membership in a list from the popover.
  const toggleMembership = async (listId: number, setId: number, isMember: boolean) => {
    try {
      const cmd = isMember ? "remove_set_from_list" : "add_set_to_list";
      await invoke(cmd, { list_id: listId, set_id: setId });
      setMenuMemberships((prev) => {
        const next = new Set(prev);
        if (isMember) next.delete(listId); else next.add(listId);
        return next;
      });
      refreshLists();
      runSearch();
      if (detail?.set_id === setId) refreshDetailInList(setId);
    } catch (e) {
      setError(String(e));
    }
  };

  const createListAndAdd = async (setId: number) => {
    const name = newListName.trim();
    if (!name) return;
    try {
      const id = await invoke<number>("create_list", { name });
      await invoke("add_set_to_list", { list_id: id, set_id: setId });
      setMenuMemberships((prev) => new Set(prev).add(id));
      setNewListName("");
      refreshLists();
      runSearch();
      if (detail?.set_id === setId) refreshDetailInList(setId);
    } catch (e) {
      setError(String(e));
    }
  };

  // ---- List management (rename / delete) ----
  const openListsModal = () => {
    setListMenuSet(null);
    setConfirmDeleteList(null);
    setModalNewList("");
    setListDrafts(Object.fromEntries(lists.map(([id, name]) => [id, name])));
    setShowListsModal(true);
  };

  const saveListName = async (id: number) => {
    const name = (listDrafts[id] ?? "").trim();
    if (!name) return;
    try {
      await invoke("rename_list", { list_id: id, name });
      await refreshLists();
      runSearch(); // list label shows in the filter dropdown
    } catch (e) {
      setError(String(e));
    }
  };

  const deleteListConfirmed = async (id: number) => {
    try {
      await invoke("delete_list", { list_id: id });
      if (listFilter === String(id)) setListFilter(""); // was the active filter
      setConfirmDeleteList(null);
      await refreshLists();
      runSearch(); // stars may go hollow for sets only in this list
    } catch (e) {
      setError(String(e));
    }
  };

  const createListInModal = async () => {
    const name = modalNewList.trim();
    if (!name) return;
    try {
      await invoke<number>("create_list", { name });
      setModalNewList("");
      const fresh = await invoke<ListInfo[]>("get_lists");
      setLists(fresh);
      setListDrafts(Object.fromEntries(fresh.map(([id, n]) => [id, n])));
    } catch (e) {
      setError(String(e));
    }
  };

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
    setSelectedSuggestions([]);
    try {
      const res = await invoke<Suggestion[]>("get_watch_suggestions");
      setSuggestions(res);
      // Auto-select the best candidate per set — only for sets that don't
      // have a preview yet (previewed sets are shown but never auto-linked).
      const best = new Map<number, Suggestion>();
      for (const s of res) {
        if (s.has_preview) continue;
        const cur = best.get(s.set_id);
        if (!cur || s.confidence > cur.confidence) best.set(s.set_id, s);
      }
      setSelectedSuggestions(
        Array.from(best.values()).map((s) => `${s.set_id}:${s.audio_path}`)
      );
    } catch (e) {
      setError(String(e));
    } finally {
      setLoadingSuggestions(false);
    }
  }, []);


  // Keep the selection in sync with visible results: ids that fall out of
  // the current search are dropped so "Queue N" never exports hidden rows.
  useEffect(() => {
    setSelected((prev) => {
      const visible = new Set(hits.map((h) => h.set_id));
      const next = new Set([...prev].filter((id) => visible.has(id)));
      return next.size === prev.size ? prev : next;
    });
  }, [hits]);

  // Anchor for shift-click range selection (last row whose checkbox was clicked).
  const selectAnchor = useRef<number | null>(null);

  // NOTE: all logic lives OUTSIDE the setSelected updater on purpose —
  // StrictMode double-invokes updaters, and mutating the anchor ref inside
  // one broke shift-ranges (second invocation saw the anchor already moved
  // and fell back to a single toggle).
  const handleRowCheck = (setId: number, shift: boolean) => {
    const next = new Set(selected);
    let ranged = false;
    if (shift) {
      const ids = hits.map((h) => h.set_id);
      // No anchor yet (nothing clicked before): shift-click ranges from the
      // top of the list, Finder-style.
      const a = selectAnchor.current !== null ? ids.indexOf(selectAnchor.current) : 0;
      const b = ids.indexOf(setId);
      if (a !== -1 && b !== -1) {
        // Whether the range selects or deselects follows the clicked row.
        const selecting = !selected.has(setId);
        const [lo, hi] = a < b ? [a, b] : [b, a];
        for (let i = lo; i <= hi; i++) {
          if (selecting) {
            next.add(ids[i]);
          } else {
            next.delete(ids[i]);
          }
        }
        ranged = true;
      }
    }
    if (!ranged) {
      if (next.has(setId)) {
        next.delete(setId);
      } else {
        next.add(setId);
      }
    }
    selectAnchor.current = setId;
    setSelected(next);
  };

  const toggleSelectAll = () => {
    setSelected((prev) =>
      prev.size === hits.length ? new Set() : new Set(hits.map((h) => h.set_id))
    );
  };

  const queueSelected = async () => {
    if (selected.size === 0) return;
    try {
      setError(null);
      const n = await invoke<number>("add_to_export_queue_bulk", {
        set_ids: Array.from(selected),
      });
      setSelected(new Set());
      refreshQueue();
      refreshStats();
      setError(`Note: ${n} render${n === 1 ? "" : "s"} queued.`);
    } catch (e) {
      setError(String(e));
    }
  };

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

  const retriageJobs = async () => {
    try {
      setError(null);
      await invoke("retriage_jobs");
      refreshQueue();
    } catch (e) {
      setError(String(e));
    }
  };

  const clearAllJobs = async () => {
    try {
      setError(null);
      await invoke("clear_all_jobs");
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
    refreshLists();
  }, [refreshStats, refreshQueue, refreshWatchFolders, refreshSuggestions, refreshLists]);

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
      } else if (line.startsWith("preview") || line.startsWith("linked")) {
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
      refreshSuggestions();
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

  const bulkPreviewScan = async () => {
    try {
      setScanLogs([]);
      setLiveStats({ indexed: 0, unchanged: 0, previews: 0, errors: 0 });
      setShowProgressModal(true);
      setScanning(true);
      setError(null);
      setScanMsg(null);
      const s = await invoke<ScanSummary>("bulk_preview_scan");
      setLiveStats({
        indexed: s.indexed,
        unchanged: s.unchanged,
        previews: s.harvested,
        errors: s.errors,
      });
      setScanMsg(
        `Preview scan complete: ${s.harvested} preview(s) matched` +
          (s.errors ? `, ${s.errors} errors` : ""),
      );
      refreshStats();
      runSearch();
      refreshSuggestions();
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

  const toggleSelectSuggestion = (key: string) => {
    setSelectedSuggestions((prev) =>
      prev.includes(key) ? prev.filter((k) => k !== key) : [...prev, key]
    );
  };

  // "Select all" = the best candidate per preview-LESS set. Projects that
  // already have a preview are only ever shown — never picked automatically;
  // linking those requires checking them individually.
  const selectableSuggestionKeys = () => {
    const best = new Map<number, Suggestion>();
    for (const s of suggestions) {
      if (s.has_preview) continue;
      const cur = best.get(s.set_id);
      if (!cur || s.confidence > cur.confidence) best.set(s.set_id, s);
    }
    return Array.from(best.values()).map((s) => `${s.set_id}:${s.audio_path}`);
  };

  const toggleSelectAllSuggestions = () => {
    const all = selectableSuggestionKeys();
    setSelectedSuggestions((prev) =>
      all.length > 0 && all.every((k) => prev.includes(k)) ? [] : all
    );
  };

  // Bulk link runs as a background job exactly like scan_folder /
  // bulk_preview_scan: progress modal + banner, shared Cancel button.
  const linkSelectedSuggestions = async () => {
    if (selectedSuggestions.length === 0 || linkProgress || scanning) return;
    try {
      setError(null);
      const matches = selectedSuggestions.map((key) => {
        const firstColonIdx = key.indexOf(":");
        const setId = parseInt(key.substring(0, firstColonIdx), 10);
        const audioPath = key.substring(firstColonIdx + 1);
        return [setId, audioPath];
      });
      setScanLogs([]);
      setLiveStats({ indexed: 0, unchanged: 0, previews: 0, errors: 0 });
      setShowProgressModal(true);
      setScanning(true);
      setScanMsg(null);
      setLinkProgress([0, matches.length]);
      const linked = await invoke<number>("link_watch_suggestions", { matches });
      setSelectedSuggestions([]);
      refreshSuggestions();
      runSearch();
      refreshStats();
      setScanMsg(`${linked} of ${matches.length} bounce${matches.length === 1 ? "" : "s"} linked`);
    } catch (e) {
      const msg = String(e);
      if (msg.includes("cancelled")) {
        setScanMsg("Link cancelled");
        refreshSuggestions();
        runSearch();
        refreshStats();
      } else {
        setError(msg);
      }
      setShowProgressModal(false);
    } finally {
      setScanning(false);
      setLinkProgress(null);
    }
  };

  // Live progress while a bulk link runs.
  useEffect(() => {
    let active = true;
    let unsubscribed = false;
    let unlistenFn: (() => void) | null = null;

    listen<[number, number]>("link-progress", (event) => {
      if (!active) return;
      setLinkProgress(event.payload);
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
      // Prefill the artist editor with this set's OWN override (blank if it's
      // only inheriting the project's derived artist).
      setArtistDraft(d.artist_override ?? "");
      refreshDetailInList(id);
      if (d.preview_missing) {
        setError("Note: The preview file was missing from disk and has been removed from the database.");
        runSearch();
        refreshStats();
      }
    } catch (e) {
      setError(String(e));
    }
  };

  // Manually attach any audio file as this set's preview (oddly-named bounces).
  const attachPreviewFile = async (setId: number) => {
    try {
      setError(null);
      const file = await openDialog({
        multiple: false,
        directory: false,
        filters: [{ name: "Audio", extensions: ["wav", "aif", "aiff", "mp3", "m4a", "flac", "ogg"] }],
      });
      if (!file || typeof file !== "string") return;
      await invoke("attach_preview", { set_id: setId, audio_path: file });
      openDetail(setId);
      runSearch();
      refreshStats();
      setError(`Note: attached "${fileName(file)}" as the preview.`);
    } catch (e) {
      setError(String(e));
    }
  };

  // Manual artist assignment. `scope` 'set' overrides just this set; 'project'
  // tags the whole folder. Blank draft clears.
  const saveArtist = async (scope: "set" | "project") => {
    if (!detail) return;
    try {
      const value = artistDraft.trim() || null;
      const cmd = scope === "set" ? "set_artist" : "set_project_artist";
      await invoke(cmd, { set_id: detail.set_id, artist: value });
      await openDetail(detail.set_id);
      runSearch();
      refreshStats();
    } catch (e) {
      setError(String(e));
    }
  };

  const tagSelectedArtist = async () => {
    try {
      const value = bulkArtist.trim() || null;
      const n = await invoke<number>("set_artist_bulk", {
        set_ids: [...selected],
        artist: value,
      });
      setScanMsg(`${value ? "Tagged" : "Cleared artist on"} ${n} set${n === 1 ? "" : "s"}`);
      setBulkArtist("");
      runSearch();
      refreshStats();
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
        // Check source from detail or hit if available, for now keep generic or rely on UI state
        setError("Note: Preview unlinked — the sketch file was deleted, or the bounce was kept on disk.");
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

    // 1. If we have a preview, show standard controls
    if (hit.has_preview) {
      if (hit.preview_source === "sketch") {
        return (
          <div style={{ display: "flex", gap: "6px", alignItems: "center", justifyContent: "flex-end" }}>
            <button className="play-btn sketch" title="Play sketch preview" onClick={(e) => { e.stopPropagation(); playPreview(hit); }}>▶ Preview</button>
            <button className="play-btn" style={{ borderColor: "var(--border)", color: "var(--dim)", fontSize: "11px", padding: "3px 6px" }} onClick={(e) => { e.stopPropagation(); addToQueue(hit.set_id); }} title="Queue automated Live export">Queue Render</button>
            <button className="open-btn" title="Open in Ableton Live" onClick={(e) => { e.stopPropagation(); openInLive(hit.set_id); }}>Open</button>
          </div>
        );
      } else {
        return (
          <div style={{ display: "flex", gap: "6px", alignItems: "center", justifyContent: "flex-end" }}>
            <button className="play-btn" title="Play preview" onClick={(e) => { e.stopPropagation(); playPreview(hit); }}>▶ Play</button>
            <button className="play-btn" style={{ borderColor: "var(--border)", color: "var(--dim)", fontSize: "11px", padding: "3px 6px" }} onClick={(e) => { e.stopPropagation(); addToQueue(hit.set_id); }} title="Re-render/update audio preview">Update ↻</button>
            <button className="open-btn" title="Open in Ableton Live" onClick={(e) => { e.stopPropagation(); openInLive(hit.set_id); }}>Open</button>
          </div>
        );
      }
    }

    // 2. If no preview, show Sketch + (Export Queue status or button)
    const sketchButton = (
      <button
        className="play-btn sketch"
        onClick={(e) => {
          e.stopPropagation();
          invoke("sketch_preview", { set_id: hit.set_id }).then(runSearch).catch((e) => setError(String(e)));
        }}
        title="Generate fast sketch preview"
      >
        Sketch
      </button>
    );

    let jobActions = null;
    if (job) {
      if (job.status === "processing") {
        jobActions = (
          <button className="play-btn" style={{ borderColor: "var(--accent)", color: "var(--accent)" }} onClick={(e) => { e.stopPropagation(); removeFromQueue(job.id); }} title="Rendering in background. Click to cancel.">⚙️ Rendering ×</button>
        );
      } else if (job.status === "pending") {
        jobActions = (
          <button className="play-btn" style={{ borderColor: "var(--dim)", color: "var(--dim)" }} onClick={(e) => { e.stopPropagation(); removeFromQueue(job.id); }} title="Queued (Pending). Click to remove.">⏳ Queued ×</button>
        );
      } else {
        jobActions = (
          <button className="play-btn" style={{ borderColor: "#ff8f8f", color: "#ff8f8f" }} onClick={(e) => { e.stopPropagation(); addToQueue(hit.set_id); }} title={`Failed: ${job.error}. Click to retry.`}>❌ Retry ↻</button>
        );
      }
    } else {
      jobActions = (
        <button className="play-btn" style={{ borderColor: "var(--border)", color: "var(--accent)" }} onClick={(e) => { e.stopPropagation(); addToQueue(hit.set_id); }} title="Queue automated Live export">Queue Render</button>
      );
    }

    return (
      <div style={{ display: "flex", gap: "6px", alignItems: "center", justifyContent: "flex-end" }}>
        {sketchButton}
        {jobActions}
        <button className="open-btn" title="Open in Ableton Live" onClick={(e) => { e.stopPropagation(); openInLive(hit.set_id); }}>Open</button>
      </div>
    );
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
        subtitle:
          `${h.project} · ${p.source}` +
          (p.confidence < 0.85 ? ` (${Math.round(p.confidence * 100)}% match)` : "") +
          (p.fidelity && fidelitySummary(p.fidelity) ? ` · ⚠ ${fidelitySummary(p.fidelity)}` : ""),
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

  // Play the open set's current primary preview (detail pane).
  const playDetailPreview = async () => {
    if (!detail) return;
    try {
      setError(null);
      const p = await invoke<PreviewInfo | null>("preview", { set_id: detail.set_id });
      if (!p) {
        setError("No preview for this set yet.");
        return;
      }
      setTrack({
        setId: detail.set_id,
        title: fileName(detail.als_path).replace(/\.als$/, ""),
        subtitle:
          `${detail.project} · ${p.source}` +
          (p.confidence < 0.85 ? ` (${Math.round(p.confidence * 100)}% match)` : ""),
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
          disabled={scanning}
          title="Re-derive artists from project paths already in the catalog — no scanning"
          onClick={async () => {
            try {
              setError(null);
              const n = await invoke<number>("reindex_artists");
              setScanMsg(`Reindexed artists: ${n} project(s) tagged`);
              runSearch();
              refreshStats();
            } catch (e) {
              setError(String(e));
            }
          }}
        >
          Reindex Artists
        </button>
        <button className="scan-btn" onClick={bulkPreviewScan} disabled={scanning} style={{ marginLeft: "10px" }} title="Match unmatched sets against their project folders and watch folders">
          {scanning ? "Scanning Previews…" : "Scan Previews…"}
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
          className={`grow${text ? " active" : ""}`}
          placeholder="Search projects, sets, tracks, devices, samples…"
          value={text}
          onChange={(e) => setText(e.target.value)}
        />
        <input
          className={`bpm${minBpm ? " active" : ""}`}
          placeholder="min bpm"
          value={minBpm}
          onChange={(e) => setMinBpm(e.target.value)}
        />
        <input
          className={`bpm${maxBpm ? " active" : ""}`}
          placeholder="max bpm"
          value={maxBpm}
          onChange={(e) => setMaxBpm(e.target.value)}
        />
        <input
          className={`plugin${plugin ? " active" : ""}`}
          placeholder="plugin…"
          value={plugin}
          onChange={(e) => setPlugin(e.target.value)}
        />
        <input
          className={`plugin${artist ? " active" : ""}`}
          placeholder="artist…"
          value={artist}
          onChange={(e) => setArtist(e.target.value)}
        />
        <select
          className={`sort-select${listFilter ? " active" : ""}`}
          value={listFilter}
          onChange={(e) => setListFilter(e.target.value)}
          title="Show only sets in a list"
        >
          <option value="">All lists</option>
          {lists.map(([id, name, count]) => (
            <option key={id} value={String(id)}>★ {name} ({count})</option>
          ))}
        </select>
        {lists.length > 0 && (
          <button
            className="sort-select"
            onClick={openListsModal}
            title="Rename or delete lists"
            style={{ cursor: "pointer", padding: "0 8px" }}
          >
            ⚙
          </button>
        )}
        <select
          className={`sort-select${sortBy !== "modified" ? " active" : ""}`}
          value={sortBy}
          onChange={(e) => setSortBy(e.target.value)}
        >
          <option value="modified">Recent</option>
          <option value="name">Name A–Z</option>
          <option value="artist">Artist A–Z</option>
          <option value="bpm">Tempo</option>
          <option value="previews">Previews first</option>
        </select>
        <select
          className={`sort-select${dateModified ? " active" : ""}`}
          value={dateModified}
          onChange={(e) => setDateModified(e.target.value)}
          style={{ width: "110px" }}
        >
          <option value="">Any date</option>
          <option value="today">Today</option>
          <option value="yesterday">Yesterday</option>
          <option value="week">This week</option>
          <option value="month">This month</option>
        </select>
        <select
          className={`sort-select${hasPreviewFilter !== "all" ? " active" : ""}`}
          value={hasPreviewFilter}
          onChange={(e) => setHasPreviewFilter(e.target.value)}
          style={{ width: "110px" }}
        >
          <option value="all">Any preview</option>
          <option value="yes">Has preview</option>
          <option value="no">No preview</option>
        </select>
        {anyFilterActive && (
          <button
            className="clear-filters"
            onClick={clearFilters}
            title="Reset all filters to default"
          >
            ✕ Clear
          </button>
        )}
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
          {hits.length > 0 && (
            <div className="select-bar">
              <label className="select-all" title="Select all results">
                <input
                  type="checkbox"
                  checked={selected.size === hits.length}
                  ref={(el) => {
                    if (el) el.indeterminate = selected.size > 0 && selected.size < hits.length;
                  }}
                  onChange={toggleSelectAll}
                />
                {selected.size > 0 ? `${selected.size} selected` : "Select all"}
              </label>
              {selected.size > 0 && (
                <>
                  <button
                    className="play-btn"
                    style={{ borderColor: "var(--accent)", color: "var(--accent)" }}
                    onClick={queueSelected}
                    title="Queue audio preview renders for all selected sets"
                  >
                    Queue {selected.size} render{selected.size === 1 ? "" : "s"}
                  </button>
                  <input
                    className="plugin"
                    placeholder="artist…"
                    value={bulkArtist}
                    onChange={(e) => setBulkArtist(e.target.value)}
                    onKeyDown={(e) => { if (e.key === "Enter") tagSelectedArtist(); }}
                    style={{ width: 130 }}
                  />
                  <button
                    className="play-btn"
                    onClick={tagSelectedArtist}
                    title="Set the artist on all selected sets (empty clears)"
                  >
                    Tag {selected.size}
                  </button>
                  <button className="play-btn" onClick={() => setSelected(new Set())}>
                    Clear
                  </button>
                </>
              )}
            </div>
          )}
          <table>
            <tbody>
              {hits.map((h) => (
                <tr
                  key={h.set_id}
                  className={`${detail?.set_id === h.set_id ? "selected" : ""}${selected.has(h.set_id) ? " checked" : ""}`}
                  onClick={(e) => {
                    // Finder-style: cmd-click toggles selection, shift-click
                    // extends the range from the last clicked row; plain
                    // click opens the detail pane.
                    if (e.metaKey || e.ctrlKey) {
                      handleRowCheck(h.set_id, false);
                    } else if (e.shiftKey) {
                      handleRowCheck(h.set_id, true);
                    } else {
                      openDetail(h.set_id);
                    }
                  }}
                >
                  <td
                    className="star"
                    onClick={(e) => e.stopPropagation()}
                    style={{ width: 24, textAlign: "center", cursor: "pointer" }}
                  >
                    <span
                      title={h.in_list ? "In a list — click to edit" : "Add to a list"}
                      onClick={(e) => openListMenu(h.set_id, e.currentTarget as HTMLElement)}
                      style={{
                        color: h.in_list ? "var(--accent, #e6b800)" : "var(--muted, #888)",
                        fontSize: "1.1em",
                        userSelect: "none",
                      }}
                    >
                      {h.in_list ? "★" : "☆"}
                    </span>
                  </td>
                  <td className="sel" onClick={(e) => e.stopPropagation()}>
                    <input
                      type="checkbox"
                      checked={selected.has(h.set_id)}
                      onChange={() => {}}
                      onClick={(e) => {
                        e.stopPropagation();
                        handleRowCheck(h.set_id, e.shiftKey);
                      }}
                    />
                  </td>
                  <td className="proj">
                    {h.project}
                    {h.artist && (
                      <span style={{ opacity: 0.55, marginLeft: 6 }}>· {h.artist}</span>
                    )}
                  </td>
                  <td className="set">{fileName(h.als_path)}</td>
                  <td className="num">{formatTempo(h.tempo, h.tempos)} bpm</td>
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
              <button
                title={detailInList ? "In a list — click to edit" : "Add to a list"}
                onClick={(e) => openListMenu(detail.set_id, e.currentTarget as HTMLElement)}
                style={{
                  border: "none", background: "none", cursor: "pointer",
                  fontSize: "1.3em", lineHeight: 1, marginLeft: "auto", padding: "0 6px",
                  color: detailInList ? "var(--accent, #e6b800)" : "var(--muted, #888)",
                }}
              >
                {detailInList ? "★" : "☆"}
              </button>
              <button onClick={() => setDetail(null)}>×</button>
            </div>
            <div className="detail-actions">
              <button className="open-btn" onClick={() => openInLive(detail.set_id)}>
                Open in Live
              </button>
              <button className="open-btn ghost" onClick={() => openInLive(detail.set_id, true)}>
                Reveal in Finder
              </button>
              <button
                className="open-btn ghost"
                onClick={async () => {
                  try {
                    setError(null);
                    const n = await invoke<number>("scan_set_folder_previews", {
                      set_id: detail.set_id,
                    });
                    openDetail(detail.set_id);
                    runSearch();
                    refreshStats();
                    setError(
                      n > 0
                        ? `Note: ${n} preview(s) harvested from the project folder.`
                        : "Note: no new preview files found in the project folder."
                    );
                  } catch (e) {
                    setError(String(e));
                  }
                }}
                title="Scan this project's folder for bounce/render files to use as previews"
              >
                Scan Folder for Previews
              </button>
              <button
                className="open-btn ghost"
                onClick={() => attachPreviewFile(detail.set_id)}
                title="Pick any audio file to use as this set's preview (for oddly-named bounces)"
              >
                Attach Audio…
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
                    <div style={{ display: "flex", gap: "6px", flexWrap: "wrap" }}>
                      <button className="open-btn ghost" onClick={() => addToQueue(detail.set_id)}>
                        Queue Render
                      </button>
                      <button
                        className="open-btn ghost"
                        style={{ borderColor: "#e6b800", color: "#e6b800" }}
                        onClick={() => invoke("sketch_preview", { set_id: detail.set_id }).then(() => openDetail(detail.set_id)).catch((e) => setError(String(e)))}
                      >
                        Sketch Render
                      </button>
                    </div>
                  );
                }
              })()}
            </div>
            <p className="meta">
              {detail.project}
              {detail.artist ? ` · ${detail.artist}` : ""} · {formatTempo(detail.tempo, detail.tempos)} bpm · {detail.time_signature ?? "?"} ·{" "}
              {detail.live_version ?? "unknown version"}
            </p>
            <div className="artist-editor" style={{ display: "flex", alignItems: "center", gap: 6, margin: "4px 0 10px", flexWrap: "wrap" }}>
              <span style={{ opacity: 0.7 }}>Artist:</span>
              <input
                value={artistDraft}
                placeholder={detail.project_artist ? `${detail.project_artist} (from path)` : "unassigned"}
                onChange={(e) => setArtistDraft(e.target.value)}
                onKeyDown={(e) => { if (e.key === "Enter") saveArtist("set"); }}
                style={{ width: 160 }}
              />
              <button className="open-btn" onClick={() => saveArtist("set")} title="Set the artist for just this set">
                Save (this set)
              </button>
              <button className="open-btn" onClick={() => saveArtist("project")} title="Set the artist for every set in this project folder">
                Apply to project
              </button>
              {detail.artist_override && (
                <span style={{ opacity: 0.55, fontSize: "0.85em" }}>
                  overriding {detail.project_artist ? `path "${detail.project_artist}"` : "(no path artist)"}
                </span>
              )}
            </div>
            {detail.has_preview && detail.preview_path && (
              <div style={{ display: "flex", alignItems: "center", gap: 8, margin: "2px 0 10px" }}>
                <span style={{ opacity: 0.7 }}>Preview:</span>
                <button className="open-btn" onClick={playDetailPreview} title="Play preview">▶</button>
                <span
                  title={detail.preview_path}
                  style={{ flex: 1, minWidth: 0, overflow: "hidden", textOverflow: "ellipsis", whiteSpace: "nowrap" }}
                >
                  {fileName(detail.preview_path)}
                </span>
                <button
                  className="open-btn ghost"
                  onClick={() => removePreview(detail.set_id)}
                  title="Unlink this preview — removes the link only, your audio file is kept"
                >
                  ✕ Unlink
                </button>
              </div>
            )}
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
                    {[...queue].sort(byQueueStatus).map((job) => (
                      <tr key={job.id}>
                        <td>
                          <div className="queue-job-title">{job.project_name}</div>
                          <div className="queue-job-path" title={job.als_path}>
                            {job.als_path.split("/").pop()}
                          </div>
                          {(() => {
                            const f = parseFidelity(job.fidelity);
                            return f && fidelitySummary(f) ? (
                              <div className="job-fidelity" title={fidelitySummary(f)}>
                                ⚠ {fidelitySummary(f)}
                              </div>
                            ) : null;
                          })()}
                          {job.error && (
                            <div className="job-error-container">
                              <div className="job-error-header">Error:</div>
                              <pre className="job-error-pre">
                                {job.error}
                              </pre>
                            </div>
                          )}
                        </td>
                        <td>
                          <span className={`status-badge ${job.status}`}>
                            {job.status}
                          </span>
                          {job.score != null && job.status === "pending" && (
                            <span
                              className={`score-badge ${job.score >= 0.9 ? "good" : job.score >= 0.6 ? "ok" : "bad"}`}
                              title="Renderability — easy sets render first"
                            >
                              {Math.round(job.score * 100)}%
                            </span>
                          )}
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
                onClick={retriageJobs}
                disabled={!queue.some(j => j.status === 'pending')}
                title="Recompute renderability scores with a fresh plugin inventory"
              >
                Re-triage
              </button>
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
                style={{ marginRight: "10px" }}
                onClick={clearCompleted}
                disabled={!queue.some(j => j.status === 'completed' || j.status === 'failed')}
              >
                Clear Done / Failed
              </button>
              <button
                className="open-btn ghost"
                style={{ marginRight: "auto" }}
                onClick={clearAllJobs}
                disabled={!queue.some(j => j.status !== 'processing')}
                title="Empty the queue (a render in progress is kept until it finishes)"
              >
                Clear All
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

      {showListsModal && (
        <div className="modal-overlay" onClick={() => setShowListsModal(false)}>
          <div className="modal-content" onClick={(e) => e.stopPropagation()}>
            <div className="modal-header">
              <h2 className="modal-title">Manage Lists</h2>
              <button
                className="modal-close-btn"
                onClick={() => setShowListsModal(false)}
                title="Close"
              >
                ×
              </button>
            </div>

            <div className="watch-section">
              {lists.length === 0 ? (
                <div className="empty-small">No lists yet.</div>
              ) : (
                <ul className="watch-list">
                  {lists.map(([id, , count]) => {
                    const draft = listDrafts[id] ?? "";
                    const changed = draft.trim() !== "" && draft !== lists.find((l) => l[0] === id)?.[1];
                    return (
                      <li key={id} className="watch-item" style={{ gap: 8 }}>
                        <input
                          value={draft}
                          onChange={(e) => setListDrafts((d) => ({ ...d, [id]: e.target.value }))}
                          onKeyDown={(e) => { if (e.key === "Enter") saveListName(id); }}
                          style={{ flex: 1, minWidth: 0 }}
                        />
                        <span style={{ opacity: 0.5, fontSize: "0.85em", whiteSpace: "nowrap" }}>
                          {count} set{count === 1 ? "" : "s"}
                        </span>
                        <button
                          className="open-btn"
                          disabled={!changed}
                          onClick={() => saveListName(id)}
                          title="Rename this list"
                        >
                          Rename
                        </button>
                        {confirmDeleteList === id ? (
                          <>
                            <button
                              className="open-btn danger-btn"
                              onClick={() => deleteListConfirmed(id)}
                            >
                              Confirm
                            </button>
                            <button className="open-btn" onClick={() => setConfirmDeleteList(null)}>
                              Cancel
                            </button>
                          </>
                        ) : (
                          <button
                            className="watch-remove-btn"
                            onClick={() => setConfirmDeleteList(id)}
                            title="Delete this list (sets and files are untouched)"
                          >
                            ×
                          </button>
                        )}
                      </li>
                    );
                  })}
                </ul>
              )}
              <div style={{ display: "flex", gap: 6, marginTop: 12 }}>
                <input
                  placeholder="new list…"
                  value={modalNewList}
                  onChange={(e) => setModalNewList(e.target.value)}
                  onKeyDown={(e) => { if (e.key === "Enter") createListInModal(); }}
                  style={{ flex: 1, minWidth: 0 }}
                />
                <button className="open-btn" onClick={createListInModal}>Create list</button>
              </div>
              <p className="hint" style={{ marginTop: 10 }}>
                Deleting a list only removes the grouping — your sets and audio files are never touched.
              </p>
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
                <div style={{ display: "flex", gap: "8px" }}>
                  {(selectedSuggestions.length > 0 || linkProgress) && (
                    <button
                      className="open-btn"
                      style={{ padding: "4px 8px", fontSize: "11px" }}
                      onClick={linkSelectedSuggestions}
                      disabled={!!linkProgress}
                    >
                      {linkProgress
                        ? `Linking ${linkProgress[0]}/${linkProgress[1]}…`
                        : `Link Selected (${selectedSuggestions.length})`}
                    </button>
                  )}
                  <button
                    className="open-btn ghost"
                    style={{ padding: "4px 8px", fontSize: "11px" }}
                    onClick={refreshSuggestions}
                    disabled={loadingSuggestions}
                  >
                    {loadingSuggestions ? "Scanning..." : "Scan/Refresh ↻"}
                  </button>
                </div>
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
                        <th style={{ width: "30px", textAlign: "center" }}>
                          <input
                            type="checkbox"
                            checked={(() => {
                              const all = selectableSuggestionKeys();
                              return all.length > 0 && all.every((k) => selectedSuggestions.includes(k));
                            })()}
                            onChange={toggleSelectAllSuggestions}
                            title="Select the best match for every set that has no preview yet"
                          />
                        </th>
                        <th colSpan={2}>Set / Matching Bounces</th>
                        <th className="job-actions" style={{ width: "160px" }}>Actions</th>
                      </tr>
                    </thead>
                    <tbody>
                      {(() => {
                        // Group candidates per set (suggestions arrive sorted
                        // by confidence, so groups appear best-first and the
                        // first item in each group is its best match).
                        type Group = {
                          set_id: number;
                          set_name: string;
                          project_name: string;
                          has_preview: boolean;
                          current_preview: string | null;
                          items: Suggestion[];
                        };
                        const groups: Group[] = [];
                        const byId = new Map<number, Group>();
                        for (const s of suggestions) {
                          let g = byId.get(s.set_id);
                          if (!g) {
                            g = {
                              set_id: s.set_id,
                              set_name: s.set_name,
                              project_name: s.project_name,
                              has_preview: s.has_preview,
                              current_preview: s.current_preview,
                              items: [],
                            };
                            byId.set(s.set_id, g);
                            groups.push(g);
                          }
                          g.items.push(s);
                        }
                        // Sets still missing a preview come first.
                        groups.sort((a, b) => Number(a.has_preview) - Number(b.has_preview));

                        return groups.flatMap((g) => [
                          <tr key={`g${g.set_id}`} className="suggestion-group">
                            <td />
                            <td colSpan={3}>
                              <span className="queue-job-title">{g.set_name.replace(/\.als$/, "")}</span>
                              <span className="queue-job-path" style={{ marginLeft: "8px" }}>
                                {g.project_name}
                              </span>
                              {g.has_preview && (
                                <span className="suggestion-badge" title="This project already has a preview — linking replaces it as primary">
                                  has preview
                                </span>
                              )}
                              {g.items.length > 1 && (
                                <span className="queue-job-path" style={{ marginLeft: "8px" }}>
                                  {g.items.length} matches
                                </span>
                              )}
                              {g.current_preview && (
                                <span className="queue-job-path" style={{ marginLeft: "8px" }}>
                                  current: {fileName(g.current_preview)}
                                  <button
                                    className="remove-job-btn"
                                    style={{ color: "var(--accent)", marginLeft: "8px" }}
                                    onClick={() =>
                                      setTrack({
                                        setId: g.set_id,
                                        title: g.set_name.replace(/\.als$/, ""),
                                        subtitle: `${g.project_name} · current preview`,
                                        src: convertFileSrc(g.current_preview!),
                                        peaks: [],
                                        duration: null,
                                      })
                                    }
                                    title="Play the current preview"
                                  >
                                    ▶
                                  </button>
                                  <button
                                    className="remove-job-btn"
                                    style={{ color: "#e38585", marginLeft: "6px" }}
                                    onClick={async () => {
                                      try {
                                        await invoke("remove_preview", { set_id: g.set_id });
                                        refreshSuggestions();
                                        runSearch();
                                        refreshStats();
                                      } catch (e) {
                                        setError(String(e));
                                      }
                                    }}
                                    title="Unlink the current preview (the file stays on disk)"
                                  >
                                    × Unlink
                                  </button>
                                </span>
                              )}
                            </td>
                          </tr>,
                          ...g.items.map((s) => {
                            const key = `${s.set_id}:${s.audio_path}`;
                            return (
                              <tr key={key}>
                                <td style={{ textAlign: "center", verticalAlign: "middle" }}>
                                  <input
                                    type="checkbox"
                                    checked={selectedSuggestions.includes(key)}
                                    onChange={() => toggleSelectSuggestion(key)}
                                  />
                                </td>
                                <td colSpan={2}>
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
                            );
                          }),
                        ]);
                      })()}
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

      {listMenuSet !== null && (
        <>
          <div
            onClick={() => setListMenuSet(null)}
            style={{ position: "fixed", inset: 0, zIndex: 1000 }}
          />
          <div
            className="list-menu"
            style={{
              position: "fixed", left: menuPos.x, top: menuPos.y, zIndex: 1001,
              width: 240, textAlign: "left", padding: 10,
              background: "var(--panel, #1e1e1e)", border: "1px solid var(--border, #444)",
              borderRadius: 8, boxShadow: "0 8px 28px rgba(0,0,0,0.5)",
              maxHeight: "min(60vh, 360px)", overflowY: "auto",
            }}
          >
            <div style={{ fontSize: "0.8em", opacity: 0.6, marginBottom: 6 }}>Add to lists</div>
            {lists.length === 0 && (
              <div style={{ fontSize: "0.85em", opacity: 0.6, padding: "4px 0" }}>
                No lists yet — create one below.
              </div>
            )}
            {lists.map(([id, name, count]) => {
              const member = menuMemberships.has(id);
              return (
                <label key={id} style={{ display: "flex", alignItems: "center", gap: 8, padding: "4px 0", cursor: "pointer" }}>
                  <input
                    type="checkbox"
                    checked={member}
                    onChange={() => listMenuSet !== null && toggleMembership(id, listMenuSet, member)}
                  />
                  <span style={{ flex: 1 }}>{name}</span>
                  <span style={{ opacity: 0.45, fontSize: "0.8em" }}>{count}</span>
                </label>
              );
            })}
            <div style={{ display: "flex", gap: 4, marginTop: 8, borderTop: "1px solid var(--border, #444)", paddingTop: 8 }}>
              <input
                placeholder="new list…"
                value={newListName}
                autoFocus
                onChange={(e) => setNewListName(e.target.value)}
                onKeyDown={(e) => { if (e.key === "Enter" && listMenuSet !== null) createListAndAdd(listMenuSet); }}
                style={{ flex: 1, minWidth: 0 }}
              />
              <button
                className="open-btn"
                onClick={() => listMenuSet !== null && createListAndAdd(listMenuSet)}
              >
                Create
              </button>
            </div>
          </div>
        </>
      )}

    </div>
  );
}
