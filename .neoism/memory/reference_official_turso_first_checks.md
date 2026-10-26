---
name: "Official Turso first runtime checks"
description: "43 official-Turso runtime checks pass; latest recovery/migration still needs final test rebuild; transport lock aligned"
type: "reference"
scope: "project"
origin: "Official-engine runtime verification"
created: "2026-10-25"
updated: "2026-10-25"
---

Official Turso0.8.3 first runtime test binary target/activation/deps/neoism_agent_server-686f174c994c89c2 passed43 checks:13 event_storage (12 codec/compactor/budget tests plus actual HTTP compressed_replay_crosses_byte_and_row_pages_before_live_family_baseline),21 conversation_search (1 old-format fixture test ignored pending generation),4 family/session_descendant,5 live_messages. Compression synthetic sample saved99.49% (12,583,626 ->64,614bytes), not prediction for live DB. Completed initial cargo check server --tests official0.8.3. Latest saved-message recovery edits landed during compilation; test binary contains unused recovery helpers but not final prompt linkage, so MUST recompile once final source settles and run lifecycle/prompt/queue suites. Migration agent is strengthening fixture events/multipage content and explicit COMMIT-error rollback; wait completion before final rebuild. No live DB changes/restart/activation.

Standalone desktop/tests/transport_contract/Cargo.lock also updated narrowly through cargo update --manifest-path neoism-frontend/desktop/tests/transport_contract/Cargo.toml -p turso --precise0.8.3; official engine all0.8.3. tools/fts-v1-fixture intentionally pins official0.6.1 as isolated historical fixture producer, not runtime.

Own event_storage build job_1748f2b2d001W483hUHO3v4io4 finished successfully after~30min. No own Cargo build currently active. User foreground cargo build --bin neoism PID511217 at last check, root target/debug, don't stop it. Its turso_core0.8.3 and fff_search compiled; do not launch competing full debug build. Next pipeline generate official0.6.1 synthetic fixture (new output path, helper refuses existing), rebuild latest server test binary once, then reuse binary filters. Planned migration helper producer command in tools/fts-v1-fixture/README.md.
