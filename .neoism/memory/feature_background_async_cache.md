---
name: "Wallpaper cache + current background-first startup policy"
description: "Async image preparation + bounded RGBA cache; CURRENT policy waits for picture before first reveal. Warm Wayland~0.575s, cold~3.33s; blur default0."
type: "perf"
scope: "project"
origin: "neoism-agent"
created: "2026-10-19"
updated: "2026-10-19"
---

# Wallpaper preparation and persistent cache

CURRENT POLICY: first reveal waits for the configured picture. The user explicitly rejected the earlier paint-first/show-wallpaper-later behavior. Keep async preparation and disk caching, but never show the initial window before a successful wallpaper frame when the image loads successfully. See feature_background_first_frame_reveal.md for the reveal gate and platform details. Blur remains OPT-IN, default0; the user's personal original49610.jpg has explicitblur3 andopacity0.35.

## Implementation

sugarloaf/src/sugarloaf/background_loader.rs owns one native worker per Sugarloaf, a Mutex/Condvar latest-pending slot, and bounded completed channel1. All decode/blur/opacity/cache I/O happens on the worker. Source identity is checked around processing/cache I/O, retrying once if replaced. Cache key is canonical path + source identity + blur/opacity bits + version; excludes fit/position. Versioned raw RGBA header, exact length/checksum, checked allocation <=256MiB and dimensions <=16384. Corrupt/unwritable cache falls back to source. Atomic0600 temp+rename; best-effort pruning64entries/256MiB on writes only. Desktop root is dirs::cache_dir()/neoism/backgrounds, with background-rgba-v1 subfolder. NEOISM_BACKGROUND_CACHE_DIR is a developer/test override. Keep cache outside the config watcher tree.

Sugarloaf::request_background_image queues; poll_background_image installs pixels on the UI/render thread. Generations reject superseded results. Clear invalidates even before first install. IMPORTANT retain/reuse the worker on clear via cancel_pending: dropping/recreating on rapid toggles accumulates detached blur threads. Drop stops after running work without joining/blocking UI teardown. Initial request does not stat/read the source on UI; installed-image layout-only fast path is preserved.

background_image() returns installed properties only; pending_background_image() exposes separate intent; background_image_ready() requires installed and no pending. Picker/pack transactions retain synchronous setter error semantics. Both snapshot installed+pending separately; Screen::rollback_background_image restores installed (clears rejected new image on restoration failure), then requeues prior pending asynchronously, surfacing errors. Never snapshot pending as installed: rollback to missing pendingA could otherwise leave rejectedB visible.

Desktop startup/hot reload queue using the screen helper. Worker callback sends RioEvent::Render. Shared Screen polling runs from the common redraw path before the one-shot startup presentation gate, including Welcome. Initial hidden-window wakes bypass ordinary focus/occlusion suppression; do not busy-poll while waiting. Existing visible wallpaper stays during subsequent reload. Gate failure/no-image/clear permits normal/error fallback instead of hiding forever.

## Measurements and test isolation

User authorized debug builds/GUI launches, not release builds. Isolate NEOISM_DEV_INSTANCE, NEOISM_CONFIG_HOME, NEOISM_IPC_SOCKET and --app-id/--title-placeholder; NEVER --new-window because it forwards. Debug helper isolation does not isolate GUI IPC/config. Benchmark only own windows; close via hyprctl pid:ownPID and leave live sessions alone. Script/private fixtures are /tmp/neoism-background-bench/measure.py.

Historical median3 debug launches, same1920x1200 originalJPEG/35%opacity/font/shaders: syncblur0 1256.30ms, syncblur3 3265.77ms, old preblurPNG/blur0 927.47ms. Thus blur added ~2009ms. Earlier async-ungated first usable frame ~532ms with coldblur3, wallpaper3213ms; warm first524ms/wallpaper559ms. That behavior was deliberately superseded by the user's no-naked-window requirement.

CURRENT gated final debug smoke: Wayland coldblur3 firstframe=wallpaper3325.67ms, reveal3325.69ms, cache_hit=false. Wayland warm firstframe=wallpaper574.90ms, reveal574.95ms, cache_hit=true, worker58.36ms. X11 warm firstframe=wallpaper1047.33ms, reveal1047.45ms, cache_hit=true, worker57.40ms. Both first_frame_background_ready=true and actually mapped in compositor. Invalid image fallback revealed542.33ms, no-image529.20ms. These are application render/reveal log timings plus compositor mapping checks, not photon measurements or release-build benchmarks. The normal cache for the user's current image is primed.

Verification before reveal:38 focused tests passed. Final reveal iteration:2 desktop gate tests +2 X11 hidden-redraw tests +21 Sugarloaf background tests=25 passed, plus debug build, physical Wayland/X11 smoke/failure/no-image tests, wasm web check and diff check. No commits/pushes.
