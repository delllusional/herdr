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

The `Dictate to this pane` action opens a popup
for the pane from which the action was invoked. While that popup stays alive it
leases the dictation presentation from Don't Speak, so native Don't Speak
overlays stay hidden; if the popup or plugin disappears, the short lease expires
and native overlays resume automatically.
