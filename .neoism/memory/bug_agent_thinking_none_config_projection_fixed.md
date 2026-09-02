---
name: "Agent thinking none config projection fixed"
description: "V2 dropped persisted kebab-case Neoism Agent settings; adapter now projects them to strict canonical V2 keys."
type: "bug"
scope: "project"
origin: "post-Agent-V2 input chip regression"
created: "2026-08-25"
updated: "2026-08-25"
---

# Agent input thinking showed none after V2

Fixed on `neoism_agent_v2` in commit `74c9152ad`.

Root cause: unified Neoism `config.json` retained shipped kebab-case Agent settings such as `reasoning-effort`, while standalone Agent V2 intentionally accepts only canonical camelCase `variant`. `NeoismConfigSourceService::project_gui` copied the grouped `agent` object unchanged, so deserialization dropped the setting and the input chip rendered `thinking: None` as `none`.

Fix belongs at the product adapter boundary, not in standalone Agent config parsing. The Neoism adapter now projects persisted product spellings to canonical V2 names, with canonical values winning if both exist. Covered top-level provider/model/permission keys, experimental keys, and nested agent profile `top-p`/`max-steps`.

The user's active config had `reasoning-effort: medium`, `dangerously-skip-permissions`, and `experimental.batch-tool`; all were affected.

Verification: 4 adapter config tests passed, strict warning checks for adapter/server passed, diff hygiene passed.

Existing sessions created without a variant may continue showing `none` until the variant is selected once or a new session is created after rebuilding/restarting.
