---
name: "Queued prompt duplicate identity — FIXED"
description: "Queued user-message duplicates fixed by preserving message ID and author through dequeue event handling"
type: "bug"
scope: "project"
origin: "neoism-agent"
created: "2026-08-31"
updated: "2026-08-31"
---

Queued agent prompts produced duplicate user bubbles because `session.queue.updated` carried the original `messageID` and `request.author`, but shared `classify_session_event` reduced dequeue to text only. Desktop/shared then inserted an empty-ID, authorless user row; the later server user-part broadcast had the durable ID and real author and could append separately depending on ordering/cache path. Fixed by carrying `message_id` and `author` through `SessionEventUpdate::DequeuedPrompt` and desktop `AgentSessionUpdate`, assigning them to active and inactive-cache dequeue rows, and reconciling exact IDs before text fallback. Regression tests cover event identity extraction and dequeue+server-echo single-row behavior in shared and desktop. Verified `cargo test -p neoism-ui dequeued_prompt`, `cargo test -p neoism dequeued_prompt`, `cargo check -p neoism-ui`, and `cargo check -p neoism`.
