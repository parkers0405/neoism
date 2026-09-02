---
name: "Minecraft chat spam + dupes"
description: "Live public/team chat limiter and audited Paper anti-dupe settings"
type: "feature"
scope: "project"
origin: "Paper 26.2 production audit and deployment"
created: "2026-07-29"
updated: "2026-07-29"
---

# Minecraft chat spam and dupe baseline

Audited/implemented on Paper 26.2 production 2026-07-29.

## Chat spam
NeoismCore now applies one shared limiter to public `AsyncChatEvent` and `/tc` team chat:
- Burst capacity 4 messages.
- One token refills every 1500 ms.
- Whitespace/case-normalized duplicate blocked for 10 seconds.
- Four blocked attempts in a 30-second window trigger a 30-second chat-only mute.
- No kick or ban; other commands remain usable.
- `neoism.chat.bypass` defaults to operators.
- Player receives clear slow-down/mute duration message.
- `ChatLimiterTest`: 3 tests pass; total NeoismCore suite 19 tests (13 Store, 3 ChatLimiter, 2 AuctionFilter, 1 AuctionTime).

Additional layers:
- Paper native spam limiter: incoming threshold 300, recipe limit 20, tab limit 500.
- Paper packet limiter: all packets KICK at 500/sec over 7 sec; place_recipe DROP at 5 over 4 sec.
- Velocity: command rate 50/s, tab-complete 10/s, kick-after-rate-limited-commands 0, login rate limit 3000 ms.

## Duplication coverage
Paper production settings are already hardened:
- `allow-piston-duplication: false` (TNT/carpet/rail and related piston/portal/sand families).
- `allow-headless-pistons: false`.
- `allow-permanent-block-break-exploits: false`.
- `allow-unsafe-end-portal-teleportation: false`.
- `skip-tripwire-hook-placement-validation: false`.
- Oversized item components sanitized (max 1024 KiB); books, titles, lore, display names, recipes bounded.
- Duplicate entity UUID handling enabled and packet/spam limits active.

Boundary: Grim is not an inventory dupe detector; Paper blocks known native/mechanical families but cannot promise unknown zero-days. No extra exploit plugin installed because AnarchyExploitFixes does not advertise 26.2 and other products have vague compatibility/overlap. Stage one targeted exploit layer only after explicit 26.2 support and testing with Geyser, auctions/components, portals, containers, and redstone.

Deployment healthy; NeoismCore, Grim, PacketEvents, Floodgate loaded. The Grim SLF4J lines are upstream no-provider warnings emitted as STDERR, not startup failures.
