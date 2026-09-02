---
name: "Shared agent avatar attribution fixed"
description: "Native prompt author was null because pane identity was initialized only during rendering; fixed by installing identity at tab creation and guaranteeing a non-empty system-derived author at every native prompt dispatch."
type: "bug"
scope: "project"
origin: "neoism-agent"
created: "2026-08-24"
updated: "2026-08-24"
---

# Shared agent avatar attribution - FIXED

## Symptom
On two native Neoism desktops sharing a workspace, hovering the profile orb beside sent user messages could show `You` for a message sent by the other machine.

## Actual native root cause
Native desktops POST directly through the joined workspace `/agent` proxy. The request had an `author` field, but it was populated from `NeoismAgentPane.local_presence_name`, which started as `None` and was only installed during the later render loop. Submission could therefore serialize `"author": null`; the server then had no sender identity to persist or broadcast.

## Fix
- Install the resolved desktop presence display name immediately when creating the agent tab, before input can submit.
- Native prompt dispatch now always returns a non-empty author: configured/published presence name first, then the desktop system-derived presence identity fallback. Native HTTP prompt requests can no longer send null author.
- Stamp optimistic native user rows with that same guaranteed author.
- Agent server already persists `PromptRequest.author` and broadcasts it on user parts; remote desktop ingestion already preserves it.
- Renderer labels an explicit author matching the local presence name as `You`, a different author as the remote name, and only legacy authorless records as `Unknown user`.

## Additional web parity
Browser presence identity is also carried through active/pending `SubmitPrompt`, daemon forwarding, typed history, and WASM mapping.

## Verification
Native author tests prove published-name selection and non-empty system fallback. Orb tests prove local, remote, and missing-author behavior. Package-scoped formatting passes, all five tests pass, and `cargo check -p neoism` passes.
