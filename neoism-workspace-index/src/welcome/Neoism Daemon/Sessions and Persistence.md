# Sessions and Persistence

## Workspace and PTY sessions

A terminal pane is backed by a real PTY session on the daemon host. Creating a terminal starts a host shell in the requested working directory. Input from an attached surface goes to that PTY, and output is broadcast to attached surfaces.

Disconnecting a browser, closing a window, or changing surfaces does not intentionally kill the PTY. On attachment, the daemon sends retained terminal history before continuing with live output. Resizing a pane updates the host PTY size.

Ending the shell, explicitly closing its terminal session, or stopping the daemon ends the live process. A shell that has exited cannot be revived by attaching to its old ID.

## What the daemon persists

The daemon snapshots workspace metadata, including workspace identity, name and root, session records, current session selection, client tab state, and shared layout-related state. It writes state through a debounced snapshot and loads it again on startup.

This metadata persistence is not process checkpointing. After a daemon restart, Neoism may still know that a workspace or pane referred to a session, but the old operating-system PTY process and its in-memory output ring are gone. A stale pane must create or attach to a live replacement session.

Terminal history is retained in memory with the live PTY so reconnecting clients can catch up. It is bounded and is not a durable transcript promised across daemon restarts.

## Workspace roots and terminal directories

The workspace root and a terminal's current directory are separate state:

- the workspace root controls the Explorer and workspace-scoped file operations;
- each PTY tracks its own current working directory;
- `cd` affects that shell only;
- changing the workspace root is an explicit workspace action.

This separation is especially important with multiple terminals: each shell can be in a different directory while all panes still belong to the same declared workspace.

## Agent sessions

Agent sessions are conversation threads, not shells. Their lifecycle is handled by the Neoism agent service and its configured provider/session storage. The workspace daemon exposes and relays agent operations, but workspace state snapshots do not turn an agent conversation into a PTY or preserve a running agent process as terminal history.

Agent conversation history may be listed and resumed when the backing agent service has retained it. A terminal's reconnect behavior says nothing about whether an agent provider can resume a thread, and the reverse is also true.

## What stays local to the host

The following remain on, or execute on, the host daemon unless an explicit workspace-transfer feature is used:

- the workspace's files and Git checkout;
- shell and child processes;
- environment variables and local credentials available to those processes;
- PTY state and retained terminal output;
- host toolchains, language servers, and filesystem permissions.

Connecting another surface gives it a view and permitted control of host-owned state; it is not a general filesystem mirror.