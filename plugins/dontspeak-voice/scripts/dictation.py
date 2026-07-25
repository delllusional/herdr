#!/usr/bin/env python3
"""Read-only mirror of the native Don't Speak dictation overlay."""

import json
import subprocess
import sys
import time


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


def draw(text):
    # No controls, state labels, target name, or instructions: this is the same
    # presentation layer as the native overlay, not a second input surface.
    print("\033[2J\033[H", end="")
    print(text, end="", flush=True)


def main():
    seq = None
    was_visible = False
    while True:
        snapshot = status(seq)
        if not snapshot:
            time.sleep(0.25)
            continue
        seq = snapshot.get("seq", seq)
        dictation = snapshot.get("dictation", {})
        state = dictation.get("state", "hidden")
        if state == "hidden":
            # This process is launched only for an active dictation popup. If
            # the turn ended before launch, exit instead of indefinitely
            # renewing the external-UI lease and hiding the native fallback.
            return
        was_visible = True
        draw(dictation.get("text", ""))


if __name__ == "__main__":
    main()
