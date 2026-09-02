---
name: feedback-keep-building
description: User wants me to keep building autonomously through multi-step features without pausing; fine iterating on runtime issues in their own dev loop.
metadata: 
  node_type: memory
  type: feedback
  originSessionId: 1b6ea9d8-d40d-43fb-bd7b-362511dac63d
---

On big multi-part feature work the user repeatedly says "keep going / do
not stop until finished." They want momentum: chain the increments,
compile as I go, and report at the end — don't stop to ask after each
piece.

**Why:** They run the app themselves and accept that GUI/runtime
behavior can't be verified by `cargo check` alone; they'll catch
runtime issues in their dev loop and we fix together. So "I can't fully
verify this without running it" is NOT a reason to pause — just flag it
and proceed. See [[feedback_build_workflow]] (I verify with cargo
check, never `--build release`).

**How to apply:**
- Keep building through the whole plan; only stop for a genuine design
  fork I can't resolve (e.g. they chose to ship the safe subset of a
  feature and defer the risky part — like deferring live-terminal
  buffer-tab migration in [[feature_workspace_detach]]).
- Prefer pragmatic scoping: ship the safe/compilable subset now, defer
  the unverifiable-risky part with a clear note, rather than stalling.
- Don't over-explain risk as a blocker; one honest line ("needs a
  runtime check in your loop") is enough, then continue.
