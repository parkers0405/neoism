---
name: "Next-level settings intelligence"
description: "Typed, searchable settings hints shared by GUI and JSONC, with system fonts and host-discovered catalogs"
type: "feature"
scope: "project"
origin: "Current workspace implementation"
created: "2026-08-20"
updated: "2026-08-20"
---

Settings/config intelligence now uses typed `ConfigOption` values, constraints (min/max/step/unit/nullable), accepted union kinds, and declarative dynamic-provider IDs. The shared Settings page and JSONC completion consume the same descriptors. System font families populate the global and per-face family fields; IDE themes, terminal palettes, Mash Up Packs, shells, agents, and workspace LSP adapters are separate providers. Long option catalogs are searchable while open, including fonts. Numeric presets remain JSON numbers rather than string guesses, ranges are displayed/validated, platform nullable overrides are corrected, and major window/navigation/renderer/font/agent enums are constrained. Web mirrors the expanded wire shape. Focused protocol/backend/UI tests and cargo checks pass.
