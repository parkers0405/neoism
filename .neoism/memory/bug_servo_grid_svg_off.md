---
name: "bug_servo_grid_svg_off"
description: "Servo artifacts looked broken vs Chrome: layout_grid etc prefs off by default + data: image loads cancelled so inline SVG invisible; fixed in servo-runtime engine.rs, side-by-side matches Chromium"
type: "bug"
scope: "project"
origin: "neoism-agent"
created: "2026-10-11"
updated: "2026-10-11"
---

2026-10-11 side-by-side (/tmp/cmp, servo_shot.py drives worker over stdio, chromium --headless --screenshot): Servo output was bad because (1) servo::Preferences layout_grid/columns/container_queries/alpha-color/attr/progress/variable_fonts/writing_mode default FALSE -> grid dashboards stacked as blocks; now enabled in engine.rs. (2) inline <svg> is drawn by Servo via data:image/svg+xml image load; our load_web_resource cancelled it and CSP img-src 'none' -> invisible SVG. Fix: return early (DoNotIntercept) for data: scheme in both delegates + CSP img-src data:. After fix page is visually ~identical to Chromium. Engine is NOT the problem; do not switch to CEF for look. Remaining gaps vs T3: no preview tool (model blind), prompt forbids CDN/ext assets, --accent mapped to palette.hover (gray), missing --secondary/--ring/--accent-surface/--code-background etc, fixed 320px box. Plan = T3 parity: html_preview tool (screenshot+console+height), better prompt/layout guide, variants tool. Worker rebuild is incremental ~30s-2.5m (cargo build in neoism-frontend/servo-runtime).
