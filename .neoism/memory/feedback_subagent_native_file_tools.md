---
name: "Subagents must use native file tools"
description: "User caught child Bash/Python source edits despite working native Read/Edit; explicitly enforce native file tools in task prompts"
type: "feedback"
scope: "project"
origin: "neoism-agent"
created: "2026-10-08"
updated: "2026-10-08"
---

# Require native file tools in delegated implementation

User observed an implementation subagent using almost only Bash instead of native Read/Edit and asked whether filetools were broken. The actual follow-scroll child confirmed read, grep, glob, edit, write and memory were available; native reads had succeeded earlier. It chose shell rg/sed/cat and Python source rewrites, never attempted edit/write, and reported NO native failure. This was agent tool-choice/instruction-following failure, not demonstrated runtime breakage.

Two fresh general children successfully performed native read -> targeted edit -> read on timeline_scroll_policy.rs with no shell. User screenshot showed native Read and Edit plus proper diffcard, confirming these tools work. No apply_patch tool was exposed in those sessions; edit/write are their actual mutation family. Do not infer tool availability from a different profile or model, or confuse no apply_patch with no file mutation tools.

When delegating implementation, explicitly require native read/grep/glob, edit for targeted source changes, write only for new/full intentional replacements (or apply_patch if actually exposed). Bash is for project commands, tests and formatting, NOT inspecting files via cat/sed or mutating source via Python/shell rewrites. If native tooling genuinely fails or is missing, stop and report exact evidence rather than silently falling back. This matters for visible structured edits/diffcards, not just filesystem correctness.
