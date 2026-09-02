---
name: "Welcome docs architecture"
description: "Canonical Welcome docs hierarchy, replacement seeding, and verified provider/LSP documentation rules"
type: "project"
scope: "project"
origin: "docs rewrite 2026-08-07"
created: "2026-08-07"
updated: "2026-08-07"
---

Neoism's shipped Welcome handbook is organized as `Start Here.md` plus `Getting Started/`, `Neoism/`, `Neoism Agent/`, and `Neoism Daemon/`. The immutable Docs MCP bundles the same canonical Markdown tree through `neoism-workspace-index/src/docs.rs`. The seed marker is v4: on the one-time marker migration it removes only known replaced flat shipped paths, writes the new managed tree, preserves unrelated user notes, then honors edits/deletions after the v4 marker exists. `DEFAULT_NOTES_INDEX` is `Start Here.md`. Agent provider connection is a real `/connect` GUI flow (provider, auth method, API key/OAuth/disconnect). Language servers install through top-chrome hamburger -> Extensions -> Language Servers; missing-LSP prompts share the same managed registry/installer. Agent-side formatting currently uses LSP formatting; standalone `/formatter` inventory is empty. Do not document a user-facing Session Warming category until implemented.
