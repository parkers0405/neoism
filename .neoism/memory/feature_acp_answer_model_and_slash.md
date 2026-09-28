---
name: "ACP answer model and slash surfaces"
description: "ACP model attribution and OpenCode command catalog distinction"
type: "feature"
scope: "project"
origin: "User screenshots, current debug API read-only inspection and OpenCode ACP source audit"
created: "2026-09-24"
updated: "2026-09-24"
---

2026-09-24 follow-up: user screenshot under answer showed `Opencode · Opencode · duration · throughput`. Root cause: `external_model(runtime)` sets UserModel.model_id to provider ID for queued prompt; `lifecycle.rs::start_assistant_step` writes placeholder and `acp_run.rs` copied same placeholder into ProviderGenerationResponse at completion. New code takes provider-confirmed `category:model`/`type:select` `currentValue` from root extra.externalAgent.configOptions AFTER ACP final event drain, writes it into completed assistant MessageInfo; if absent, empty model. Shared `api_mapping.rs::assistant_response_footer` omits unknown and filters already-persisted external provider-name placeholder, preserves duration/throughput. ACP does not guarantee per-answer exact model if agent internally switches; this is turn-final session-selected model, not proof of each token. Fake lifecycle and options tests assert default and selected model persistence; footer test covers legacy, unknown, known.
OpenCode slash follow-up: live debug API current port 38111 root showed exactly six ACP `availableCommands`: customize-opencode, diagnose-crash, init, omarchy, review, typesafe-ai. Installed OpenCode 1.18.31 ACP service source builds available commands from SDK command.list plus skills; TUI built-in slash actions are separate, not advertised/mostly not executable over ACP. ACP special-cases `/compact` via session/prompt to summarize even if unadvertised; Neoism native picker now adds only this verified OpenCode ACP action (dedupes if advertised), routes it as provider draft. Never fill provider picker with unsupported TUI command names; to add more, implement real app/client actions separately and label ownership. Current running debug app still needs restart for code changes; do not claim live GUI verified. Backend 29 ACP tests/native /compact test pass; footer test initial expectation was 6s vs actual 6.0s, fixed and recheck running.
