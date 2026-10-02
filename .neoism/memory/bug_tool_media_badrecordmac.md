---
name: "Tool media BadRecordMac — FIXED"
description: "Single-chat BadRecordMac caused by 50MB of replayed base64 tool image attachments; fixed media budget/pruning and repaired affected chat via compaction."
type: "bug"
scope: "project"
origin: "session diagnosis and implementation"
created: "2026-10-02"
updated: "2026-10-02"
---

# Tool image replay causes repeated BadRecordMac

## Symptom
A single long-running chat repeatedly failed OpenAI OAuth Responses requests with `received fatal alert: BadRecordMac`, while other chats and direct TLS tests worked.

## Root cause
The affected chat (`Minecraft-Themed Neoism Mashup Using Authentic Textures`) used the native `read` tool on four large PNG screenshots. `tool_support/file.rs` persisted each image as a base64 data URL under tool-result `metadata.attachments`. Serialized tool parts were approximately 10.4 MB, 15.1 MB, 17.5 MB, and 8.9 MB. `message_model::tool_result_messages` replayed every historical tool attachment on every provider request, creating a request above 50 MB. The normal tool-output cap only bounded the tiny text output (`Image read successfully`), and `prune_old_tool_outputs_in_messages` estimated only output text, not attachment URLs. Initial prompt construction also skipped pruning. The chat retried repeatedly and did not continue; a `busy`/`retry` status and newly created incomplete assistant rows were not proof of progress.

## Fix
In `neoism-agent/crates/neoism-agent-server/src/message_model.rs`:
- Bound aggregate tool-generated media replay to 20 MiB, preserving newest media and always permitting one newest data attachment.
- Omit older tool media from provider replay with a compact marker.
- Stop duplicating attachments onto the Tool-role provider message; adapters consume media through the generated User-role media message.

In `neoism-agent/crates/neoism-agent-server/src/session_prompt.rs`:
- Run tool-output pruning before the first provider request of each new prompt, not only in-run followups.
- Include attachment data URLs in replay-token accounting so old media tool results are persistently marked compacted.

## Live repair
POSTed `/v2/sessions/<id>/compact` for the affected chat. It returned 204 and wrote a non-empty 13,475-byte summary with `tailStartMessageId` after the giant image reads, so the existing installed runtime can retry that chat without replaying those images.

## Verification
- `cargo check -p neoism-agent-server`
- New tests for aggregate media budget and attachment token accounting
- Existing media replay, compacted media, and in-run pruning tests all pass
- `git diff --check` passes

## Diagnostic lesson
When a user says only one chat fails, inspect that chat's persisted tool-part sizes and terminal message errors. Do not infer recovery from `type: busy` or incomplete assistant rows; verify a completed assistant message with output or a settled run.
