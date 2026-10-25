---
name: "Maximum two concurrent subagents"
description: "Hard user preference: no more than two concurrent subagents"
type: "feedback"
scope: "project"
origin: "User request during pull-request implementation"
created: "2026-10-22"
updated: "2026-10-22"
---

The user explicitly requested a maximum of two subagents running at a time while implementing pull-request support. Stop excess agents and keep concurrency at two or fewer, including follow-up implementation waves. Parent can work on independent host/integration tasks; do not fan out additional subagents beyond this cap.
