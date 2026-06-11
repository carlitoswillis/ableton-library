#!/usr/bin/env python3
"""Reference implementation / executable spec for als-core.

Parses Ableton .als files (gzipped XML) into SetSnapshot JSON.
The Rust parser in crates/als-core must produce equivalent output;
this script is the test oracle (diff Rust output against this).

Usage: python3 tools/reference_extract.py <library-root> [--pretty]
JSON (array of ProjectSnapshot) on stdout, human summary on stderr.
"""
import gzip
import hashlib
import json
import os
import sys
import xml.etree.ElementTree as ET
from datetime import datetime, timezone

TRACK_KINDS = {"MidiTrack": "midi", "AudioTrack": "audio",
               "ReturnTrack": "return", "GroupTrack": "group"}
MASTER_TAGS = {"MasterTrack", "MainTrack"}  # MainTrack = Live 12+ name
PLUGIN_INFO = {"AuPluginInfo": "au", "VstPluginInfo": "vst", "Vst3PluginInfo": "vst3"}
PLUGIN_WRAPPERS = {"PluginDevice", "AuPluginDevice", "VstPluginDevice", "Vst3PluginDevice"}
AUDIO_EXTS = (".wav", ".aif", ".aiff", ".mp3", ".flac", ".m4a", ".ogg")
# Subtrees that are bulk (automation points, MIDI notes, plugin binary state)
# and contain nothing we extract. MUST stay in sync with als-core SKIP_SUBTREES.
SKIP_SUBTREES = {"AutomationEnvelopes", "KeyTracks", "Notes", "Events",
                 "ParameterSettings", "ProcessorState", "Buffer", "Data",
                 "AutomationTarget", "ModulationTarget"}


def sha256_file(path):
    h = hashlib.sha256()
    with open(path, "rb") as f:
        for chunk in iter(lambda: f.read(1 << 20), b""):
            h.update(chunk)
    return h.hexdigest()


def track_ctx_label(cur_track):
    return cur_track if isinstance(cur_track, int) else cur_track  # int | "master" | None


