# Don't Speak Voice for Herdr

This first version adds a compact `$dontspeak_voice` pane token and a popup that
shows live Don't Speak dictation inside Herdr. Link it locally with:

```text
herdr plugin link /path/to/plugins/dontspeak-voice
```

Add `$dontspeak_voice` to an agents-sidebar row in Herdr's UI configuration to
show the per-pane voice status, for example:

```toml
[ui.sidebar.agents]
rows = [["state_icon", "workspace"], ["agent", "$dontspeak_voice"]]
```

When Caps Lock starts local dictation, the startup bridge opens a small, read-only
popup over the active Herdr pane. It mirrors only the native Don't Speak
transcript and closes when dictation ends. Keyboard and paste events continue to
the underlying agent pane, so Don't Speak keeps its normal gesture semantics:
single tap inserts and submits; double tap inserts without Enter; long press
cancels. If the popup or plugin disappears, the short UI lease expires and the
native Don't Speak overlay resumes automatically.
