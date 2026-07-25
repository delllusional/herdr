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

When Caps Lock starts local dictation, the startup bridge opens a small, display-only
popup over the active Herdr pane. It mirrors only the native Don't Speak
transcript and closes when dictation ends. Keyboard and paste events continue to
the underlying agent pane, so Don't Speak keeps its normal gesture semantics:
single tap inserts and submits; double tap inserts without Enter; long press
cancels.

The popup acquires a lease scoped to the current dictation session, renders its
first snapshot, and only then marks itself ready. It renews while visible and
releases on close. The startup bridge never owns that lease. If the popup fails
to start, exits, or stops renewing, the native Don't Speak overlay remains or
returns automatically without giving the plugin microphone, paste, or
Accessibility capabilities.

External presentation is fail-closed on Herdr's foreground-client focus signal:
the popup does not take over when the terminal is backgrounded, and focus loss
releases the lease during dictation. Terminals or nested multiplexers that do
not report focus changes keep the native Don't Speak overlay instead.

The terminal presenter requires a Herdr build whose session snapshot exposes
`outer_terminal_focus` (this feature branch includes it). On an older or stock
build without that optional field, voice metadata still works and dictation
degrades safely to the native Don't Speak overlay.
