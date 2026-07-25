# Don't Speak Voice for Herdr

This first version adds a compact `$dontspeak_voice` pane token and a popup that
shows live Don't Speak dictation inside Herdr. It is implemented as a small Rust
runtime and has no Python dependency.

## Install

GitHub installs run the declared Cargo release build automatically:

```text
herdr plugin install delllusional/herdr/plugins/dontspeak-voice --ref codex/dontspeak-voice-plugin-rust
```

For local development, build first because `plugin link` intentionally does not
run manifest build commands:

```text
cargo build --locked --release --manifest-path runtime/Cargo.toml
herdr plugin link /path/to/plugins/dontspeak-voice
```

A Rust toolchain is required at install/build time. Herdr's current plugin
format builds source in the managed checkout; it does not select prebuilt assets
by platform and CPU architecture.

The terminal popup requires a Don’t Speak build with the session-scoped
`presenter` commands. Older builds can still supply voice-session badges; they
fall back to the native dictation overlay.

It also requires the Herdr session snapshot to expose
`outer_terminal_focus`—this feature branch does, while stock Herdr 0.7.5 does
not. Missing capability fails closed to the native overlay.

## Configure

Add `$dontspeak_voice` to an agents-sidebar row in Herdr's UI configuration to
show the per-pane voice status, for example:

```toml
[ui.sidebar.agents]
rows = [["state_icon", "workspace"], ["agent", "$dontspeak_voice"]]
```

When Caps Lock starts local dictation, the startup bridge opens a small,
display-only popup over the active Herdr pane. It mirrors only the native Don't
Speak transcript and closes when dictation ends. Keyboard and paste events
continue to the underlying agent pane, so Don't Speak keeps its normal gesture
semantics: single tap inserts and submits; double tap inserts without Enter;
long press cancels.

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

The bridge uses the current `dontspeak status --json` long-polling interface as
an observation adapter. Moving observation to a push subscription later does
not require changing metadata, presenter ownership, or popup rendering. See
[`DESIGN.md`](DESIGN.md) for lifecycle and compatibility details.
