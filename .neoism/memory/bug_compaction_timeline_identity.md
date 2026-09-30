---
name: "Compaction timeline identity — FIXED"
description: "Compaction marker/text/deltas share assistant identity and kind across live, WASM, and hydration timelines"
type: "bug"
scope: "project"
origin: "2026-09-30 implementation"
created: "2026-09-30"
updated: "2026-09-30"
---

Compaction persistence is one hidden user marker plus one assistant message containing a `CompactionPart(summary:true)` and synthetic text part. The live projection must never flatten those assistant parts into separate Compaction and Assistant rows.

Fix: `session_context` marks compaction text snapshots with `compactionSummary:true` and compaction deltas with `partType:"compaction"`. Shared `part_block` maps both marker and text snapshot to `NeoismAgentMessageKind::Compaction`, keyed by the assistant `messageID`. `ContentKind::Compaction` preserves that discriminator through workspace-daemon/WASM, where compaction deltas use assistant message identity rather than synthetic text-part identity. Desktop and shared fallback delta creation also create Compaction rows.

Hydration invariants: Compaction is a streamed live kind and longer live summary text wins over an empty/older stale snapshot with the same assistant ID. Ordered part snapshots still replace text. Failed compactions render the terminal error rather than an empty compaction card. Duplicate compaction-ended events may only transition `Compacting -> Idle`; they must not idle a newly generating response.

Expected visual order for proactive auto-compaction is `trigger user -> one compaction card -> normal answer`. Queued prompts remain server-serialized and were not the source of corruption.

Top-left AI/Conversations glyph behavior: its click action is `TopBarAction::OpenAgent`, not `ToggleConversations`. This intentionally uses the same host path as Alt+A: open/create Agent, show Conversations, and leave the composer focused.

Verification: 31 server compaction tests, 12 shared compaction tests, 7 desktop compaction tests, protocol roundtrip, managed daemon compilation, Agent-button hit test, native check, wasm32 check, web TypeScript typecheck, rustfmt check, and diff check pass. The workspace-daemon lib test binary has an unrelated pre-existing compile blocker because `agent/tests.rs` calls private `todo_items_from_response`; normal daemon compilation passes and the new bridge regression is present.
