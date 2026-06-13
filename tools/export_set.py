#!/usr/bin/env python3
import os
import sys
import time
import argparse
import subprocess
from datetime import datetime

def log(msg, is_error=False):
    timestamp = datetime.now().strftime("%H:%M:%S")
    stream = sys.stderr if is_error else sys.stdout
    print(f"[{timestamp}] {msg}", file=stream)
    stream.flush()

def run_applescript(script_content):
    """Run AppleScript content using osascript."""
    process = subprocess.Popen(
        ['osascript', '-'],
        stdin=subprocess.PIPE,
        stdout=subprocess.PIPE,
        stderr=subprocess.PIPE,
        text=True
    )
    stdout, stderr = process.communicate(script_content)
    # osascript `log` statements arrive on stderr — ALWAYS surface them
    # (narration was previously swallowed: `do shell script "echo"` captures
    # its own output and prints nothing).
    for line in (stderr or "").splitlines():
        if line.strip():
            log(f"[AS] {line.strip()}")
    return process.returncode, stdout, stderr

def is_live_running():
    """Check if the Ableton Live process is running on macOS."""
    try:
        res = subprocess.run(["pgrep", "-x", "Live"], capture_output=True)
        return res.returncode == 0
    except Exception:
        # Fallback to True to avoid false negatives in case of command failure
        return True

