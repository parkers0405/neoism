# Undo and Redo

Neoism can restore workspace changes recorded around an agent turn. This is separate from asking the model to reverse its work and separate from Git history.

## Undo

Use `/undo` to revert the latest reversible agent step. Neoism associates session messages with snapshots and patch metadata, restores the recorded workspace state, and marks the session as reverted at the relevant message/part boundary.

Undo does not erase the conversation. The retained history lets Neoism understand which turn was reverted and makes redo possible.

## Redo

Use `/redo` to apply the stored patch from a reverted step. The server also exposes `revert` and `unrevert` operations; the UI maps them to the user-facing undo/redo workflow.

## What it covers

Undo is designed for filesystem edits represented by Neoism's snapshot/patch system. It does not promise to reverse every side effect of a tool call.

Examples that may not be reversible:

- A pushed Git commit.
- A deployed service.
- An email or message sent through an integration.
- A database mutation.
- A shell command that modified state outside the tracked workspace.
- A process started in an external service.

Permission review remains necessary even when undo is available.

## Inspect before restoring

The session diff endpoint and Git changes panel show workspace changes associated with current work. Review the patch when undoing a large turn, especially if the workspace also contains concurrent user or subagent edits.

## Concurrency

Snapshot restoration cannot reliably distinguish overlapping modifications made after the snapshot by another process. Avoid undoing across concurrent edits without inspecting the diff first.

## Git relationship

Neoism undo is immediate session history; Git is durable project history. Keep meaningful work committed. Use Git to restore committed states and Neoism undo to back out the most recent agent step while working.

See [[Sessions and Sharing]] and [[Permissions]].