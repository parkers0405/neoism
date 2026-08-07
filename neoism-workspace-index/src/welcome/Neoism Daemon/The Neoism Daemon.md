# The Neoism Daemon

The Neoism daemon is the host-side service behind a Neoism workspace. The desktop app normally starts and connects to it for you. Web and mobile surfaces connect to the daemon running on the machine that owns the workspace.

## What the daemon owns

The daemon is the authority for:

- workspace records and each workspace's declared root directory;
- terminal sessions and their live PTY processes;
- terminal output history used when a surface attaches or reconnects;
- shared pane and tab state sent to connected surfaces;
- file operations performed against the host workspace;
- shared editor documents, saves, and collaborator presence;
- paired-device identities, permissions, and revocation;
- the bridge to Neoism agent sessions.

A workspace is a directory owned by one daemon. The Explorer follows that declared workspace root. Running `cd` in a terminal changes that terminal session's working directory; it does not silently redefine the workspace or move the Explorer root.

## Host and surface

The **host** is the machine running the daemon and holding the files and processes. A desktop window, browser, phone, or another paired Neoism installation is a **surface** connected to that host.

Closing or disconnecting a surface is not the same as ending work on the host. A live terminal belongs to the daemon, so another surface can attach to the same PTY and receive its retained output. Files and commands still execute on the host machine, not on the phone or browser displaying them.

## Local and remote connections

The desktop normally uses its embedded local daemon without asking for connection details. Remote surfaces connect to a reachable daemon and authenticate as a paired device. Network reachability and Neoism pairing are separate requirements: discovering or reaching a host does not by itself authorize access.

Neoism can describe other paired hosts and host their workspace trees in the workplace UI. Selecting a remote workspace changes which daemon is authoritative for that workspace; it does not copy all host state into the viewing device.

## Daemon sessions are not agent sessions

Neoism uses “session” for two distinct things:

1. A **workspace/PTY session** is a shell process owned by the workspace daemon. It has terminal dimensions, a current working directory, output history, and attached clients.
2. An **agent session** is a conversation or thread managed through the Neoism agent service. It has messages, tool activity, permissions, and its own conversation history.

An agent may use a workspace and terminals, but its conversation ID is not a PTY session ID. Reattaching to a terminal does not resume or select an agent conversation, and switching agent conversations does not replace the workspace's shell process.

See [[Sessions and Persistence]], [[Remote Devices and Pairing]], [[Multiplayer and Sync]], and [[Troubleshooting]].