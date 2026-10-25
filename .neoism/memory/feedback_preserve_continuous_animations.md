---
name: "Keep continuous animations and effects intact"
description: "User explicitly forbids idle-freezing, disabling, throttling, or lowering animation/effect quality as a frame-rate workaround"
type: "feedback"
scope: "project"
origin: "neoism-agent"
created: "2026-10-23"
updated: "2026-10-23"
---

# Preserve continuous animations during performance work

User explicitly said: do not make animations stop animating while they sit on the screen; they like the effects and do not accept disabling them as a performance fix. Preserve continuous idle animation, effect quality, rates, timing, and shader behavior. Do not reduce redraw demand by hiding animation owners, pause effects on inactivity, cap their FPS, or simplify/remove visual effects to mask unrelated stalls. Target filesystem work, queue bursts, cache churn, unnecessary allocations/locks, and correct GPU synchronization instead. If an effect itself needs optimization, preserve equivalent rendered output and continuous motion.
