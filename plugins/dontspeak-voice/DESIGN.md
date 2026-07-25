# Rust runtime design

## Scope

Replace the plugin's Python scripts with one small Rust executable while keeping
the manifest action, pane entrypoint, metadata token, and compact input-
passthrough popup behavior stable.

## Build and distribution

The runtime is a standalone Cargo crate under `runtime/`, not a dependency of
the Herdr binary. GitHub plugin installation runs the manifest's host-platform
Cargo build command and keeps the resulting release binary inside the managed
plugin checkout. Local `plugin link` development requires the same Cargo build
to be run explicitly because Herdr intentionally does not build linked plugins.

This is a source-distributed plugin, not a prebuilt universal binary. Users need
a Rust toolchain at installation time. Herdr's current plugin format has no
platform-asset resolver, so pretending that one checked-in executable is
portable across macOS/Linux architectures would be incorrect.

## Runtime model

The executable has four entrypoints:

- `startup` launches a detached bridge for the current Herdr server and exits,
  respecting the plugin contract that startup hooks are one-shot.
- `bridge` observes status without claiming presentation, mirrors pane-keyed
  voice status into Herdr metadata, and opens the dictation popup for the
  visible session.
- `open` resolves the current foreground dictation session for the manual
  action, then opens the same scoped popup as the bridge.
- `dictation` acquires a session-scoped presenter lease, renders only the live
  transcript, renews while the same session is visible, and releases on exit.

The bridge holds an OS file lock scoped to the Herdr socket, preventing
duplicate bridge processes after live handoff. It probes the Herdr server and
exits after repeated loss so a later server startup can replace it. Child
commands use argv directly, bounded output capture, execution timeouts, and
sanitized status labels.

Both popup launch and lease ownership require Herdr's live session snapshot to
report `outer_terminal_focus: true`. Missing or false focus fails closed. The
popup rechecks immediately before Ready and each Renew; focus loss releases the
lease so a background Herdr client cannot suppress Don’t Speak's native UI.

## Don’t Speak compatibility boundary

`DontSpeakClient` separates status and presenter transport from state handling.
The initial adapter uses the shared CLI contract:

```text
dontspeak status --json [--since N --timeout-ms N]
dontspeak presenter acquire --id ID --session SESSION --ttl-ms N
dontspeak presenter ready --lease LEASE --session SESSION
dontspeak presenter renew --lease LEASE --session SESSION --ttl-ms N
dontspeak presenter release --lease LEASE --session SESSION
```

Long polling is the current observation transport, not the presenter ownership
model. A future push/subscription adapter can implement the same client boundary
without changing metadata reconciliation, popup lifecycle, or rendering.
Presenter ownership is always explicit and scoped to the opaque dictation
session id.

## Lifecycle and failure behavior

- Don’t Speak unavailable: clear previously published tokens, back off, retry.
- Herdr unavailable repeatedly: clear best-effort, release the lock, exit.
- Invalid or partial JSON: reject the snapshot without publishing partial
  metadata.
- Popup open race, crash, or another modal: retry after a delay longer than the
  presenter lease without changing focus.
- Manual action, popup launch race, or stale session: resolve and validate the
  current recording/confirmation session before opening or acquiring a lease.
- Refusal state: remain on the native presenter because the terminal popup is a
  text-only mirror.
- Herdr is backgrounded or focus is unknown: do not open or activate the popup;
  release an active lease on the next status update.
- Popup crash, close, or removal: the session lease expires and Don’t Speak
  restores its native overlay by the next native status refresh.
- Bridge crash: metadata TTLs expire; it cannot hide the native overlay because
  the bridge never owns presentation.

The plugin never logs transcript text or full status payloads. Diagnostics
contain only command categories and sanitized error summaries.

## Platform boundary

The baseline manifest remains macOS/Linux because those are the platforms
already supported by the plugin and its no-focus popup path. The Rust crate
keeps process-launch code compile-gated so Windows support can be added later,
but this change does not claim untested Windows plugin support.