def generate_applescript(set_path, output_dir, output_filename, live_app, set_stem):
    # Escape double quotes for AppleScript
    escaped_set_path = set_path.replace('"', '\\"')
    escaped_output_dir = output_dir.replace('"', '\\"')
    escaped_output_filename = output_filename.replace('"', '\\"')
    # Fallback path: absolute path typed straight into the name field
    # (NSSavePanel resolves it) when the Go-To panel won't open.
    escaped_full_output_path = f"{escaped_output_dir}/{escaped_output_filename}"
    escaped_live_app = live_app.replace('"', '\\"')
    escaped_set_stem = set_stem.replace('"', '\\"')

    script = f'''
log "[AppleScript] Starting automation..."
tell application "System Events"
    -- Check if Ableton Live is running, if not launch it
    set isRunning to false
    if exists (process "Live") then
        set isRunning to true
    end if
end tell

-- Launch specific Live app with the set
do shell script "open -a \\"{escaped_live_app}\\" \\"{escaped_set_path}\\""

delay 3 -- Wait for launch command to register

tell application "System Events"
    tell process "Live"
        set frontmost to true
        
        -- 1. Wait for the set to load (wait for main window containing the set name to exist)
        log "[AppleScript] Waiting for set to load..."
        set windowLoaded to false
        repeat 60 times -- Wait up to 30 seconds
            -- Handle potential blocking dialogs (OK / Newer version / Missing media)
            try
                if exists (window 1 whose subrole is "AXDialog") then
                    log "[AppleScript] Dismissing dialog..."
                    keystroke return
                    delay 1
                end if
            end try
            
            try
                if exists (window 1 whose name contains "{escaped_set_stem}") then
                    set windowLoaded to true
                    log "[AppleScript] Set loaded."
                    exit repeat
                end if
            end try
            delay 0.5
        end repeat
        
        -- Even if windowLoaded is false, we try to proceed to avoid breaking on minor title mismatch
        delay 2
        set frontmost to true
        
        -- 2. Select All in arrangement/active view (Cmd + A)
        log "[AppleScript] Selecting all..."
        keystroke "a" using {{command down}}
        delay 1
        
        -- 3. Trigger Export Menu (Shift + Cmd + R)
        log "[AppleScript] Triggering Export menu..."
        keystroke "r" using {{command down, shift down}}
        delay 3
        
        -- Press Enter to accept the export settings (OK button)
        log "[AppleScript] Accepting export settings..."
        keystroke return

        -- 4. WAIT for the save panel — WHEREVER it lives. Live is non-native:
        -- the NSSavePanel may be a sheet of window 1 OR a separate dialog
        -- window. Detect both (assuming sheet-only made us stare past a
        -- visible dialog for 60s — stall 2026-06-13).
        log "[AppleScript] Waiting for save panel (sheet or dialog window)..."
        set savePanel to missing value
        set sheetDeadline to (current date) + 60
        repeat while savePanel is missing value
            try
                if exists (sheet 1 of window 1) then
                    set savePanel to sheet 1 of window 1
                    log "[AppleScript] Found save panel: sheet of window 1"
                end if
            end try
            if savePanel is missing value then
                try
                    repeat with w in windows
                        if exists (text field 1 of w) then
                            if subrole of w is "AXDialog" or subrole of w is "AXSystemDialog" then
                                set savePanel to w
                                log "[AppleScript] Found save panel: dialog window"
                                exit repeat
                            end if
                        end if
                    end repeat
                end try
            end if
            if savePanel is missing value then
                if (current date) > sheetDeadline then
                    log "[AppleScript] ERROR: no save panel found (sheet or dialog)"
                    error "Save panel never appeared as sheet or dialog window"
                end if
                delay 0.5
            end if
        end repeat
        -- Panel exists before it accepts input; let it settle.
        delay 2.5

        -- 4b. Navigate via "Go to Folder" (Cmd+Shift+G), then set just the
        -- FILENAME. (Setting the full path into the name field via AX does
        -- NOT get parsed as a path — it becomes a literal filename dumped in
        -- the default folder: user found the default dir littered with files
        -- named after the whole path, 2026-06-13. NSSavePanel only resolves
        -- a path through the Go-To field.)
        set frontmost to true
        log "[AppleScript] Opening Go to Folder (Cmd+Shift+G)..."
        keystroke "g" using {{command down, shift down}}

        -- Wait for the Go-To combo field (a sheet on the save panel).
        set gotoField to missing value
        set gotoDeadline to (current date) + 8
        repeat while gotoField is missing value
            try
                set gotoField to text field 1 of sheet 1 of savePanel
            end try
            if gotoField is missing value then
                try
                    set gotoField to combo box 1 of sheet 1 of savePanel
                end try
            end if
            if gotoField is missing value then
                if (current date) > gotoDeadline then exit repeat
                delay 0.4
            end if
        end repeat

        if gotoField is not missing value then
            log "[AppleScript] Go-To open; setting directory..."
            try
                set value of gotoField to "{escaped_output_dir}"
            on error
                keystroke "{escaped_output_dir}"
            end try
            delay 0.6
            keystroke return
            delay 1.5
        else
            -- Go-To didn't open: type the dir into it blind (older fallback).
            log "[AppleScript] Go-To field not found; typing directory blind..."
            keystroke "{escaped_output_dir}"
            delay 0.6
            keystroke return
            delay 1.5
        end if

        -- Now set ONLY the filename in the panel's name field.
        log "[AppleScript] Setting filename in name field..."
        try
            set value of text field 1 of savePanel to "{escaped_output_filename}"
            delay 0.5
        on error
            try
                click text field 1 of savePanel
            end try
            delay 0.4
            keystroke "a" using {{command down}}
            delay 0.3
            keystroke "{escaped_output_filename}"
            delay 1
        end try
        log "[AppleScript] Confirming save..."
        keystroke return
        delay 2

        -- 6. If a "file already exists — Replace?" confirmation appeared.
        -- Check the sheet attached to the FOUND panel first: when the save
        -- panel is a separate dialog window, the Replace sheet hangs off it,
        -- not off window 1 (binge2 failure 2026-06-13: panel stayed open
        -- because Replace was never clicked).
        log "[AppleScript] Checking for Replace dialog..."
        try
            if exists (button "Replace" of sheet 1 of savePanel) then
                log "[AppleScript] Clicking Replace (sheet of save panel)..."
                click button "Replace" of sheet 1 of savePanel
                delay 1
            end if
        end try
        try
            if exists (button "Replace" of sheet 1 of sheet 1 of window 1) then
                log "[AppleScript] Clicking Replace (sheet 2)..."
                click button "Replace" of sheet 1 of sheet 1 of window 1
                delay 1
            end if
        end try
        try
            if exists (button "Replace" of sheet 1 of window 1) then
                log "[AppleScript] Clicking Replace (sheet 1)..."
                click button "Replace" of sheet 1 of window 1
                delay 1
            end if
        end try
        try
            if exists (window 1 whose subrole is "AXDialog") then
                log "[AppleScript] Dismissing final dialog..."
                keystroke return
                delay 1
            end if
        end try

        -- 7. CONFIRM the export actually started: the save sheet must be
        -- gone. If it's still open, path/filename entry failed - error NOW
        -- with a clear message instead of idling until the worker's
        -- 12-minute timeout (the "got lost at the save dialog" stall).
        log "[AppleScript] Verifying save sheet dismissed..."
        -- Dismissal test: does the panel still have its NAME FIELD?
        -- (`savePanel` is an index-based reference: once the save panel
        -- closes, Live's render-PROGRESS dialog can occupy the same window
        -- index, so `exists savePanel` stays true and we false-failed
        -- WHILE THE RENDER WAS RUNNING — 2026-06-13. The progress dialog
        -- has no text field; the save panel does.)
        set goneDeadline to (current date) + 20
        set panelGone to false
        repeat until panelGone
            try
                if not (exists text field 1 of savePanel) then set panelGone to true
            on error
                set panelGone to true -- stale reference = panel destroyed = gone
            end try
            if not panelGone then
                if (current date) > goneDeadline then
                    log "[AppleScript] ERROR: save panel still open"
                    error "Save panel still open after path entry - confirm likely failed"
                end if
                delay 0.5
            end if
        end repeat
        log "[AppleScript] Save sheet dismissed - export started."
        log "[AppleScript] Export command sequence finished."
    end tell
end tell
'''
    return script

