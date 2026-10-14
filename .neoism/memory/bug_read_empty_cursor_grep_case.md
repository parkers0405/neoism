---
name: "Read empty cursor + grep explicit case false fixed"
description: "Model sends cursor:\"\" + offset: now accepted; real cursor overrides offset. Explicit grep false now truly insensitive, omitted retains smartcase. 57 focused tests passed."
type: "bug"
scope: "project"
origin: "neoism-agent"
created: "2026-10-08"
updated: "2026-10-08"
---

# Read empty cursor and grep explicit false regressions fixed

Supersedes the read argument contract in feature_grep_read_pagination.md: model-facing tool schemas can cause models to supply every optional field. Never reject ordinary read calls containing cursor:"" plus offset. In tool_support/file.rs empty/whitespace/null cursor means absent; a valid nonempty cursor is authoritative and ignores offset (including default 1). Directories continue using offsets. tool_registry.rs descriptions updated; removed omit-offset rule. read_pagination tests exercise blank cursor files/directories and genuine continuations with offset through actual plugin tool dispatch. Tests using tool::execute now need AppState/plugin runtime, not just ToolContext, following concurrent runtime changes.

Grep: explicit caseSensitive:false MUST ignore uppercase/lowercase in patterns; true must respect case; only omitted uses smartcase. Resolve omission in server workspace_search.rs helper before constructing the strictly boolean service-api request. LineMatcher no longer re-applies uppercase smartcase. Fuzzy matcher uses neo_frizbee CaseMatching::Respect for true, not merely scoring bonuses. Regressions cover plain/regex/fuzzy/auto, indexed/walk/exact-file paths and public dispatch.

Verification: 28 fff tests passed; 29 focused server tests passed (15 read pagination, 3 registry, 5 existing read, 3 workspace search, 2 public grep, 1 external-directory permission). cargo check for server and fff passed. No release build/restart performed. Both defects reported by user model smoke test and corrected in source; rebuilt server required for live behavior.
