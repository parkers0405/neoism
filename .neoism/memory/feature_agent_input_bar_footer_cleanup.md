---
name: "Agent input footer cleanup"
description: "Input bar: no plus/lightning, submit shares44px chip row, display capitalization,10px corners; total heights unchanged and24 tests passed"
type: "feature"
scope: "project"
origin: "neoism-agent"
created: "2026-10-22"
updated: "2026-10-22"
---

User asked native agent input bar: remove plus, put send arrow on same row as agent/model/thinking, capitalize model/provider and thinking labels (Openai/Gpt, High), remove lightning, less-rounded Apple-like shape. Implemented in shared panels/agent_pane/view/user_input.rs (desktop reexports this renderer): removed plus painting and registration, leaving per-frame clear_usage_chip_rect so no invisible hotspot; removed lightning painting/animation/17px advance. New ComposerFooterGeometry / composer_footer_geometry shares row center for chips/send visual/touch targets and reserves full submit hit rectangle plus8px gap before chip sizing. Footer CHIPS_BAND_H=44 so mobile44px target fits inside island, no text/help overlap. Existing send/stop painted sizes26/30, action behavior, independent radius8 retained; island radius10 home/chat. Renderer-only capitalize_chip_label uppercases first char of slash/space-separated segments, preserves remaining case/acronyms; applied to model/thinking and external option display, raw IDs still used for provider logo/picker/API. model_chip_label returns owned display String, narrow still terminal nonempty model segment. composer_text_padding(home)=(15,25), chat=(11,21); layout derives base from paddings+44 footer, so overall min heights106/98 and per-row22 remain unchanged. Text viewport now excludes image rail and balances actual text paddings rather than old separate internal send band. Changes also in view/layout.rs and comment in view/mod.rs. Verified 19 composer policy/geometry/capitalization/text+image sizing tests and5 layout tests passed; cargo check -q -p neoism --tests passed; scoped diff --check clean. Prior timer initial-hidden and reply suffix removal retained.
