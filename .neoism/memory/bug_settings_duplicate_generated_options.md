---
name: "Settings duplicate generated options"
description: "Settings GUI no longer renders generated config completion templates as duplicate controls"
type: "bug"
scope: "project"
origin: "session implementation"
created: "2026-08-28"
updated: "2026-08-28"
---

Settings duplicate rows were fixed by separating graphical Settings visibility from exhaustive JSONC config intelligence.

Root cause: `config_descriptors()` appends every serialized config leaf, all Linux/Windows/macOS platform templates, and wildcard `agent.agent.*`/`agent.mode.*` completion templates. The Settings pane rendered every descriptor, while generated labels used only the final path component, yielding repeated Mode/Family/Backend/etc. User values and aliases were not duplicating descriptors.

Implementation:
- `neoism_protocol::config::ConfigDescriptor` now has serde-compatible `settings_visible` (defaults true for older payloads).
- Curated `d(...)` descriptors are visible; `generated_descriptor(...)` marks completion-only rows hidden.
- Generated `agent.*` descriptors map to `ConfigCategory::Agent` rather than General.
- Shared Settings `set_descriptors` filters hidden descriptors and defensively rejects any path with a literal `*`, so templates can never become writable GUI controls or reappear through search.
- Exhaustive generated descriptors remain available for JSONC completion/hover and raw config editing.

Tests cover protocol roundtrip, descriptor uniqueness/exhaustiveness, hidden platform/wildcard templates, Agent categorization, GUI filtering, and search not reintroducing hidden rows. Targeted protocol/backend/UI tests and `cargo check -p neoism -p neoism-ui -p neoism-workspace-daemon` pass.
