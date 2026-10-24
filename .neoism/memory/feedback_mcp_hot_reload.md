---
name: "Keep MCP hot reload behavior"
description: "User accepts MCP generation refresh and yellow/ready connection reset for now; preserve current hot reload"
type: "preference"
scope: "project"
origin: "neoism-agent"
created: "2026-10-16"
updated: "2026-10-16"
---

User explicitly accepts the existing MCP behavior where editing/enabling one server republishes the workspace plugin generation and can reset unrelated runtime connections, leaving enabled servers yellow/ready. Preserve hot reload; do not change this lifecycle merely to avoid the ready/yellow refresh. `ready` means enabled/configured but not runtime-connected, and maps to Warning/yellow in shared agent picker. Connected is Good/green. User asked about this behavior and then said it is okay for now because they want the current hot reload.
