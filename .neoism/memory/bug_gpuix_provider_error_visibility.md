---
name: "GPUix provider/OAuth error visibility"
description: "Per-session persistent composer failure footer, nested OAuth/provider errors, durable history extraction and passing checks"
type: "bug"
scope: "project"
origin: "neoism-agent"
created: "2026-11-02"
updated: "2026-11-02"
---

# GPUix persistent provider/OAuth errors

Implemented on `fix/gpuix-chat-performance` after inspecting old Sugarloaf GUI via `git show`/`git grep main`, never checking out main. Legacy main had `Retrying · reason` activity above composer, fatal errors as notifications/dialogs plus persisted `Agent error` cards, OAuth sign-in retry within connect picker. It did not have a persistent fatal-error footer.

GPUix `src/model.ts` now holds per-session `sessionErrors`, independent of global temporary toast error. Live `session.error` and rejected HTTP prompts populate it; terminal failures set local streaming Idle, while admission failures preserve an existing running state. Idle/toast expiry does not clear it. Explicit dismissal, local submit or transition to a new busy run clears it. Initial history recovers latest-turn `System/Agent error` only; dismissed entries use an explicit undefined key so head refresh does not revive them.

`src/agent/session-error.ts` handles string, top-level message and nested data.message errors; ignores interruption/MessageAbortedError. Synthetic live transcript error messages MUST use role `System` (capitalized), otherwise Rust rejects MessageUpdated with unknown variant `system`.

`src/ui/App.tsx` replaces normal composer footer paint with persistent red failure strip, preserving native footer geometry (pass existing system field to force footer even with hints disabled and no cwd). Details opens original inline picker with full body and Connect (/connect), Copy error and Dismiss. Scope is the viewed conversation; draft is preserved; narrow panes retain controls. No native bridge API change required.

Shared Rust `api_mapping.rs` and `stream_events.rs` now also read nested data.message, ignore named cancellation and have focused regression tests, so persisted errors survive snapshot reload.

Verification: 19 pass / 0 fail in `bun test tests/agent-session-errors.test.ts tests/agent-transcript-sync.test.ts tests/agent.test.ts tests/agent-event-batching.test.ts`; `bunx tsc --noEmit`, `cargo check -p neoism-ui --tests --locked`, and `git diff --check` pass. Rust tests were typechecked, not executed (earlier attempts blocked by Cargo lock; later cargo check completed successfully). No release build. Tests include actual App component graph on recording renderer and native footer geometry at 240/310/800 px. App recording test launches from repository root with explicit @gpuix/solid preload, no production display/window. Concurrent display-pin edits can make default project preloader reject old display artifact; production guard left unchanged. Do not kill unrelated builds.
