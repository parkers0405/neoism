---
name: "project_cef_artifact_runtime_spike"
description: "2026-10-11 CEF windowless spike PASSED; decision = replace Servo with CEF worker for live inline html artifacts (T3 parity)"
type: "project"
scope: "project"
origin: "neoism-agent"
created: "2026-10-12"
updated: "2026-10-12"
---

User decision 2026-10-11: wants LIVE interactive inline HTML like T3 (not screenshots/CDP), fine with far-out arch. Servo judged wrong engine (compat, no CDN, 10min builds, 21GB target). Chose CEF via tauri-apps/cef-rs (v154.5.0, cloned /tmp/cef-rs; spike /tmp/cef-spike). Prebuilt CEF downloaded to ~/.local/share/cef (1.5GB; export with `cargo run --release -p export-cef-dir -- --force $HOME/.local/share/cef`; build needs CEF_PATH + LD_LIBRARY_PATH to that dir; copy pak/bin/dat/locales next to binary). Spike results headless CPU on_paint, --disable-gpu --no-sandbox: init 277ms, first paint 58ms, 60.2fps steady with rAF, click->next paint 6.7ms, JS handler ran, grid/SVG/gradient dashboard rendered correctly (matches Chromium). Spike pattern: execute_process then initialize(windowless_rendering_enabled, no_sandbox), browser_host_create_browser_sync with WindowInfo windowless, wrap_render_handler view_rect/screen_info/on_paint (BGRA), do_message_loop_work loop, host.send_mouse_click_event. Plan: neoism-cef-runtime worker speaking existing neoism-interactive-artifacts IPC (reconcile/resize/input/frames) so desktop bridge screen/bridges/html_artifacts.rs + overlay path reuse; then T3 parity: html_preview tool (screenshot+console+contentHeight), html_variants card strip, fuller theme vars (T3 ~40: secondary, ring, accent-surface, code-background...; fix --accent mapped to palette.hover), T3-style layout guide, allow CDN. Open: sandbox must be ON before ship, CPU frame copy perf in desktop unmeasured (GPU shared-texture path is fallback), bundle/download-on-first-use.
