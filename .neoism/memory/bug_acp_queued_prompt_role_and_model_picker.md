---
name: "ACP queued user role and model picker"
description: "ACP queued prompt duplicate and /model collision"
type: "bug"
scope: "project"
origin: "neoism-agent"
created: "2026-09-24"
updated: "2026-09-24"
---

2026-09-24: Screenshot native ACP root showed user bubble and same text as plain assistant row under it while 'Crafting'. Root cause NOT provider agent_message_chunk: `external_agent/lifecycle.rs::append_external_user_message` published `MESSAGE_PART_UPDATED` with serialized text part missing `role:"user"`; shared `api_mapping::part_block` classified any untagged text as Assistant. Queued external roots call this helper with create_reply=false, so temporary duplicate disappears on history reload. Fix stamps role user (+author) on broadcast, preserves stored part schema; lifecycle HTTP regression waits for user-role event. Native external footer removed provider label and `Model:/Mode:/Thinking:` prefixes, renders provider-confirmed mode/model/thought_level selections (values only) then other options; ordered indices map to original option IDs for chip clicks and overflow. `/model` Enter picker was a collision: provider slash command value `model` ranked above local sentinel; now local row value `model` outranks `Provider /model` and opens config model; pending root retains `/model` intent until options arrive, no prompt sent. Keep provider-specific skills/MCP boundary explicit: T3 uses Codex app-server skills/list, Claude Agent SDK + SKILL.md scan, OpenCode SDK app.skills/command.list, not a universal ACP skills API; Neoism ACP passes mcpServers:[] and local /skill inserts text reference only. Native debug process restart required to verify UI.
