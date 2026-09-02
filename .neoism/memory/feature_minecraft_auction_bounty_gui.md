---
name: "Minecraft auction and bounty GUIs"
description: "NeoismCore auction and bounty cross-edition chest GUIs plus TAB filtering"
type: "feature"
scope: "project"
origin: "session"
created: "2026-07-29"
updated: "2026-07-29"
---

Paper production NeoismCore now includes server-side cross-edition chest GUIs. `/ah` opens a 6-row auction browser; `/ah sell <price>` escrows full ItemStack serialization (NBT/enchant/name/lore) in SQLite; left-click buys, seller right-click cancels, My Items or `/ah claims` claims sold/cancelled/expired items exactly once. Listings expire at 48h, max $1m. Purchases credit seller atomically and buyer claims item separately from durable escrow. `/bounty` opens player-head GUI sorted active bounty then balance/kills; cards show bounty, balance, K/D; click prints `/bounty add player amount` because cross-edition chest GUI has no safe number text entry. Geyser translates both standard server inventories for Bedrock; no client mod. TAB query now filters chosen metric >0; no historical zero-kill fillers, header says no kills yet, bounty suffix only when >0. Seven Store tests pass including atomic auction sale, one-time claim, cancellation and expiry returns. Production JAR deployed and healthy; DB backed up pre-auction under NeoismCore plugin dir. Source `/home/parkersettle/Docker/Minecraft/neoism/src/neoism-core`, docs production README.
