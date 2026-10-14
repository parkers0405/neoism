---
name: "Conversation load animated through history — fixed"
description: "History/cache geometry was mistaken for live growth; separate live-arrival provenance from viewport initialization and spring ownership"
type: "bug"
scope: "project"
origin: "neoism-agent"
created: "2026-10-08"
updated: "2026-10-08"
---

# Conversation load animated through history — fixed

Related: [[feature_smooth_stream_follow]]. The Wheel/FollowBottom spring previously classified any measured extent increase after `timeline_viewport_rect = Some` as live growth. An empty/provisional first paint established that marker; first-history hydration only cleared it once, and warm cache restore started with cached/stale heights. Later history hydration, estimated-to-exact measurements, and cache refreshes therefore added the entire historical extent as FollowBottom lag, producing the visible top-to-bottom movie.

Shared `state/timeline.rs` and desktop `pane/timeline.rs` now keep live-arrival provenance (`timeline_live_growth`) separate from both viewport initialization and spring ownership. Nonempty live deltas/upserts enable it; changed history, pagination, resize/reflow, explicit gestures/jump, and session navigation clear it. Navigation resets provenance instead of caching it, so hidden background live appends reopen as history at offset zero. History/layout growth cannot start a follow spring or add lag to an existing one; existing live lag and shrink continue settling, preserving the idle final frame. No blanket `history.loading` guard: a genuine live initial reply can overflow smoothly while history is still being fetched.

Shared `apply_history` compares the reconciled transcript before attributing it to history. Desktop root Messages uses its existing structural/dirty diff, with matching ChildMessages and active SessionPreloaded handling: identical snapshot echoes retain pending live provenance (delta -> idle/snapshot -> paint must still smooth); changed history removes it. First authoritative hydration still clears viewport initialization once. Warm cache restoration already opens bottom, intentionally discarding cached historical scroll/lag; that existing behavior is preserved.

Regression tests in shared state/tests.rs and desktop pane/tests.rs cover empty/small first measurements, multiple authoritative/layout refinements (asserting the render scroll-top equation before ticks, offset zero and no FollowBottom target), cache return with parked live lag and hidden appends, paging refinements, live overflow while history is loading, idle echo before final live measurement, history while an existing spring settles, and explicit jump followed by larger refined geometry. Existing stream tests now ingest actual live deltas rather than pretending geometry-only growth is live. Manual historical reader fixtures use the scroll API so their follow latch matches real user gestures. No GPU or tool-motion/card changes.
