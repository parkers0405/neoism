---
name: "ACP sidebar reorder and picker lag"
description: "ACP sidebar timestamps, cached options, native picker UX"
type: "bug"
scope: "project"
origin: "neoism-agent"
created: "2026-09-24"
updated: "2026-09-24"
---

2026-09-24: Native ACP chats jumped in sidebar on mere selection because options GET updated root `time.updated` in `external_agent/helpers.rs::update_external_session_metadata`, `options.rs::persist`, and `apply_commands`; sidebar sorts by this timestamp. Fix: preserve timestamp for metadata-only changes, suppress no-op metadata writes, keep actual prompt/activity timestamp updates. Options GET previously spawned/initialized/loaded ACP per switch (30-45s) and generated overlapping conflicts; now returns persisted provider-confirmed config snapshot for bound roots, while new roots and POST still use ACP; capability changes refresh on provider prompts/sets. Native slash picker has local `/model` action only when confirmed provider model select exists and local `/skill` browse text-reference fallback, with provider-advertised same-name commands taking precedence. Footer loading copy removed, category names normalized Model/Mode/Thinking, avoid cloning command list per frame. Tests in external_agent/options_tests.rs and native pane/tests.rs. Debug process must restart to load binary; do not claim live GUI verified from cargo check alone.
