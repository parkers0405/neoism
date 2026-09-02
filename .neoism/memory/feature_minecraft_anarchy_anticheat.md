---
name: "Minecraft anarchy anti-cheat"
description: "Live GrimAC/Floodgate/Paper anti-cheat baseline and anarchy-friendly policy"
type: "feature"
scope: "project"
origin: "Paper 26.2 production deployment"
created: "2026-07-29"
updated: "2026-07-29"
---

# Minecraft anarchy anti-cheat baseline

Deployed to Paper 26.2 production on 2026-07-29.

## Live stack
- GrimAC `2.3.74-7ae1d8f`, exact Modrinth 26.2 Bukkit artifact; SHA-512 `05f750f2b2544cbd7d5d89c30cc81a8bd15c7d858b2a1e4ace1d2ab8ac1d0eed6daaa93d64205a6d9ece1e6309f2c0d7695356d82b8dc3eb774ac5a19c09963e`.
- PacketEvents `2.13.0` shared by Grim and NeoismCore team locator.
- Floodgate `2.2.5` build 138 installed on Paper in addition to Velocity; Velocity `send-floodgate-data: true`; matching private key copied securely to Paper Floodgate folder.
- Paper and Velocity healthy after restart; Geyser and MCXboxBroadcast recovered normally.

## Enforcement posture
- Grim defaults are retained: impossible movement gets setbacks; sustained simulation/knockback violations can kick; alerts/history/logging enabled; no automatic bans.
- Intended to block Java fly, Jesus/water walk, speed, timer, nofall, reach/combat and malformed movement/packet behavior.
- Geyser/Floodgate players are recognized and exempted from Grim Java simulation because Bedrock physics cause false positives. Paper native checks/packet limits still apply.
- `allow-flight=false` remains set.
- Paper native packet limiter: all packets KICK at 500/sec over 7 sec; place_recipe DROP at 5 over 4 sec; incoming spam threshold 300.

## Policy
- Allow utility clients/mods like minimaps, freecam, inventory helpers, schematics unless they generate impossible server-side behavior.
- Disallow impossible movement, combat automation/physics violations, packet/crash attacks, dupes, and intentional lagging.
- Do not stack multiple general anti-cheats.
- AnarchyExploitFixes is the preferred future exploit-specific complement, but its published artifact did not advertise 26.2 during deployment. Do not install until a 26.2 release exists and passes staging with Geyser, auctions/item serialization, portals, and redstone.
- ExploitFixer, IllegalStack, Panilla, and AnarchyExploitFixes overlap; choose one tested exploit layer rather than stacking all.

## Operations
- `/grim alerts` toggles operator alerts.
- `/grim history <player>` shows stored evidence.
- Production README documents policy and compatibility reasoning.
