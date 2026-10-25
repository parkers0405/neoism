---
name: "Activity timer hidden until started"
description: "Hide activity timer suffix until model elapsed is positive; keep Crafting and dots visible while initializing"
type: "bug"
scope: "project"
origin: "neoism-agent"
created: "2026-10-22"
updated: "2026-10-22"
---

User explicitly requested not showing the Crafting/Pondering timer at all until it starts, rather than displaying 0.0 during initialization. In shared agent pane view/user_input.rs render_streaming_status_row, draw the timer suffix only when `elapsed > 0.0`; animated activity label and dots plus queue/background rows remain visible unchanged. This pairs with all-desktop admission/model-time fix in bug_native_model_timer_init_reset.md. `cargo check -q -p neoism-ui` and scoped git diff --check passed.
