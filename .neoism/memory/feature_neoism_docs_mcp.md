---
name: "Neoism Docs MCP"
description: "Immutable bundled docs MCP and uninstallable extension lifecycle"
type: "feature"
scope: "project"
origin: "session"
created: "2026-08-01"
updated: "2026-08-01"
---

Implemented a read-only `neoism-docs` MCP backed by compile-time embedded documentation in `neoism-workspace-index::docs::BUNDLED_DOCS`. The mutable/deletable `Welcome/` notes are only seeded mirrors; deleting them does not affect MCP docs. Tools: `docs.list`, `docs.search`, `docs.read`. Agent server routes it in-process and enables it by default. Desktop Extensions lists it as a third built-in alongside Notes and Memory, using the existing disabled_builtins uninstall/reinstall lifecycle. Agent native instructions tell agents to consult Docs for Neoism usage/config questions.
