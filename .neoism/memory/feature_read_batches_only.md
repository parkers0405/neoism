---
name: "Read bundles only within actual tool-call batches"
description: "Read/search bundles require same native model response batch ID; sequential neighboring reads stay separate; sidebar title Agents"
type: "feature"
scope: "project"
origin: "neoism-agent"
created: "2026-10-07"
updated: "2026-10-07"
---

Read/search tool bundling is now gated by authoritative batch provenance, not proximity or three-adjacent-row threshold. ProviderStreamStepState generates a unique tool_batch_id for each native model response; pending and ToolCall parts are stamped at top-level metadata.toolBatchID before store/broadcast, including missing-input-start and validation-error paths. Existing state transitions preserve that metadata. The shared api_mapping tool_block maps only nonempty top-level toolBatchID (never messageID, timing or external ACP assistant-turn IDs). Both UI message structs, shared/desktop timeline traits, desktop From conversion, protocol HistoryMessage (optional/default), daemon history conversion, wasm history mapping and TS wire types carry tool_batch_id. read_tool_group_at requires >=2 contiguous eligible read-like calls with the SAME nonempty batch ID; separate/native steps and rows without batch provenance stay individual, including older transcripts and external ACP events without a real batch ID. Existing tool_group messages cannot be regrouped. Stable first-child id.. identities, four-child preview cap, expansion anchors and animation dirtying remain unchanged. Sidebar agent section heading in shared view/side_panel/sections.rs changed from Branches to Agents. Regression tests cover absent/blank batch provenance, two-member batches, distinct adjacent batches, stable IDs/live status updates, failures/existing group isolation, per-step unique IDs and pending/running/completed/error metadata retention. Native production cargo check -p neoism-ui -p neoism -p neoism-protocol passed. Other check results recorded in session.
