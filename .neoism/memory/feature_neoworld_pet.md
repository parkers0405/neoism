---
name: feature-neoworld-pet
description: "NeoWorld pet = authentic Radica Cube World bitmap frames; anatomy extracted from real sprite rips, not guessed from photos"
metadata: 
  node_type: memory
  type: project
  originSessionId: 29bb43eb-6a08-4f99-a081-a746a8d5c4d4
  modified: 2026-08-18T04:31:24.427Z
---

NeoWorld pet pane (`neoism-frontend/shared/src/panels/neoworld.rs`) renders the pet as 66 16x22 `&[&str]` bitmap frames (run-length drawn via `draw_frame`, mirrored per-row for facing). Anatomy copied from the real Radica Cube World sprite: solid ~4x5 head fused straight onto a shoulder wedge (NO neck, NO eyes/face), 2px torso with 4px hip flare, thin arms tapering to 2px blob hands, splayed 2px legs with 3px feet, ~21px tall in the 160x120 LCD world.

Behavior AI lives in `neoism-neoworld-core`: `CritterStyle` (24 styles — stroll/march/skip/sneak/moonwalk/zoomies wander flavors, dance/robot/air-guitar/juggle/spin play, boxing/pushups/squats/jump-rope exercise, hammer/read tinker, gaze/water-plant/bird-watch observe, fume/kick-ground sulk) picked by `weighted_flavor` tables scaled live by emotions (excitement/tiredness/loneliness/irritation) + temperament + station kind; `Moment` one-shots (sneeze/shiver/hiccup/stare-at-viewer/trip-while-walking/dizzy-after-hard-landing) roll on a cooldown in `step()`, freeze locomotion while active; zoomies run 2.4x, sneak 0.45x via style speed factors. Renderer maps style/moment → clips in `draw_stick_pet` with physics overrides first (grab dangle, fall flail, LAND squash, turn flash), plus LCD effects: notes, juggle balls, speed lines, sweat, sparks, water drops, sneeze burst, hiccup "!", orbiting dizzy stars, bird flying across during bird-watch, blinking Zzz. Sleep lies on the bed platform (`y -= 7`). Moonwalk draws with facing flipped; spin alternates front/profile frames with mirroring.

**Why:** First attempt (2px-thick limbs, big octagon head with eye pixels) was rejected hard — the user pointed out the real toy figure is a thin stick figure. Ground truth came from the ripped sheet at spriters-resource.com/lcd_handhelds/cubeworld/ (asset 505284; download `/media/assets/485/505284.png` with browser UA + referer, WebFetch 403s) — dumped frames to ASCII via `magick ... txt:-` to get pixel-exact anatomy.

**How to apply:** Never redesign the pet figure from memory or a low-res product photo — re-fetch the sprite sheet and diff pixels (see [[feedback-warp-visual-match]]: read the reference literally). Frames must stay exactly 16 cols (mirroring depends on it; test `every_pet_frame_is_a_uniform_16x22_bitmap` enforces this). New poses: copy the anatomy rules above, preview via a scratch ASCII rasterizer before wiring in.
