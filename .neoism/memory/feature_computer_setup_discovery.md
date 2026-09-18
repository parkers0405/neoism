---
name: "Computer MCP attachment instructions and bundled docs"
description: "computer.capabilities includes self-contained Firefox/Chromium setup before approval; canonical searchable Docs page Neoism Agent/Computer Use.md is registered"
type: "feature"
scope: "project"
origin: "User requested discoverable Firefox attachment information"
created: "2026-09-16"
updated: "2026-09-16"
---

Browser attachment guidance is now exposed directly in computer.capabilities on BOTH permission-free call_tool and authorized worker paths via browser::setup_info(). It reports config validity (not connectivity), protocol and configurationError; static Firefox/Chromium launch examples; exact endpoint env names/URLs; requirement to set env in actual agent-server process and restart; separate-profile/user-consent warnings; native target/tab workflow; Firefox disconnect/crash recovery. It never connects to the browser. browser_tabs description and unconfigured-browser errors point to capabilities and canonical bundled Docs path.
Added neoism-workspace-index/src/welcome/Neoism Agent/Computer Use.md, registered in neoism-product-docs/src/lib.rs BUNDLED_DOCS and linked from MCP Servers.md. Thus first-class Docs tool can search/read `Neoism Agent/Computer Use.md` after rebuilding/loading the new bundle. Existing sidecar neoism-agent/browser-computer-use.md links to it. No Welcome seeding marker bump: don't overwrite existing user's editable Welcome notes just to publish immutable docs. Product bundle registration/linkage test passed; new server test covers unapproved capabilities setup fields. Related implementation memory: feature_computer_browser_cdp.md.
