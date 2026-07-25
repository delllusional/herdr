#!/usr/bin/env python3
"""Small terminal UI for a Don't Speak dictation preview in a Herdr popup."""

import json
import os
import subprocess
import sys
import time


def target_pane():
    context = json.loads(os.environ.get("HERDR_PLUGIN_CONTEXT_JSON", "{}"))
    return context.get("focused_pane_id")


def status(since=None):
    args = ["dontspeak", "status", "--json", "--ui-receiver", "herdr.voice"]
    if since is not None:
        args += ["--since", str(since), "--timeout-ms", "2000"]
    result = subprocess.run(args, capture_output=True, text=True, check=False)
    if result.returncode:
        return None
    try:
        return json.loads(result.stdout)
    except json.JSONDecodeError:
        return None


def draw(target, state, text):
    print("\033[2J\033[H", end="")
    print("DON'T SPEAK DICTATION")
    print(f"Target: {target or 'no agent pane'}")
    print(f"State: {state}")
    print()
    print(text or "Listening for speech...")
    print()
    print("Enter sends the displayed text. Esc closes. Keep this popup open while dictating.")
    sys.stdout.flush()


def main():
    target = target_pane()
    seq = None
    latest = ""
    while True:
        snapshot = status(seq)
        if snapshot:
            seq = snapshot.get("seq", seq)
            dictation = snapshot.get("dictation", {})
            latest = dictation.get("text", latest)
            draw(target, dictation.get("state", "unavailable"), latest)
        if sys.stdin in select_ready(0.15):
            key = sys.stdin.read(1)
            if key in ("\x1b", "q"):
                return
            if key in ("\r", "\n") and target and latest.strip():
                subprocess.run(["herdr", "pane", "run", target, latest], check=False)
                return


def select_ready(timeout):
    import select
    ready, _, _ = select.select([sys.stdin], [], [], timeout)
    return ready


if __name__ == "__main__":
    main()
