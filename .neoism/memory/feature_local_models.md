---
name: "Local models via compatible providers"
description: "OpenCode-style local GGUF runtime providers"
type: "feature"
scope: "project"
origin: "session"
created: "2026-08-01"
updated: "2026-08-01"
---

Implemented OpenCode-style local provider support. Config lives under root `agent.provider` (canonical AgentConfigDocument field `provider`) with provider name/npm/auth/env/options.baseURL/discoverModels/compatibility/models. Supports @ai-sdk/openai-compatible endpoints such as llama.cpp, Ollama, LM Studio; Neoism does not directly load GGUF, runtime server does. ProviderCatalog merges workspace config with Models.dev, bounded GET /models discovery (3s, 1MiB, 512 models, 60s cache, last-good fallback), manual model overrides, conservative capabilities. Provider API metadata carries auth/tool/stream-usage/reasoning compatibility. Runtime supports none/optional/required auth and omits bearer headers for keyless providers; local models appear as connected. Workspace plugin host creates ProviderPlatform with effective config and generation/compaction metadata uses same workspace provider. CLI picker now uses /v2/providers/configured. Docs: neoism-workspace-index/src/welcome/Neoism Agent/Providers.md. Verified focused tests and cargo check for core/builtins/server/CLI. Existing unrelated fixture edits were untouched.
