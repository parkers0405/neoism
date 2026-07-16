# Neoism server connections

Each Neoism Desktop OS window is a client of one active workspace daemon.
Different windows can use different servers concurrently. Tailscale is an
optional discovery and reachability mechanism; it is not the connection model.

## Connection classes

- **Local Server** is the daemon started or selected by desktop startup. It has
  stable identity `local`; its socket or loopback transport is not persisted in
  the saved-server registry.
- **Saved servers** are user-named direct daemon endpoints with optional bearer
  credentials.
- **Discovered servers** remain supplied by pairing and tailnet discovery. A
  later UI increment can offer `Save server` without changing transport code.

The active server owns workspaces, sessions, PTYs, files, git, CRDT state, and
agent services for that window. Selecting a server replaces only that window's
daemon connection. It is distinct from moving a workspace between hosts.

## Window ownership

`Application` owns a `WindowServerSession` per `WindowId`. Each session owns its
connection runtime, active server id, status, profile id, host advertisements,
and pending workspace adoption. Daemon replies and outbound queues are routed
only through the owning window.

- A normal new window always dials Local Server.
- A detached workspace opens a new window on the source workspace's server.
- Closing a window drops only its server session and runtime.
- Explicit CLI server/SSH options affect the initial window only.

## Desktop behavior

Open the command palette and choose `Servers` to see Local Server and saved
servers. Search matches name or address. Selecting a row establishes and
authenticates a fresh connection before replacing the working connection.

`Add server` currently uses one modal input:

```text
https://host:7878 | Optional name | Optional access token
```

HTTP addresses normalize to `ws://.../session`; HTTPS addresses normalize to
`wss://.../session`. Explicit `ws` and `wss` addresses are also accepted.
Credentials in URL query parameters are rejected.

Saved servers are never an implicit startup default. Fresh windows start Local;
explicit `--daemon-url` and `--ssh-host` remain intentional initial-window
overrides.

## Persistence

Desktop writes these files under the Neoism config directory:

- `servers.json`: ids, names, canonical endpoints, and window-profile workspace
  subscriptions.
- `server-credentials.json`: tokens keyed by server id, separate from public
  metadata and mode `0600` on Unix.

The local daemon endpoint and credential are never added to these files.

Subscriptions are local UI preferences keyed by `(window profile, server id)`.
Window profiles are stable ordinals (`window-1`, `window-2`, …) assigned in
window-creation order, so the profile a window gets after a relaunch matches
the one that wrote its subscriptions — the main window is always `window-1`:

```text
subscribed workspace ids
last active workspace id
```

When a server catalog arrives, the owning window idempotently adopts all
subscribed workspaces present in that catalog and then selects the saved active
workspace. Tabs and pane layouts remain daemon-owned and restore through the
normal workspace snapshot path.

## Switching safety

The replacement connection must authenticate and reach protocol `Open` before
the old connection is dropped. Modified buffers block a switch. Clean switches
reset the owning window's daemon context, pane/session cache, Markdown CRDT
bindings, presence, pending file/git operations, visible file/git data, and
agent sessions before attaching the replacement.

Agent server endpoints are pane/window-owned; there is no process-global remote
agent override.

## Connection status

The top-right server rack and manager rows use live status:

- green: authenticated/open,
- amber: connecting or reconnecting,
- red: offline/failed,
- gray: saved but not yet probed.

Opening the server manager probes inactive saved endpoints and updates rows in
place. Only the active server has a checkmark.

## Follow-up UI work

- Add pointer context-menu affordances beside the existing keyboard Edit/Remove
  controls.
- Merge paired and Tailscale-discovered daemons under a Discovered section with
  Connect and Save actions.
- Show failed connection details in the notification panel.
