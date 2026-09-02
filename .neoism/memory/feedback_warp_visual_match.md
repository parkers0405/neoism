---
name: Match Warp visuals literally — read the screenshot, don't infer
description: When mimicking Warp's UI, only paint chrome the screenshot actually shows; absence is signal
type: feedback
originSessionId: a22dfe78-6308-4520-8808-47ad28a168f6
---
When matching Warp visual styling, scrutinize the screenshot for what is **absent** as much as what is present. Don't add divider lines, borders, or accents that aren't visible in the reference image just because "it'd help separation."

**Why:** I added a 1px top divider on the bottom composer because Warp's source has an optional `Border::top(1px)` — but in the screenshot the user shared, the divider is off (it's gated behind a `show_block_dividers` setting). Painting it anyway looked wrong and made the user (rightly) angry.

**How to apply:** Before adding any chrome line/border/fill that the user has shown me a reference for, verify that line is actually visible in their reference image. When in doubt, omit. Subtractive design beats additive when matching a target.

Also: I have the Warp source cloned at `/home/parkersettle/projects/warp` — I should read it before guessing at visuals, and check feature flags / settings that gate the visual.
