#!/usr/bin/env python3
import os
import sys
import time
import argparse
import subprocess

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
    escaped_live_app = live_app.replace('"', '\\"')
    escaped_set_stem = set_stem.replace('"', '\\"')

    script = f'''
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
        set windowLoaded to false
        repeat 60 times -- Wait up to 30 seconds
            -- Handle potential blocking dialogs (OK / Newer version / Missing media)
            try
                if exists (window 1 whose subrole is "AXDialog") then
                    keystroke return
                    delay 1
                end if
            end try
            
            try
                if exists (window 1 whose name contains "{escaped_set_stem}") then
                    set windowLoaded to true
                    exit repeat
                end if
            end try
            delay 0.5
        end repeat
        
        -- Even if windowLoaded is false, we try to proceed to avoid breaking on minor title mismatch
        delay 2
        set frontmost to true
        
        -- 2. Select All in arrangement/active view (Cmd + A)
        keystroke "a" using {{command down}}
        delay 1
        
        -- 3. Trigger Export Menu (Shift + Cmd + R)
        keystroke "r" using {{command down, shift down}}
        delay 3
        
        -- Press Enter to accept the export settings (OK button)
        keystroke return
        delay 3
        
        -- 4. Open "Go to Folder" sheet (Cmd + Shift + G)
        keystroke "g" using {{command down, shift down}}
        delay 1.5
        
        -- Type the absolute output directory path and press enter
        keystroke "{escaped_output_dir}"
        delay 1
        keystroke return
        delay 1.5
        
        -- 5. Type the filename and press enter
        keystroke "{escaped_output_filename}"
        delay 1
        keystroke return
        delay 2

        -- 6. If a "file already exists — Replace?" confirmation appeared
        -- (e.g. the pre-delete failed on an iCloud-locked file), confirm it
        -- so the queue never wedges. Guarded checks only — a blind extra
        -- Return could toggle playback in Live.
        try
            if exists (button "Replace" of sheet 1 of sheet 1 of window 1) then
                click button "Replace" of sheet 1 of sheet 1 of window 1
                delay 1
            end if
        end try
        try
            if exists (button "Replace" of sheet 1 of window 1) then
                click button "Replace" of sheet 1 of window 1
                delay 1
            end if
        end try
        try
            if exists (window 1 whose subrole is "AXDialog") then
                keystroke return
                delay 1
            end if
        end try

    end tell
end tell
'''
    return script

def main():
    parser = argparse.ArgumentParser(description="Automate Ableton Live audio export via UI scripting.")
    parser.add_argument("--set-path", required=True, help="Absolute path to the .als set.")
    parser.add_argument("--output-dir", required=True, help="Absolute path to the output directory.")
    parser.add_argument("--live-app", default="/Applications/Ableton Live 11 Suite.app", help="Path to Ableton Live application.")
    args = parser.parse_args()

    if not os.path.exists(args.set_path):
        print(f"Error: Set path '{args.set_path}' does not exist.", file=sys.stderr)
        sys.exit(1)

    os.makedirs(args.output_dir, exist_ok=True)

    set_basename = os.path.basename(args.set_path)
    set_stem = os.path.splitext(set_basename)[0]
    output_filename = f"{set_stem}.wav"

    output_file = os.path.join(args.output_dir, output_filename)
    if os.path.exists(output_file):
        try:
            os.remove(output_file)
            print(f"Removed pre-existing output file to avoid overwrite dialog: {output_file}")
        except Exception as e:
            print(f"Warning: Could not remove existing file: {e}")

    print(f"Starting export for: {set_basename}")
    print(f"Targeting Live app: {args.live_app}")
    print(f"Output directory:  {args.output_dir}")

    # Generate the AppleScript
    script = generate_applescript(
        set_path=args.set_path,
        output_dir=args.output_dir,
        output_filename=output_filename,
        live_app=args.live_app,
        set_stem=set_stem
    )

    # Execute
    code, stdout, stderr = run_applescript(script)

    if code != 0:
        print(f"AppleScript failed with exit code: {code}", file=sys.stderr)
        if stdout:
            print(f"Stdout:\n{stdout}", file=sys.stderr)
        if stderr:
            print(f"Stderr:\n{stderr}", file=sys.stderr)
        sys.exit(1)

    # Wait for the output wav file to be fully written
    print(f"Waiting for render to complete. Target file: {output_file}")
    
    start_time = time.time()
    timeout = 600  # 10 minutes timeout
    created = False
    
    while time.time() - start_time < timeout:
        if os.path.exists(output_file):
            created = True
            break
        # If Ableton Live is closed during wait, abort immediately
        if not is_live_running():
            print("Error: Ableton Live was closed or crashed during rendering.", file=sys.stderr)
            sys.exit(1)
        time.sleep(1)
        
    if not created:
        print(f"Error: Render timed out after {timeout} seconds (file was never created).", file=sys.stderr)
        sys.exit(1)
        
    # Wait for the file size to stop changing (ensure it is fully written)
    print("File created. Waiting for export to finalize...")
    last_size = -1
    stable_ticks = 0
    
    while time.time() - start_time < timeout:
        # If Ableton Live is closed during finalize, abort
        if not is_live_running():
            print("Error: Ableton Live was closed or crashed during finalizing.", file=sys.stderr)
            sys.exit(1)
        try:
            current_size = os.path.getsize(output_file)
            if current_size > 0 and current_size == last_size:
                stable_ticks += 1
                if stable_ticks >= 3:  # Must be stable for 1.5 seconds
                    break
            else:
                last_size = current_size
                stable_ticks = 0
        except Exception:
            # File might be temporarily locked or inaccessible
            stable_ticks = 0
        time.sleep(0.5)
        
    print(f"Render completed successfully! File size: {os.path.getsize(output_file)} bytes")

    # Close document in Ableton Live
    print("Closing set in Ableton Live...")
    close_script = f'''
tell application "System Events"
    tell process "Live"
        set frontmost to true
        keystroke "w" using {{command down}}
        delay 1.5
        try
            if exists (window 1 whose name contains "Save Changes" or name contains "Save") then
                keystroke "d" using {{command down}}
                delay 1
            end if
        end try
    end tell
end tell
'''
    run_applescript(close_script)
    sys.exit(0)

if __name__ == "__main__":
    main()
