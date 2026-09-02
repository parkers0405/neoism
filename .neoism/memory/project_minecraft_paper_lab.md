---
name: "Neoism Minecraft Paper lab"
description: "Isolated localhost-only Paper 26.2 migration lab with NeoismCore and validated support stack"
type: "project"
scope: "project"
origin: "session"
created: "2026-07-29"
updated: "2026-07-29"
---

On piss-desktop (Tailscale 100.93.155.59), isolated Paper migration lab lives at `/home/parkersettle/Docker/Minecraft/neoism-paper-lab`. It is separate from production `/home/parkersettle/Docker/Minecraft/neoism`, restores backup `world-20260729-084313.tar.gz`, and binds only loopback: Paper diagnostic TCP 25576, Velocity Java TCP 25577, Geyser Bedrock UDP 19152, voice UDP 24464. Stack: Paper 26.2, Velocity modern forwarding + lab-only secret, Geyser/Floodgate, Simple Voice Chat (optional clients), LuckPerms, PlaceholderAPI, isolated Neoism agent/gateway. NeoismCore source is under lab `src/neoism-core`, JAR in `data/plugins/NeoismCore.jar`, SQLite in `data/plugins/NeoismCore/neoism.db`. Core provides teams/team chat/friendly fire, 3s TPA movement/damage cancellation, diamond ledger, pay/deposit/withdraw, escrowed bounties, death tax, repeat/team-kill abuse controls, kill/death stats, leaderboards, Java TAB, Bedrock-safe commands, `/neoism`, and PlaceholderAPI fields. Legacy Fabric scoreboard + owner properties import is idempotent; snapshot imported 1 team / 4 memberships. Five Store tests pass. Gateway authenticated health returned HTTP 200. Production containers and Playit remained unchanged/healthy. README in lab documents operation and cutover gates. Still requires real Java and Bedrock client validation of UUID/inventory/ender chest/advancement continuity before production cutover.
