# Troubleshooting

## The host does not appear or will not connect

Check these in order:

1. Confirm Neoism is open and its daemon is running on the host machine.
2. Confirm the client can reach that host over the intended local or private network.
3. If the host is listed but marked offline, refresh discovery and verify the host has not changed networks or gone to sleep.
4. If the connection reaches the host but authentication fails, pair again or verify that the device has not been revoked.

Discovery, reachability, and authorization are separate. A discovered host can still be unreachable, and a reachable host can still reject an unpaired device.

## A pairing code does not work

Generate a new code on the host. Pairing codes:

- expire after 60 seconds;
- can be claimed only once;
- are invalidated by a daemon restart;
- use an unambiguous uppercase alphabet.

If the claim reports pending, repeatedly submitting the same code will not help: claiming consumes it, and pending does not issue a token. The current daemon needs host auto-approval configured to grant immediately. Correct the host approval configuration and create a fresh code. If the claim is rejected, also review the requested permissions.

## Connected, but an action is denied

The device is authenticated but lacks the permission for that operation. File reads, file writes, PTY creation/use, agent access, clipboard access, and device management are separate capabilities. Re-pair with the intended permissions or have an authorized host device update the access arrangement. Do not treat a permission denial as a network failure.

## A terminal pane is blank or says its session is stale

The saved pane may refer to a PTY session that no longer exists. This commonly happens after the daemon restarts or after the shell exits. Create a new terminal session for that pane.

Closing a surface normally leaves a live daemon PTY available for reattachment, but stopping the daemon does not checkpoint the operating-system process. Workspace metadata surviving a restart does not imply that the old shell survived.

If a live session reconnects but earlier output is missing, remember that terminal catch-up history is bounded and held with the running daemon; it is not a permanent transcript.

## The Explorer shows the “wrong” directory

The Explorer shows the workspace's declared root. `cd` changes only the current directory of that terminal session. Use the workspace-root action to repoint the workspace; do not expect a shell command to move the Explorer.

On a remote workspace, also verify that the active workspace belongs to the intended host. Identically named folders on two hosts are still different workspaces.

## Another collaborator is visible, but edits do not match

Confirm both surfaces opened the same file in the same host workspace. Shared documents are keyed by the absolute path on the owning host; similar paths on two different daemons are not one collaborative document.

Then check connection status on both surfaces. Presence is a separate stream from document updates, so presence working does not prove text updates are flowing. Reconnect and reopen the file to request the daemon's current document snapshot.

Also check the kind of surface in use. The current web/mobile markdown view receives daemon content and presence but is not yet a full markdown co-editor; use a desktop editor surface when you need collaborative text entry.

If disk contents differ from the visible shared document, save from Neoism. The daemon writes the converged text and broadcasts the saved state. Unsaved collaborative edits are not promised as durable across a daemon restart.

## An agent conversation is missing while the terminal still works

Terminal and agent sessions have independent registries. A healthy PTY only proves that the workspace daemon's shell session is alive. Check the agent service connection and select or resume the intended agent conversation. Conversely, a resumable agent thread does not restore an exited shell.

## A remote file operation works but a terminal or agent does not

This usually indicates capability-specific permissions or a host service problem, not total disconnection. Verify the paired device has the relevant PTY or agent permission. For agents, also verify that the host's agent service/provider is available; pairing authorizes use but does not make an unavailable provider run.

## When reconnecting does not help

On the host, inspect Neoism's daemon status and recent logs for authentication rejection, permission denial, PTY exit, file-save errors, or agent-service failure. Avoid deleting daemon state as a first step: it contains workspace metadata and paired-device records. If credentials may be compromised, revoke the affected device and pair it again instead of sharing or copying its token.