def parse_set(als_path, project_dir):
    snap = {
        "als_path": os.path.abspath(als_path),
        "file_size": os.path.getsize(als_path),
        "mtime": datetime.fromtimestamp(os.path.getmtime(als_path),
                                        tz=timezone.utc).isoformat(timespec="seconds"),
        "content_hash": sha256_file(als_path),
        "live_version": None,
        "schema_version": None,
        "tempo": None,
        "time_signature": None,
        "tracks": [],
        "devices": [],
        "samples": [],
        "locators": [],
        "warnings": [],
    }
    project_name = os.path.basename(os.path.abspath(project_dir))

    stack = []            # open-element tag stack (skipped subtrees never pushed)
    skipping = None       # tag we are skipping, with depth counter
    skip_depth = 0
    cur_track = None      # index into snap["tracks"] | "master" | None
    cur_plugin = None     # pending plugin device
    cur_locator = None
    cur_sig = None        # pending (numerator, denominator)
    tempo_cands = []      # (value, seen_in_master_context)
    sig_cands = []
    samples_seen = set()

    with gzip.open(als_path, "rb") as f:
        for event, elem in ET.iterparse(f, events=("start", "end")):
            tag = elem.tag

            if skipping is not None:
                if event == "start":
                    skip_depth += 1
                else:
                    skip_depth -= 1
                    if skip_depth == 0:
                        skipping = None
                    elem.clear()
                continue

            if event == "start":
                if tag in SKIP_SUBTREES:
                    skipping = tag
                    skip_depth = 1
                    continue
                parent = stack[-1] if stack else None
                gparent = stack[-2] if len(stack) >= 2 else None
                val = elem.get("Value")

                if tag == "Ableton":
                    snap["live_version"] = elem.get("Creator")
                    snap["schema_version"] = elem.get("MinorVersion")
                elif tag in TRACK_KINDS and parent == "Tracks":
                    snap["tracks"].append(
                        {"kind": TRACK_KINDS[tag], "name": None, "color": None})
                    cur_track = len(snap["tracks"]) - 1
                elif tag in MASTER_TAGS and parent == "LiveSet":
                    cur_track = "master"
                elif (tag == "EffectiveName" and parent == "Name"
                      and gparent in TRACK_KINDS and isinstance(cur_track, int)
                      and snap["tracks"][cur_track]["name"] is None):
                    snap["tracks"][cur_track]["name"] = val
                elif (tag == "Color" and parent in TRACK_KINDS
                      and isinstance(cur_track, int)
                      and snap["tracks"][cur_track]["color"] is None and val):
                    snap["tracks"][cur_track]["color"] = int(val)
                elif tag == "Manual" and parent == "Tempo" and val:
                    tempo_cands.append((float(val), cur_track == "master"))
                elif tag == "Manual" and parent == "TimeSignature" and val:
                    # Encoded: value = 99 * log2(denominator) + (numerator - 1)
                    enc = int(val)
                    sig_cands.append(((enc % 99 + 1, 2 ** (enc // 99)),
                                      cur_track == "master"))
                elif tag == "RemoteableTimeSignature":
                    cur_sig = {}
                elif (cur_sig is not None and parent == "RemoteableTimeSignature"
                      and tag in ("Numerator", "Denominator") and val):
                    cur_sig[tag] = int(val)
                elif tag in PLUGIN_INFO:
                    cur_plugin = {"track": track_ctx_label(cur_track),
                                  "kind": PLUGIN_INFO[tag],
                                  "name": None, "manufacturer": None, "_tag": tag}
                elif cur_plugin is not None and parent == cur_plugin["_tag"]:
                    if tag in ("Name", "PlugName") and cur_plugin["name"] is None:
                        cur_plugin["name"] = val
                    elif tag == "Manufacturer":
                        cur_plugin["manufacturer"] = val
                elif (parent == "Devices" and elem.get("Id") is not None
                      and tag not in PLUGIN_WRAPPERS):
                    snap["devices"].append({"track": track_ctx_label(cur_track),
                                            "kind": "native", "name": tag,
                                            "manufacturer": "Ableton"})
                elif (tag == "Path" and parent == "FileRef" and val
                      and val.lower().endswith(AUDIO_EXTS)):
                    if val not in samples_seen:
                        samples_seen.add(val)
                        snap["samples"].append(
                            {"path": val,
                             "in_project": (os.sep + project_name + os.sep) in val
                                           or ("/" + project_name + "/") in val,
                             "exists": os.path.exists(val)})
                elif tag == "Locator" and parent == "Locators":
                    cur_locator = {"name": None, "time": None}
                elif cur_locator is not None and parent == "Locator":
                    if tag == "Name":
                        cur_locator["name"] = val
                    elif tag == "Time" and val:
                        cur_locator["time"] = float(val)

                stack.append(tag)

            else:  # end
                if stack:
                    stack.pop()
                parent = stack[-1] if stack else None
                if tag in TRACK_KINDS and parent == "Tracks":
                    cur_track = None
                elif tag in MASTER_TAGS and parent == "LiveSet":
                    cur_track = None
                elif cur_plugin is not None and tag == cur_plugin["_tag"]:
                    cur_plugin.pop("_tag")
                    snap["devices"].append(cur_plugin)
                    cur_plugin = None
                elif tag == "RemoteableTimeSignature" and cur_sig is not None:
                    if "Numerator" in cur_sig and "Denominator" in cur_sig:
                        sig_cands.append(((cur_sig["Numerator"], cur_sig["Denominator"]),
                                          cur_track == "master"))
                    cur_sig = None
                elif tag == "Locator" and cur_locator is not None:
                    snap["locators"].append(cur_locator)
                    cur_locator = None
                elem.clear()

    # Resolve tempo / time signature: prefer master-track context, else first seen.
    def resolve(cands, label):
        master = [v for v, in_master in cands if in_master]
        if master:
            return master[0]
        if cands:
            snap["warnings"].append(
                f"{label} not found in master-track context; using first occurrence")
            return cands[0][0]
        snap["warnings"].append(f"{label} not found")
        return None

    snap["tempo"] = resolve(tempo_cands, "tempo")
    sig = resolve(sig_cands, "time_signature")
    if sig:
        snap["time_signature"] = f"{sig[0]}/{sig[1]}"
    return snap


def lineage(project_dir):
    out = []
    backup = os.path.join(project_dir, "Backup")
    if os.path.isdir(backup):
        for name in sorted(os.listdir(backup)):
            if name.endswith(".als"):
                p = os.path.join(backup, name)
                out.append({"file": name,
                            "size": os.path.getsize(p),
                            "mtime": datetime.fromtimestamp(
                                os.path.getmtime(p),
                                tz=timezone.utc).isoformat(timespec="seconds")})
    return out


def find_projects(root):
    """A 'project' is any directory directly containing .als files (Backup excluded)."""
    projects = []
    for dirpath, dirnames, filenames in os.walk(root):
        dirnames[:] = [d for d in dirnames if d != "Backup"]
        als = sorted(f for f in filenames if f.endswith(".als"))
        if als:
            projects.append((dirpath, als))
    return sorted(projects)


def main():
    if len(sys.argv) < 2:
        sys.exit(__doc__)
    root = sys.argv[1]
    pretty = "--pretty" in sys.argv
    library = []
    for project_dir, als_files in find_projects(root):
        proj = {"folder_path": os.path.abspath(project_dir),
                "name": os.path.basename(os.path.abspath(project_dir)),
                "sets": [parse_set(os.path.join(project_dir, f), project_dir)
                         for f in als_files],
                "backups": lineage(project_dir)}
        library.append(proj)
        for s in proj["sets"]:
            kinds = {}
            for t in s["tracks"]:
                kinds[t["kind"]] = kinds.get(t["kind"], 0) + 1
            plugs = sum(1 for d in s["devices"] if d["kind"] != "native")
            print(f"  {os.path.basename(s['als_path']):40s} "
                  f"{s['tempo'] or '?':>6} bpm  {s['time_signature'] or '?':>5}  "
                  f"tracks={kinds}  devices={len(s['devices'])} ({plugs} plugin)  "
                  f"samples={len(s['samples'])}  warnings={len(s['warnings'])}",
                  file=sys.stderr)
        print(f"{proj['name']}: {len(proj['sets'])} set(s), "
              f"{len(proj['backups'])} backup(s)", file=sys.stderr)
    json.dump(library, sys.stdout, indent=2 if pretty else None)
    print(file=sys.stdout)


if __name__ == "__main__":
    main()
