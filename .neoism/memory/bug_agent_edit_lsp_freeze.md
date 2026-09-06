---
name: "Agent post-edit LSP touch versus genuine saves"
description: "Corrected OpenCode comparison: diagnostic touch uses open/change, not save; disk sync fixed, real editor didSave retained, cache freshness explicit"
type: "bug"
scope: "project"
origin: "neoism-agent"
created: "2026-09-06"
updated: "2026-09-06"
---

## Current verified behavior (supersedes earlier OpenCode comparison)
Released OpenCode touchFile sends didOpen/didChange plus watched-file notifications, NOT didSave. Earlier claims in this memory that OpenCode achieves parity by pulling diagnostics or by ensure_open+save were incorrect. Do not disable LSP or checkOnSave to address redundant agent-triggered builds.

Neoism fix: `lsp_service::touch` now explicitly reads disk when text=None, then ensure_open(Some(text)); initial touch opens and later changed disk contents send didChange. Identical snapshots stay deduplicated. Explicit live text takes precedence. Removed save_document only from diagnostic touch; `LspService::save` remains independent and sends negotiated didSave (includeText when requested). Navigation ensure_open(None) still retains live buffers. Pull and asynchronous push handling are retained, not rewritten. No watched-file notification support added by this minimal patch; not full OpenCode timing/protocol parity.

Callers: agent tool_support::touch_paths -> public touch_document, LSP HTTP touch route, explicit LSP touch tool, language_server facade. Genuine saves: desktop document.rs/code_crdt.rs -> notify_code_lsp_saved -> CodeLspJob::Save; shared/wasm editor DidSave -> daemon socket -> live_sync FIFO -> language_server::save_document. Formatting paths untouched.

Cache freshness is NOT guaranteed by touch: push-only cache may predate the edit; even empty is not proof of clean. Post-edit metadata entries now say diagnosticsKind=cached and freshness=unknown; report says cached errors may predate edit and must be verified before fixing. UI diagnostic cache is not cleared merely to hide stale results. Existing versioned-push rejection stays intact.

Regression tests in lsp_service_e2e_tests.rs: diagnostic_touch_syncs_disk_without_saving_but_editor_save_still_notifies asserts exact open/change/change/save sequence, disk reread, identical touch dedup, explicit live text, preserved hover, push clear/stale rejection; diagnostic_touch_preserves_pull_without_saving exercises advertised pull with a distinct result. Fake subprocess uses rustc, no installed LSP required.

## Async blocking invariant
Agent edit/write/apply_patch metadata builders must run synchronous LSP I/O inside tokio::task::spawn_blocking, not the async executor. Never replace real diagnostics with diagnosticsSkipped placeholders. touch_paths syncs and optionally pulls; attach_lsp_diagnostics only reads cache, avoiding a second blocking query. There is currently no push wait in touch; earlier single-wait/600ms-push claims are outdated. Initialization and optional pull can still block.

Metadata includes bounded touched + project snapshots, diagnosticsCount, diagnosticsProjectFileLimit=8, diagnosticsProjectScanLimit=200; errors-only report cap 20/file. Desktop tool diff-card footer consumes metadata diagnostics (cap 6 visible lines plus remainder). Existing footer rendering does not itself gain freshness labeling from the added fields.

.cargo/config.toml jobs=2 bounds per-invocation parallelism for subsequent automatic AND explicit Cargo in this repo unless overridden. It is not an aggregate limit across invocations, does not alter current jobs, and is not user/global config. Source fix needs normal rebuild/restart of hosting processes; no runtime/user settings or process kills required.
