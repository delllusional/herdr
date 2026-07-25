#!/usr/bin/env python3
"""Read-only mirror of the native Don't Speak dictation overlay."""

import json
import subprocess
import sys


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
            continue
        seq = snapshot.get("seq", seq)
        dictation = snapshot.get("dictation", {})
        state = dictation.get("state", "hidden")
        if state == "hidden":
            if was_visible:
                return
            continue
        was_visible = True
        draw(dictation.get("text", ""))


if __name__ == "__main__":
    main()
