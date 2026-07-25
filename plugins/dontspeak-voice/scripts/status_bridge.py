#!/usr/bin/env python3
"""Publish Don't Speak voice-session state into Herdr pane metadata."""

import json
import subprocess
import sys
import time

SOURCE = "plugin:dontspeak-voice"
TOKEN = "dontspeak_voice"
POPUP_PLUGIN = "dontspeak.voice"
POPUP_ENTRYPOINT = "dictation"
POPUP_RETRY_SECS = 5
PRESENTABLE_STATES = {"recording", "awaiting_confirm"}


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


def herdr_foreground():
    result = command("herdr", "api", "snapshot")
    if result.returncode:
        return False
    try:
        snapshot = json.loads(result.stdout).get("result", {}).get("snapshot", {})
    except json.JSONDecodeError:
        return False
    return snapshot.get("outer_terminal_focus") is True


def open_dictation_popup(session_id):
    # A popup uses the currently focused Herdr pane as its target. --no-focus
    # preserves that target so DontSpeak's native paste/Enter route is unchanged.
    if not herdr_foreground():
        return False
    return command(
        "herdr", "plugin", "pane", "open",
        "--plugin", POPUP_PLUGIN,
        "--entrypoint", POPUP_ENTRYPOINT,
        "--placement", "popup",
        "--no-focus",
        "--env", f"DONTSPEAK_DICTATION_SESSION={session_id}",
    ).returncode == 0


def open_current_dictation():
    result = command("dontspeak", "status", "--json")
    if result.returncode:
        return False
    try:
        dictation = json.loads(result.stdout).get("dictation", {})
    except json.JSONDecodeError:
        return False
    session_id = dictation.get("session_id")
    return bool(
        dictation.get("state") in PRESENTABLE_STATES
        and session_id
        and open_dictation_popup(session_id)
    )


def main():
    seen = set()
    seq = None
    opened_session = None
    opened_at = None
    while True:
        # This bridge observes status and launches the UI. The popup process owns
        # the presenter lease only after it has rendered the first snapshot.
        args = ["dontspeak", "status", "--json"]
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
        session_id = dictation.get("session_id")
        if dictation.get("state") in PRESENTABLE_STATES and session_id:
            external_active = bool(dictation.get("external_ui_active"))
            retry_due = (
                session_id == opened_session
                and not external_active
                and opened_at is not None
                and time.monotonic() - opened_at >= POPUP_RETRY_SECS
            )
            if (session_id != opened_session or retry_due) and open_dictation_popup(session_id):
                opened_session = session_id
                opened_at = time.monotonic()
        else:
            opened_session = None
            opened_at = None
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
    if sys.argv[1:] == ["--open-current"]:
        raise SystemExit(0 if open_current_dictation() else 1)
    main()
