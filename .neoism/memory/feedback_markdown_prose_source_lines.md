---
name: "Markdown prose: no arbitrary source hard wrapping"
description: "Agent Markdown tools now explicitly request one source line per prose paragraph; preserve deliberate breaks and raw writes. Screenshot wraps originated in assistant payload."
type: "feedback"
scope: "project"
origin: "neoism-agent"
created: "2026-09-17"
updated: "2026-09-17"
---

# Markdown prose source lines / agent writing contract

User observed awkward line breaks after agent edits and suspected Neoism editing tooling influences the model. Traced generic read/edit/write, Notes MCP, Markdown save and daemon CRDT save: they preserve source text, not visual wrapping. Screenshot docs/documentation-notebooks.md lines41-43 were 79/79/65-char physical lines, already present in this session's original assistant write payload. Optional formatter hook can alter text when configured; FormatterConfig defaults false. Do not claim every user's formatter is disabled.

Added explicit Markdown-authoring rule to engineering/build agent prompt in agent-builtins/plugin/agents/native.rs, generic write/edit/apply_patch descriptions in agent-server/tool_registry.rs, and desktop notes_mcp create/write descriptions: keep each prose paragraph on one source line, do not hard-wrap to arbitrary column widths, preserve deliberate breaks/lists/tables/fences and explicit user/project formatting requirements, do not reflow unrelated text. Guidance is preventive, not a mechanical source transform or guarantee of model compliance. Existing files were not unwrapped.

Regression coverage for prompt/MCP contract and write tool preserving a long paragraph plus deliberate hard breaks byte-for-byte. cargo check -p neoism-agent-builtins -p neoism-agent-server -p neoism --tests passed (compiled tests, not executed). Preserve user's intentional editor newlines; do not silently normalize all saves to compensate for model formatting habits.
