---
name: "NeoUI: implementation over test volume"
description: "NeoUI build: user wants implementation/completion, not repeated test suites/reviews; brief compile checks only unless necessary"
type: "feedback"
scope: "project"
origin: "explicit user correction"
created: "2026-10-25"
updated: "2026-10-25"
---

# Prioritize framework implementation over test volume

During the NeoUI framework build the user explicitly said: "stop doing so many freaking test work on getting this done". They want the full clean GPUI-like framework, not repeated small milestones dominated by test writing/running. Prioritize production APIs, integrated migrations and completing the architecture. Use brief compile checks to catch actual errors; don't add large new regression suites, run the full shared suite repeatedly, or spend multiple subagents on reviews/tests unless requested or a concrete blocker requires it. Keep honest about unverified behavior and don't claim parity based on compilation.
