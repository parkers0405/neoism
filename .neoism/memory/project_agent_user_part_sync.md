---
name: agent-user-part-sync
description: "Agent-server broadcasts user-prompt parts tagged role:\"user\"; part_block maps them to User bubbles; sender dedupes by adopting server id onto empty-id optimistic copy"
metadata: 
  node_type: memory
  type: project
  originSessionId: 3eff29bd-a895-4c7f-98f6-b44fd5974e1b
---

Cross-device live user messages (2026-06-11): opencode-style parts carry NO
role, so remote devices never saw user prompts live (only `message.updated`
info envelopes, no text). Fix spans three layers:

1. agent-server `session_prompt.rs`: after the user `message.updated` publish,
   also publishes `message.part.updated` per user part with `"role": "user"`
   injected into the part JSON (event-only, not persisted).
2. shared `api_mapping::part_block`: `"text"` part with `role == "user"` →
   `agent_message_user` (else assistant, as before). Both desktop (PartUpdated)
   and daemon (`history_from_part` → MessageUpdated) go through part_block.
3. shared `upsert_part_message`: incoming User part with non-empty id adopts
   onto an existing User message with EMPTY id + same trimmed text (the
   sender's optimistic copy) instead of appending a duplicate bubble.

**Why:** parts are role-less by design; tagging only the broadcast event keeps
the stored shape wire-compatible.

**How to apply:** needs agent-server + daemon restart to take effect; testing
requires two attached clients on one session.
