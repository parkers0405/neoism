---
name: "Minecraft PvP cash + team locator"
description: "Deployed PvP cash transfer/burn, team-only vanilla locator bar, and improved /pay gifting"
type: "feature"
scope: "project"
origin: "Neoism Paper 26.2 production implementation"
created: "2026-07-29"
updated: "2026-07-29"
---

# NeoismCore PvP cash, team locator, and gifting

Implemented and deployed to Paper 26.2 production on 2026-07-29.

## PvP cash settlement
- Eligible PvP kills expose 10% of the victim balance above a protected first $100.
- Total loss is capped at $500 per death.
- Killer receives 80% of the loss; 20% is permanently burned.
- Environmental, self, teammate, and repeat killer/victim deaths do not debit money.
- The old separately minted $25 kill reward was removed from the live event path and config.
- Debit, killer credit, burn ledger entries, bounty payout, and cooldown record happen in one SQLite transaction.
- Ledger kinds are `pvp_cash` and `pvp_burn`.

## Team locator
- Existing `/team` system remains authoritative; production verified one legacy team and four memberships.
- Vanilla `locator_bar` gamerule remains enabled.
- PacketEvents 2.13.0 is installed server-side.
- NeoismCore filters outbound WAYPOINT packets against a once-per-second in-memory cache of Neoism team membership.
- Only players sharing the same Neoism team can see each other's locator indicator.
- A player with no team sees no other player indicators.
- No Java or Bedrock client mod is required.

## Money gifting
- `/pay <player> <amount>` remains an atomic existing-money transfer with overdraft protection.
- It now rejects self-pay, gives the sender a named confirmation, and notifies an online recipient.

## Verification
- 16 tests passed: StoreTest 13, AuctionFilterTest 2, AuctionTimeTest 1.
- Production healthy after restart; NeoismCore and PacketEvents loaded.
- SQLite integrity check `ok`; supply unchanged at $152.
- Pre-deployment DB backup: `data/plugins/NeoismCore/neoism.db.pre-pvp-cash-20260729-210810`.
- Source and README synced to production.
