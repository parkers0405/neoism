---
name: "Canonical config intelligence core"
description: "Canonical schema + raw JSONC revision API + comment-preserving targeted edits implemented"
type: "feature"
scope: "project"
origin: "neoism-agent"
created: "2026-08-20"
updated: "2026-08-20"
---

Implemented canonical config intelligence across protocol/backend/daemon config surfaces. `neoism-protocol::config` now carries serializable `ConfigDescriptor`, value/category/control metadata, raw `ConfigDocument`, and GetConfigSchema/GetConfigDocument/EnsureConfigDocument/SaveConfigDocument messages. `neoism-backend::config::intelligence::config_descriptors()` provides curated settings plus exhaustive descriptors generated from grouped Config defaults, runtime fonts/themes/shells, and static agent/reasoning/LSP suggestions. `create_config_file` now returns `io::Result<PathBuf>`, creates custom parents, and uses create_new. Raw document reads expose display path/revision/writable; saves validate JSONC/grouped config, check opaque FNV revision, and atomically rename. `write_setting` and `write_keybind` use a concrete-syntax JSONC span editor preserving unrelated comments/format/order. Daemon config surface wires permissions and handlers. Protocol full tests passed; backend targeted tests/check passed. Daemon check blocked by unrelated missing neoism-agent-server/src/workflow.rs from concurrent work.
