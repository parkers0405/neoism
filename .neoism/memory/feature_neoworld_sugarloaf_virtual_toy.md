---
name: "NeoWorld Sugarloaf virtual toy"
description: "NeoWorld is a Sugarloaf-native GPU virtual toy with fixed logical pixels, deterministic spawn-generated rooms/appearance, and autonomous object-directed pet AI; multiplayer follows after the clean single-player base."
type: "feature"
scope: "project"
origin: "neoism-agent"
created: "2026-08-17"
updated: "2026-08-17"
---

# NeoWorld Sugarloaf virtual toy

NeoWorld targets the composition and autonomous room life of Radica Cube World, but is not a literal LCD clone. Its visual identity is Neoism's Sugarloaf-native GPU GUI: fixed logical pixels, crisp themed geometry, deep OLED-friendly backgrounds, and generated terminal-machine rooms.

## Architecture

- `neoism-neoworld-core/src/lib.rs` owns deterministic simulation and spawn-derived generation.
- World coordinates are fixed at 160x120 with floor y=108, independent of pane size.
- `RoomPlan::from_id(PetId)` deterministically generates room style, wall pattern, shelf, and three stations. Every room includes a bed plus generated activity objects.
- `PetState::appearance()` deterministically generates head, accessory, and build variants from spawn identity.
- Autonomous activities include idle, wander, walk-to-station, play, exercise, rest, tinker, observe, and sulk.
- Needs evolve over time; tiredness forces sleep, loneliness influences social substitutes, and temperament weights preferred stations.
- `neoism-frontend/shared/src/panels/neoworld.rs` scales the fixed world into a centered Sugarloaf device display, maps pointers back to logical coordinates, and draws rooms/stations/pets with rectangle-based pixel geometry.
- Existing grab/throw/poke/persistence behavior remains.

## Direction

Finish the clean single-player room/AI foundation before multiplayer. Later multiplayer should add cube adjacency, pet transfer, and cross-room social behavior without replacing the deterministic core.
