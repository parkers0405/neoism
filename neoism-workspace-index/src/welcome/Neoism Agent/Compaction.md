# Compaction

Compaction keeps a long session usable when its accumulated messages approach the selected model's context limit. Neoism summarizes older context into a durable compaction part and continues from that summary plus newer messages.

## Manual compaction

Run `/compact` or `/summarize`. The Agent server generates a structured handoff summary containing the active goal, constraints, progress, decisions, next steps, and critical context.

The summary is stored in the session. It is not only transient text shown in the UI.

## Automatic compaction

Neoism estimates context use from message content, tool output, attachments, and model limits. When the session becomes too large, it can compact before the next provider request rather than sending an oversized prompt.

Compaction avoids repeatedly compacting when no new material has been added since the previous compaction.

## What remains

After compaction, Neoism keeps:

- The structured summary of prior work.
- Messages newer than the compaction boundary.
- The session's agent/model metadata.
- Durable child-session and task references that remain in storage.

Original messages remain part of stored history, but the full old transcript is no longer necessarily included in each model request.

## What can be lost from model context

A summary cannot preserve every exact sentence or tool byte. Details are most at risk when they were never written to a file or captured as a durable decision.

For long work:

- Keep source-of-truth decisions in project notes or memory.
- Save exact commands, IDs, and paths when they matter.
- Ask the agent to produce a handoff summary before switching providers.
- Reattach a file or quote exact text when later work depends on it.

## Hidden compaction agent

Neoism uses an internal compaction agent with a low temperature. It is not a normal selectable implementation agent. The selected provider/model still determines whether compaction can run successfully.

## Failure behavior

Compaction can fail if no model is available, provider authentication expires, the provider rejects the context, or the session is interrupted. The existing session history is not deleted by a failed compaction.
See [[Sessions and Sharing]] and [[Memory]].
