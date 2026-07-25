#!/usr/bin/env python3
"""Publish Don't Speak voice-session state into Herdr pane metadata."""

import json
import os
import subprocess
import time

SOURCE = "plugin:dontspeak-voice"
TOKEN = "dontspeak_voice"
POPUP_PLUGIN = "dontspeak.voice"
POPUP_ENTRYPOINT = "dictation"


def command(*args):
    return subprocess.run(args, capture_output=True, text=True, check=False)


def token_for(row, muted):
    voice = row.get("voice") or "voice"
    queued = int(row.get("queued") or 0)
    if muted:
        return f"MUTE {voice}"
    if row.get("speaking"):
        return f"SPEAK {voice}"
    if row.get("blocked"):
        return f"WAIT {queued} {voice}"
    if queued:
        return f"Q{queued} {voice}"
    return f"{voice}"


def report(pane_id, value=None):
    args = ["herdr", "pane", "report-metadata", pane_id, "--source", SOURCE, "--ttl-ms", "5000"]
    args += ["--token", f"{TOKEN}={value}"] if value else ["--clear-token", TOKEN]
    command(*args)


def open_dictation_popup():
    # A popup uses the currently focused Herdr pane as its target. --no-focus
    # preserves that target so DontSpeak's native paste/Enter route is unchanged.
    return command(
        "herdr", "plugin", "pane", "open",
        "--plugin", POPUP_PLUGIN,
        "--entrypoint", POPUP_ENTRYPOINT,
        "--placement", "popup",
        "--no-focus",
    ).returncode == 0


def main():
    seen = set()
    seq = None
    dictation_visible = False
    while True:
        # Keep the external-UI lease in the long-lived bridge, not in the
        # short-lived popup. That lets DontSpeak choose Herdr before Caps Lock
        # starts a dictation turn, while the popup itself remains display-only.
        args = ["dontspeak", "status", "--json", "--ui-receiver", "herdr.voice"]
        if seq is not None:
            args += ["--since", str(seq), "--timeout-ms", "2000"]
        result = command(*args)
        if result.returncode:
            for pane_id in seen:
                report(pane_id)
            seen.clear()
            time.sleep(2)
            continue
        try:
            status = json.loads(result.stdout)
        except json.JSONDecodeError:
            time.sleep(1)
            continue
        seq = status.get("seq", seq)
        muted = bool(status.get("activity", {}).get("muted"))
        dictation = status.get("dictation", {})
        now_visible = dictation.get("state") != "hidden"
        if now_visible and not dictation_visible:
            dictation_visible = open_dictation_popup()
        elif not now_visible:
            dictation_visible = False
        current = set()
        for row in status.get("voice_sessions", []):
            pane_id = row.get("pane_id")
            if not pane_id:
                continue
            current.add(pane_id)
            report(pane_id, token_for(row, muted))
        for pane_id in seen - current:
            report(pane_id)
        seen = current


if __name__ == "__main__":
    main()
