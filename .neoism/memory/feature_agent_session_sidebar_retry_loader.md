---
name: "Agent session sidebar retry loader"
description: "Session sidebar animates through retries until Ready"
type: "feature"
scope: "project"
origin: "coding session"
created: "2026-08-29"
updated: "2026-08-29"
---

Agent session sidebar loading presentation: `Initial`, `Loading`, and retrying `Error` states all render the animated session skeleton. The skeleton owns redraw frames continuously until the catalog reaches `Ready`; it no longer freezes after 1.5s or shows `couldn't load sessions; retrying`. A successful `Ready` response with zero rows shows the normal `no previous sessions` state. Error metadata and stale rows remain internally preserved while retries continue.
