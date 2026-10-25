---
name: "Use the existing debug GUI"
description: "Use the user's existing Hyprland debug window; no duplicate launches or unapproved restarts"
type: "feedback"
scope: "project"
origin: "neoism-agent"
created: "2026-10-23"
updated: "2026-10-23"
---

When the user provides an already-running debug GUI on a named Hyprland workspace, identify that exact existing process/window and use it. Do not launch replacement or duplicate debug windows. In the 2026-10-23 chat-latency investigation the user's existing debug was PID 214390 on workspace 3, with embedded agent HTTP at port 45541; these IDs/ports are ephemeral. I had incorrectly launched extra instrumented windows, and the user explicitly stopped that. Request permission before restarting their existing debug for environment-only tracing. Desktop focus/geometry changes require re-observation, not blind clicks or automatically fighting focus. The user explicitly requested cargo build/debug/self-testing for this task; debug cargo build is authorized, not cargo build --release.
