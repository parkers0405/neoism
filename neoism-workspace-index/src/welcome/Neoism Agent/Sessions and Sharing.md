# Sessions and Sharing

A session is a durable Neoism Agent conversation. It stores the selected workspace directory, title, agent, model, messages, parts, parent relationship, timestamps, and runtime metadata.

## Create and continue

Opening a new agent pane creates or attaches to a session. Messages are appended to server storage before the response finishes. Closing the pane does not delete the session.

Use `/sessions` to open the session picker and `/new` to start another conversation. `/status` reports the current session and active work.

## Messages and parts

A session contains user and assistant messages. Each message contains ordered parts such as:

- Text and reasoning.
- File/image attachments.
- Tool calls and results.
- Agent markers.
- Step start and finish records.
- Patches, snapshots, and compaction summaries.

Clients load stored history and then follow the event stream. Neoism identifies parts by their parent message so optimistic local input and replayed server input do not become duplicate turns.

## Parent and child sessions

Subagents create child sessions with `parentID`. The server exposes child-session lists and includes source metadata in status, permission, and question events. A child can be opened independently or continued through its task ID.

Neoism can also fork a session at a specific message. The fork copies messages up to that point with new message and part IDs, preserving the original conversation.

## Share a session

Sharing marks a session with a Neoism URL:

```text
neoism://session/<session-id>
```

The link is a Neoism session locator for connected Neoism clients. It is not a public transcript uploaded to an anonymous web page by this server route. The receiving client still needs access to the Neoism host/workspace that owns the session.

Unsharing removes the link metadata; it does not delete the session.

Configure the sharing policy:

```jsonc
{
  "agent": {
    "share": "manual"
  }
}
```

Valid modes are `manual`, `auto`, and `disabled`. `autoshare: true` is also accepted for compatibility.

## Cross-device synchronization

The workspace daemon broadcasts agent snapshots and live events to attached desktop, browser, and mobile clients. A remote client can see user messages, assistant parts, running tools, permissions, questions, and child-session activity.

This is synchronization of one server-owned session, not two independent agents merging chat logs. If a client disconnects, it reloads stored history and resumes from live events after reconnecting.

## Import and export

Neoism supports workspace-scoped session bundles. Export collects sessions under a workspace root. Import writes bundles into the target workspace's agent store. This is used for workspace transfer and recovery, not as a substitute for source control.

## Archive and delete

Session updates can archive a conversation. Deleting removes it from the agent store. Neither operation deletes project source files that tools changed.

## Interrupt a run

Press `Esc` or invoke abort to stop the active response. Interruption prevents further model/tool steps but does not automatically revert edits or commands already completed. Use [[Undo and Redo]] for snapshot-based restoration where available.

See [[Compaction]] for long conversations and [[Agents and Subagents]] for child sessions.