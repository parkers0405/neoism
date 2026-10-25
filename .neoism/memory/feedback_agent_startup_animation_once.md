---
name: "Retain the agent startup animation without double playback"
description: "Keep Alt+A's startup scramble; one shared 320ms clock with loading, never replay after data arrives"
type: "preference"
scope: "project"
origin: "user"
created: "2026-10-22"
updated: "2026-10-22"
---

# Keep the new agent tab chip animation, once

User explicitly wants the existing agent/model/thinking label scramble animation retained for a fresh Alt+A tab. Do not remove the animation merely to improve perceived loading speed. Avoid playing a separate second animation after real info loading.

Implementation rule: `ConfigChipHydration` uses one `requested_at` clock for loading and the 320ms reveal. Cached defaults reveal once. Fast requests reveal only for the remainder of that initial 320ms window. Requests already loading beyond 320ms stop animating when complete, without starting another reveal. Repeated completion/catalog replies cannot restart the clock. Failure and opening an existing conversation cancel the draft animation. Preserve the asynchronous data/caching performance fixes.
