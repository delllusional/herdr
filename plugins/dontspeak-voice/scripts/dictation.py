#!/usr/bin/env python3
"""Terminal presenter for one Don't Speak dictation session."""

import json
import os
import subprocess
import time

PRESENTER_ID = "herdr.voice"
LEASE_TTL_MS = 3500
PRESENTABLE_STATES = {"recording", "awaiting_confirm"}


def status(since=None):
    args = ["dontspeak", "status", "--json"]
    if since is not None:
        args += ["--since", str(since), "--timeout-ms", "1000"]
    result = subprocess.run(args, capture_output=True, text=True, check=False)
    if result.returncode:
        return None
    try:
        return json.loads(result.stdout)
    except json.JSONDecodeError:
        return None


def presenter(action, session_id, lease_id=None):
    args = ["dontspeak", "presenter", action, "--session", session_id]
    if action == "acquire":
        args += ["--id", PRESENTER_ID, "--ttl-ms", str(LEASE_TTL_MS)]
    else:
        args += ["--lease", lease_id]
        if action == "renew":
            args += ["--ttl-ms", str(LEASE_TTL_MS)]
    result = subprocess.run(args, capture_output=True, text=True, check=False)
    if result.returncode:
        return None
    if action != "acquire":
        return True
    try:
        return json.loads(result.stdout).get("lease_id")
    except json.JSONDecodeError:
        return None


def herdr_foreground():
    result = subprocess.run(
        ["herdr", "api", "snapshot"],
        capture_output=True,
        text=True,
        check=False,
    )
    if result.returncode:
        return False
    try:
        snapshot = json.loads(result.stdout).get("result", {}).get("snapshot", {})
    except json.JSONDecodeError:
        return False
    return snapshot.get("outer_terminal_focus") is True


def draw(text):
    # No controls, state labels, target name, or instructions: this is the same
    # presentation layer as the native overlay, not a second input surface.
    print("\033[2J\033[H", end="")
    print(text, end="", flush=True)


def main():
    expected_session = os.environ.get("DONTSPEAK_DICTATION_SESSION")
    snapshot = status()
    if not snapshot:
        return
    dictation = snapshot.get("dictation", {})
    session_id = dictation.get("session_id")
    if (
        dictation.get("state") not in PRESENTABLE_STATES
        or not session_id
        or session_id != expected_session
        or not herdr_foreground()
    ):
        return

    lease_id = presenter("acquire", session_id)
    if not lease_id:
        return
    try:
        # Ready is deliberately sent after the first flushed render. Until then
        # Don’t Speak keeps its native overlay visible as the safe fallback.
        draw(dictation.get("text", ""))
        if not herdr_foreground() or not presenter("ready", session_id, lease_id):
            return
        seq = snapshot.get("seq")
        last_status_at = time.monotonic()
        while True:
            snapshot = status(seq)
            if not snapshot:
                if (time.monotonic() - last_status_at) * 1000 >= LEASE_TTL_MS:
                    return
                time.sleep(0.1)
                continue
            last_status_at = time.monotonic()
            seq = snapshot.get("seq", seq)
            dictation = snapshot.get("dictation", {})
            if (
                dictation.get("state") not in PRESENTABLE_STATES
                or dictation.get("session_id") != session_id
                or not herdr_foreground()
            ):
                return
            draw(dictation.get("text", ""))
            if not presenter("renew", session_id, lease_id):
                return
    finally:
        presenter("release", session_id, lease_id)


if __name__ == "__main__":
    main()
