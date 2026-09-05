---
name: "Web overlay/splash/Notes sync fixes"
description: "Unified composer overlay ownership, typed mobile splash intents, and durable generation-bound Notes sync"
type: "bug"
scope: "project"
origin: "user task + implementation"
created: "2026-08-05"
updated: "2026-08-05"
---

# Web overlay/splash/Notes ownership fixes

Implemented 2026-08-05.

- Shared Chrome now centralizes `generic_keyboard_overlay_active` and `terminal_composer_eligible`. Layout, composer painting/input, wasm prompt-row removal/reserved rows, and trail cursor use effective eligibility. Configured shell visibility survives overlay ownership and restores on close. Status remains at physical bottom while keyboard inset only limits content.
- SplashOverlay publishes paint-time active state plus typed `SplashMenuAction` hits through WASM (`splash_active`, `splash_action_at`). Mobile touch treats active splash as a boundary: background/gaps/below rows are inert; Change Directory/Palette/Search anticipate their typing overlay and retain trusted-touch focus; Tree/Notes/Agent/New Terminal do not request keyboard; only clean touchend activates.
- Web Notes uses durable desired state keyed by adapter, normalized vault, workspace, and ProtocolClient generation. Dirty survives failures; retries coalesce with capped exponential delay and only run after connected workspace hydration barrier. Adapter install, reconnect, opening Notes and same-root replay ensure snapshots. Stale results reject. Recursive collection publishes all-or-nothing. request-zero Files Changed refreshes only the active vault. App replays active vault before rehydrate refresh and preserves cached vault if reconnect tree lookup is not ready.

Verification: cargo check neoism-ui; cargo check neoism-terminal-wasm wasm32 web; cargo test neoism-ui --lib (2327); cargo test neoism-terminal-wasm --lib (8); web tsc; full web tests (175); focused mobile/Notes tests (31); git diff --check.