def main():
    parser = argparse.ArgumentParser(description="Automate Ableton Live audio export via UI scripting.")
    parser.add_argument("--set-path", required=True, help="Absolute path to the .als set.")
    parser.add_argument("--output-dir", required=True, help="Absolute path to the output directory.")
    parser.add_argument("--output-name", help="Optional output filename (without extension). Defaults to set name.")
    parser.add_argument("--live-app", default="/Applications/Ableton Live 11 Suite.app", help="Path to Ableton Live application.")
    args = parser.parse_args()

    if not os.path.exists(args.set_path):
        log(f"Error: Set path '{args.set_path}' does not exist.", is_error=True)
        sys.exit(1)

    os.makedirs(args.output_dir, exist_ok=True)

    set_basename = os.path.basename(args.set_path)
    set_stem = os.path.splitext(set_basename)[0]
    
    # Use output_name if provided, otherwise fallback to set_stem
    effective_stem = args.output_name if args.output_name else set_stem
    output_filename = f"{effective_stem}.wav"

    output_file = os.path.join(args.output_dir, output_filename)
    if os.path.exists(output_file):
        try:
            os.remove(output_file)
            log(f"Removed pre-existing output file: {output_file}")
        except Exception as e:
            log(f"Warning: Could not remove existing file: {e}", is_error=True)

    log(f"Starting export for: {set_basename}")
    log(f"Targeting Live app: {args.live_app}")
    log(f"Output directory:  {args.output_dir}")

    # --- Pre-flight Safety Check ---
    log("Running pre-flight safety check...")
    
    # Check if output directory is the same as the project root (too dangerous)
    project_root = os.path.dirname(args.set_path)
    if os.path.abspath(args.output_dir) == os.path.abspath(project_root):
        log("Safety Warning: Output directory is the project root. This is dangerous.", is_error=True)
        # Proceed with caution but log it clearly
    
    # Check if target file is the .als file
    output_filename = f"{effective_stem}.wav"
    if output_filename == set_basename:
        log("Safety Error: Output filename conflicts with project filename.", is_error=True)
        sys.exit(1)
        
    log("Safety check passed. Proceeding in 5 seconds (interrupt now if something looks wrong)...")
    time.sleep(5)
    # --- End Pre-flight ---

    # Generate the AppleScript
    script = generate_applescript(
        set_path=args.set_path,
        output_dir=args.output_dir,
        output_filename=output_filename,
        live_app=args.live_app,
        set_stem=set_stem
    )

    # Execute
    log("Executing AppleScript for UI automation...")
    code, stdout, stderr = run_applescript(script)

    if code != 0:
        # stderr (the [AS] narration incl. the error) was already surfaced
        # line-by-line by run_applescript — don't dump it twice.
        log(f"AppleScript failed with exit code: {code}", is_error=True)
        if stdout:
            log(f"AppleScript Stdout:\n{stdout}", is_error=True)
        sys.exit(1)

    # Wait for the output wav file to be fully written
    log(f"Waiting for render to complete. Target file: {output_file}")
    
    start_time = time.time()
    timeout = 600  # 10 minutes timeout
    created = False
    
    while time.time() - start_time < timeout:
        if os.path.exists(output_file):
            created = True
            log(f"File detected after {int(time.time() - start_time)}s.")
            break
        # If Ableton Live is closed during wait, abort immediately
        if not is_live_running():
            log("Error: Ableton Live was closed or crashed during rendering.", is_error=True)
            sys.exit(1)
        time.sleep(1)
        
    if not created:
        log(f"Error: Render timed out after {timeout} seconds (file was never created).", is_error=True)
        sys.exit(1)
        
    # Wait for the file size to stop changing (ensure it is fully written)
    log("File created. Waiting for export to finalize (size stability)...")
    last_size = -1
    stable_ticks = 0
    
    while time.time() - start_time < timeout:
        # If Ableton Live is closed during finalize, abort
        if not is_live_running():
            log("Error: Ableton Live was closed or crashed during finalizing.", is_error=True)
            sys.exit(1)
        try:
            current_size = os.path.getsize(output_file)
            if current_size > 0 and current_size == last_size:
                stable_ticks += 1
                if stable_ticks >= 3:  # Must be stable for 1.5 seconds
                    break
            else:
                if current_size != last_size:
                    log(f"  Current size: {current_size} bytes...")
                last_size = current_size
                stable_ticks = 0
        except Exception as e:
            log(f"  (Wait) File inaccessible: {e}")
            stable_ticks = 0
        time.sleep(0.5)
        
    final_size = os.path.getsize(output_file)
    log(f"Render completed successfully! Final size: {final_size} bytes")

    # Close document in Ableton Live
    log("Closing set in Ableton Live...")
    close_script = f'''
tell application "System Events"
    tell process "Live"
        set frontmost to true
        keystroke "w" using {{command down}}
        delay 1.5
        try
            -- In most macOS dialogs, 'd' is 'Don't Save', but 'Esc' is safer.
            -- Try to press Esc to dismiss Save Changes
            key code 53 
            delay 0.5
        end try
    end tell
end tell
'''
    code, stdout, stderr = run_applescript(close_script)
    if code != 0:
        log(f"Warning: Close script failed (code {code}), but render was successful.", is_error=True)

    sys.exit(0)

if __name__ == "__main__":
    main()
