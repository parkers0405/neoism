---
name: "Multi-account provider connections GUI"
description: "Multi-account provider GUI shipped across desktop/shared/web with opaque connection IDs and strict deleted-selection behavior"
type: "feature"
scope: "project"
origin: "neoism-agent"
created: "2026-08-31"
updated: "2026-08-31"
---

Implemented desktop/shared/web multi-account provider connection UX. Account identity is an opaque dedicated `connectionId`; model IDs remain `provider/model` and labels are display-only. Secret-free summaries (`providerId`, `connectionId`, `label`, `authType`, `isDefault`) drive listing/reconciliation. Zero accounts follows existing auth, one silently selects, multiple opens an inline picker with select/add/rename/default/confirmed disconnect. Explicitly missing/deleted selections are retained and error rather than falling back. OAuth carries opaque attempt IDs and returns created connection summaries; API-key additions accept labels. Connection selection persists through session/model/prompt paths on desktop and WASM. Daemon routes manage connections and protocol messages carry IDs. Important invariant: disconnect `ConnectFinished` must send `connection_id: None`; sending the deleted target ID would make it look newly selected. Shared callback with `None` preserves the current explicit selection. Verification: UI suite 2155 passed before added regression test; focused regression passed, daemon agent tests 10 passed, protocol opaque-ID test passed, cargo checks for neoism-ui/neoism/daemon/WASM passed, diff check passed. Full daemon integration tests remain blocked by unrelated dirty tests missing `AppState.lsp_runtime`.
