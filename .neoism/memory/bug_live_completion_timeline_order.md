---
name: "Live completion card splits assistant turn — FIXED"
description: "Desktop appended delayed completion cards after already-streaming assistant text; canonical parent message IDs now place late rows before later live message groups (674d9e097)."
type: "bug"
scope: "project"
origin: "neoism-agent"
created: "2026-08-25"
updated: "2026-08-25"
---

# Live completion card splits assistant turn — FIXED

Fixed on `neoism_agent_v2` in commit `674d9e097`.

## Symptom

While the model was streaming a parent response triggered by a subagent completion, the older `Subagent finished` system card appeared below already-generated assistant text and above later reasoning. One assistant message group was visually split around its triggering notification.

## Evidence

The Agent store was canonical and correctly ordered: completion user message `msg_03a800995...` preceded assistant response `msg_03a800b6c...`. The inversion existed only in the desktop live timeline.

## Cause

The provider could begin broadcasting assistant parts before the persisted completion user part reached the desktop. `upsert_part_message` appended the delayed completion row at the tail. The assistant part-to-parent-message map already supplied canonical ascending message IDs, but insertion ignored them. Reasoning normalization could not move assistant text across the newly appended user/system boundary.

## Fix

Desktop live ingestion now derives the canonical message group ID from either a `msg_` row ID or `live_part_parent_ids`, and inserts a delayed row before the first later canonical message group. Parts within one assistant message retain the existing reasoning/text normalization.

A regression test reproduces the exact order: assistant text arrives, older completion arrives, reasoning arrives; expected timeline is completion, reasoning, answer.
