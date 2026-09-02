---
name: "Descriptor-driven Settings GUI"
description: "Canonical descriptor schema now drives desktop/web Settings GUI with owned rows and inline generic editing"
type: "feature"
scope: "project"
origin: "neoism-agent"
created: "2026-08-20"
updated: "2026-08-20"
---

Follow-up GUI integration completed: NeoismSettingsPane now consumes owned Vec<neoism_protocol::config::ConfigDescriptor>, removes hardcoded SETTINGS metadata, maps all 11 protocol categories and controls, displays every descriptor path, merges static/runtime suggestions plus desktop font/theme sources, preserves model picker/font/theme/keybinding specializations, supports extensible custom values, and provides inline typed string/number/array/object editing emitting owned SettingsAction::Set paths. Desktop seeds from neoism_backend::config::intelligence::config_descriptors(); web fetches GetConfigSchema and calls wasm set_settings_descriptors. Checks: cargo check -p neoism-ui and -p neoism-terminal-wasm passed; 3 focused Settings tests passed; web npm typecheck passed. Desktop check remains blocked before frontend compilation by concurrent missing neoism-agent-server/src/workflow.rs.
