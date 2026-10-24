---
name: "Saved inactive systems and Active switch"
description: "New systems save deactivated with no SSH; durable enabled intent and real Active switch; safe guarded off, legacy records preserved enabled."
type: "feature"
scope: "project"
origin: "neoism-agent"
created: "2026-10-23"
updated: "2026-10-23"
---

# Saved inactive Systems and Active switch

Systems can be registered without connecting. Add system uses Computer (hostname/SSH alias), button Add system, and explains that new entries start deactivated. SystemRequest::Add only upserts and refreshes; no SSH/bootstrap/descriptor discovery or activation, including duplicates. There is no Add-form switch; activation switch is in the Systems list, avoiding existing server/Fast-specific form-switch handlers.

SavedSystem.enabled is durable intent, separate from live Connected/Connecting/Disconnected. Missing flag in existing systems.json defaults TRUE for compatibility; new recipes explicitly start FALSE; duplicate upserts preserve state. SystemRegistry::set_enabled locks, reloads and atomically persists before publishing cached state. Failed writes/corruption do not overwrite data or requested state.

Remote rows have existing-style 40x22 Active track/16px knob and separate Remove. PaletteSystemEntry.enabled is host-supplied; local is always true with no switch/removal. Disabled rows display Deactivated even with a stale Connected snapshot. Idempotent SetSystemEnabled{id,enabled} action queues SystemRequest::SetEnabled{id,enabled,discard}. Row/Enter activates inactive entries; enabled rows retain retry. Switch hits/hover/pointer use clipped bounds; refresh preserves source selection/query.

Activate persists true before background connect; connection error leaves enabled intent true. Plain Connect rejects disabled recipes. Deactivate performs all-window unsaved preflight before persistence/transport changes; SystemDeactivateConfirm retains intent through confirmation, and cancellation leaves connection/flag unchanged. Persist false successfully before guarded teardown, cancel late workers, revoke source credentials and retain recipe. Disabled recipes cannot dial, accept completions or retain/publish Agent sources. No workspace activation, remote daemon shutdown or remote file deletion. Existing leave helper may retain workspace/tunnel when creating safe local replacement fails; intent stays false and owner sources are removed by pump, prioritizing workspace safety.

WASM/web activation routes to the honest desktop-only explanation, never fake state or hosted server authority. Legacy active records remain enabled; deactivated profiles stay listed for later activation. Adding a laptop offline requires only a syntactically valid host/alias, not a reachable machine or SSH authentication.

Verification: 120 palette tests and 12 registry tests passed; wasm32 renderer check, web typecheck and scoped diff checks passed. Full native checks/system tests were attempted but blocked by concurrent image/session-preload and editor initialization edits outside this feature; latest diagnostic was missing CodePane.diagnostic_counts initializer in shared/editor/code/types.rs. Those changes were not modified/reverted. No release build or live GUI/device actions.
