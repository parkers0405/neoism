# Native Omarchy agent-status plugin

Neoism bundles `assets/omarchy/dev.neoism.agent-status/`. This is an Omarchy
`bar-widget` plugin, **not a pinned generic tray icon**. Each running Neoism SNI
item (`id == "neoism"`) gets a native bar button using its own icon and tooltip.
Left-click calls that item's `activate()` (the notifier owns acknowledgement).
The widget does not infer agent state, focus windows, or open a popup. With no
items it occupies no space. Images request physical pixels for HiDPI displays.

## Activity behavior

- **Working:** animated N indicator while an execution family is unfinished, including subagents, queued work and background jobs. Finishing one tool call does not mean done.
- **Finished:** three brief flashes over three seconds, followed by a steady completion mark. Left-click acknowledges it without stealing focus. Finished means the run settled, not necessarily that its requested task succeeded.
- **Idle:** neutral N indicator.
- **Unknown:** neutral question mark when activity cannot be verified; a disconnected endpoint is never treated as finished.

The desktop owns one process-wide observer and notification item, independent of pane rendering or window focus. It deduplicates authorized direct-loopback agent endpoints and reads server-authoritative execution snapshots. Revision/generation checks reject stale updates. Initial historical completions do not flash. Each independent Neoism desktop process gets its own button; closing the application removes its item rather than keeping a tray-only process alive.

The activity API is `/v2/execution-activity` with `/v2/execution-activity/events` for snapshots over SSE. Hosted and workspace/directory-scoped credentials cannot use this global view. Permanently lost endpoints remain Unknown until process restart because the desktop cannot reliably prove their ownership or termination.

`ui.agent-tray` defaults to `true` and can be toggled in Settings under **Omarchy agent activity indicator**. Disabling it stops observation and removes the item; the installed plugin then has zero footprint. Real activity requires the rebuilt desktop/server code. The plugin itself can be installed and tested independently with the harness below. macOS and Windows status-bar integrations are not implemented in this change.

## Automatic installation

On a release Linux launch, a background task independent of desktop-launcher
installation detects `$OMARCHY_PATH/shell/shell.qml`, falling back to
`/usr/share/omarchy/shell/shell.qml`. It requires the plugin-capable shell IPC and
a live shell. Flatpak and development launches skip automatic installation.
Unavailable shells and incomplete activation are retried on the next launch.

Plugin discovery is hardcoded by Omarchy to
`~/.config/omarchy/plugins/dev.neoism.agent-status`, **regardless of
`XDG_CONFIG_HOME`**. Assets are compiled into Neoism; git and the source checkout
are not required on the installed machine.

The installer:

1. Publishes an atomically staged plugin with `.neoism-owned.json` (owner and
   SHA-256 asset hashes). A per-user nonblocking lock serializes Neoism launches.
   Unowned, symlinked, git/custom, or locally edited plugins are left untouched.
   Verified pristine owned upgrades use atomic directory exchange.
2. Calls `rescanPlugins`, polls `listPlugins` with a bound, then uses
   `putBarWidget dev.neoism.agent-status '{"section":"right"}'`. Existing
   placement/settings are preserved.
3. Reads **live** `listShellConfig`. Only after successful placement does it union
   `"neoism"` into each `omarchy.tray` entry's `hidden` names, through
   `setBarWidget`. Existing arrays are retained; legacy comma-separated strings
   are split into names, trimmed, and normalized to arrays. **Tray.qml only
   honors arrays**, not CSV strings. Other hidden names (including prefixes),
   pinned settings, and all unrelated fields are retained. Section/index
   selectors preserve distinct multiple trays.
   JSON array arguments receive a leading space to defeat `qs`/CLI11's bracket
   list expansion. Each array must round-trip exactly through both live
   `listShellConfig` and persisted `shell.json` before activation succeeds.
4. Writes `~/.config/neoism/bootstrap/omarchy/activated-v1.json`, independently of
   asset version. Subsequent launches do not re-add a manually removed bar entry,
   re-hide an explicitly unhidden generic icon, or reinstall a deleted plugin.

No full config rewrite, `/usr/share` modification, shell restart, or tray
ownership declaration is used. There is no atomic compare-and-swap IPC for the
hidden setting; avoid simultaneously editing that same setting during first-run
activation. Each write changes only that selected key.

## Array transport regression and guarded repair

On Quickshell 0.3.1, even shell-quoted `["neoism"]` is treated as a CLI bracket
list: its single element reaches `JSON.parse` as `"neoism"`, so the persisted
setting becomes a string. `["neoism","neoism"]` instead produces "Too many
arguments provided (4 required but 5 were provided.)". This is **argument
expansion, not a string-kind setting hook**. Sending `' ["neoism"]'` (a leading
space inside the argument) passes an intact JSON array. Host tests confirmed
both disk and live configuration became `hidden: ["neoism"]`.

A separate `~/.config/neoism/bootstrap/omarchy/hidden-array-v1.json` stamp guards
one-time repair of old activations. With a verified owned plugin, recognized
`activated-v1.json`, and the plugin still placed, only the exact malformed
`hidden: "neoism"` is repaired. Custom CSV, empty/removed hidden state, removed
plugins, and removed bar entries are not overridden. Already-correct arrays are
verified without rewriting. The existing activation marker is not reset;
after the fix stamp, later user choices are left alone. New first-run installs
normalize CSV names to arrays before writing either success marker.

An ignored reviewed-host test exercises the actual `LiveShell` → wrapper → qs
transport and verifies both effective and stored JSON. It requires an owned,
activated plugin and an existing Neoism-only hidden array; it writes that same
array and checks the entire configuration remains unchanged:

```sh
cargo test --manifest-path tools/omarchy-installer/Cargo.toml host_verify_array_transport -- --ignored --nocapture
```

## Isolated checks (no host shell changes)

```sh
cargo test --manifest-path tools/omarchy-installer/Cargo.toml
cargo check --manifest-path tools/omarchy-installer/Cargo.toml
omarchy-plugin-validate assets/omarchy/dev.neoism.agent-status
```

The standalone harness compiles the **same**
`neoism-frontend/desktop/src/bootstrap_omarchy.rs`, not a second installer, and
avoids a full application/release build. Tests use temporary HOME directories
and mock IPC: preservation, polling, retry, idempotence, ownership/hash guards,
symlinks, atomic upgrades, and user removal are covered.

## Explicit host installation / developer smoke test

**This mutates the host plugin/config and must be run only after review.**

```sh
cargo run --manifest-path tools/omarchy-installer/Cargo.toml -- --install-host
```

Without the exact flag the harness refuses to act. No environment opt-in can
silently enable it. It shares release detection, safety checks, and activation
markers. A live shell is required; launching/restarting the shell is not part of
the installer. After installation, test real SNI animation, tooltip, click
acknowledgement, two simultaneous processes, no-item collapse, and HiDPI display.

Removing the plugin's bar entry is respected on later launches. To deliberately
request activation again, remove only `activated-v1.json` above and rerun the
explicit installer. Do not remove the ownership marker to force an update: that
makes the plugin unowned and intentionally ineligible. Back up local edits and
remove/move the plugin directory yourself if you want to replace your own copy.
