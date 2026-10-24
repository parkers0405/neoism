---
name: "Stacked settings controls and wrapped descriptions"
description: "Settings labels and descriptions wrap above controls; shared native/web row heights drive scrolling, shortcuts and dropdown geometry."
type: "feature"
scope: "project"
origin: "neoism-agent"
created: "2026-10-16"
updated: "2026-10-16"
---

Settings controls now stack below wrapped labels/descriptions at every width in neoism-frontend/shared/src/panels/settings_page/view.rs. Same painter serves desktop and web; no CSS or separate native row implementation. setting_layout measures labels/descriptions (including long path tokens and explicit lines); its heights drive culling, dividers, total scroll metrics, shortcut section start, and dropdown anchors. Shortcut pills/reset controls also sit below their labels/action descriptions. Control hit rectangles intersect the viewport, and offscreen text clips never fall back to the entire content viewport. Dropdown geometry flips/shrinks within content bounds; stored menu scroll clamps via state.rs clamp_dropdown_scroll. Generic text editing scrolls the value horizontally to retain the trailing caret. Verified cargo check -p neoism-ui --tests and cargo test -p neoism-ui --lib panels::settings_page (17 passed); unrelated existing unused id warning in editor/neodraw/create.rs. No live UI launched.
