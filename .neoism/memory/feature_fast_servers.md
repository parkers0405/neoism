---
name: "Fast Servers: temporary OpenTunnel hosting and recovery"
description: "Desktop Add Fast Server Host/Join, purple temp dots, memory-only registry; OpenTunnel authenticated ingress; delayed vertical reconnect modal; same Create Server folder default"
type: "feature"
scope: "project"
origin: "neoism-agent"
created: "2026-10-12"
updated: "2026-10-12"
---

# Fast Servers: OpenTunnel, temporary entries, and recovery

Desktop Servers picker has one `Add Fast Server` action opening the existing form-style Host/Join modal. Online temporary entries use `theme.magenta` (purple) instead of green, without Saved/Hosting/Joined tags; joined rows offer Leave and hosted rows offer Copy invite / Stop hosting. Row-control clicks use the normal dispatcher and their backgrounds/hitboxes are clipped to the list viewport. Server forms have 12px rounded corners and no left accent stripe.

Fast Host uses the SAME `workspace_root_for_new_shell()` directory default as existing Create Server, per user correction. It starts an owned, dedicated loopback daemon rooted at the selected canonical directory on the hosting machine, not a VM and not a copy of a remote workspace. Fast Join accepts a complete HTTPS/WSS invite plus password; friendly names are labels, not global lookup keys. Core files: `desktop/src/fast_servers.rs`, `app/fast_server_sessions.rs`, `desktop/src/server_registry.rs`, `service-api/src/fast_hosting.rs`, and `workspace-daemon/src/server/fast.rs`.

Desktop registry wraps the persistent service registry with in-memory temporary metadata, credentials, and per-profile subscriptions. Temporary entries survive transient outages but are removed on explicit Leave, Stop hosting, or confirmed HostEnded. Endpoint-scoped Screen teardown removes adopted grids/chrome and subscriptions, detaches guest sessions before close (no host ClosePty), creates a local fallback if necessary, and preflights all affected windows for unsaved edits. Unsaved edits require Keep workspace / Leave and discard confirmation. Owned-host death is handled once; app exit calls `stop_and_wait()` because process::exit skips Drop.

Reconnect UI lives in `app/connection_recovery.rs`: after 30 seconds of observed active recovery, show one modal per outage with vertical Retry now / Leave workspace rows. Sampling gaps over 15 seconds restart grace after suspension; eight-second backoff gaps still accumulate. Reconnection dismisses the matching modal by its action endpoint, not English title. Retry wakes the SAME active/parked runner via `request_reconnect`, never creates a duplicate client. Home and confirmed ended/rejected links do not get ordinary retry. Parked drains preserve HostEnded and rejected HelloAck while dropping stale PTY frames.

Async server switches now carry a generation and desired endpoint; late or canceled results cannot reopen a departed server, override a newer parked/current selection, or clear a newer inflight marker. A new different target invalidates an older request before the unsaved guard; selecting the current server can cancel a pending dial without saving. Async commit rechecks live dirty documents as well as tab dots to protect edits made while dialing. Teardown clears provisional departing-endpoint joins after all chrome swaps.

OpenTunnel Rust SDK is pinned to 0.5.1. Public ingress authenticates before private WS dispatch and exposes filtered multiplayer plus workspace-scoped agent HTTP/SSE. Guest password, owner ingress credential, and daemon master token are separate; invites do not contain the owner/master secrets. Valid credentials are checked before failed-attempt throttling. File API paths are root-scoped; creation requires a single basename and joined-destination symlink checks; administration planes stay private. Artifact filename/session headers are forwarded only on POST v2/artifacts. Provider/MCP auth-store paths are resolved before changing cwd; provider OAuth storage does not follow NEOISM_CONFIG_HOME, so existing host provider credentials remain usable without copying daemon/device registry secrets.

This is TRUSTED host-user execution, not a sandbox: terminal/agent access can use host capabilities and model budget. Explicit stop attempts HostEnded even during private upgrade/blocked forwarding, drains tracked proxies with a bounded deadline, withdraws forwarding, and stops owned processes. Unix ownership tracking and Windows jobs cover ordinary owned descendants; deliberately detached/never-observed Unix descendants and older-platform signal races are documented limits. Farewell to an unwritable peer is bounded best effort.

Invites currently support desktop Fast Join; public static browser IDE/password bootstrap is NOT implemented. Verification uses cargo check, including test targets, plus wasm32 web check; tests are compile-checked, not executed. Live OpenTunnel provisioning/OAuth flows and macOS/Windows runtime validation remain outstanding. Do not claim end-to-end live validation or run cargo build --release.
