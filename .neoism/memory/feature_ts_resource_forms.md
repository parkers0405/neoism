---
name: "TS skill/workflow forms and current layout"
description: "Complete skill/workflow forms, correct selected-project writes, persistence fixes; anchored todo list; latest no-brand/no-chat-hints layout; 549 tests"
type: "feature"
scope: "project"
origin: "user"
created: "2026-09-10"
updated: "2026-09-10"
---

Latest GUI increment supersedes older footer/branding/form notes.

- Top chrome has navigation controls and chat tabs only; removed favicon + Neoism name. Full home wordmark unchanged.
- Native activity word is at transcript live end and scrolls OUT of view with history; do not pin it to viewport above input. Background-only label Background, active response Crafting, reasoning Pondering.
- Chat footer hints (tab agents / commands) and project selector are hidden. Native 8-cell scanner moved to far RIGHT inside agent/model/thinking skirt. No reserved footer height in chat, input lower. Home keeps project/hints footer.

Skills/workflows now use a.directory (active selected tab project) rather than a.prefs.directory in App Library prop AND key. Resource scope helper resolves via server /v2/directories, guards stale scopes; old-server 404 allows explicit absolute path unchanged with warning, not guessed empty/relative roots. Creation capability gated; current running server lacks neoism.management and requires opt-in + operator auth for writes. Do not weaken auth/restart live Neoism blindly.

New ResourceEditorDialog/SkillDefinitionEditor/StructuredValueEditor/WorkflowDefinitionEditor: 920px responsive themed shell, Geist24 headings, icons, grouped labeled forms, sticky Cancel/Create/Save. Complete typed JSON compatibility/metadata, support-file add/rename/remove/text limits, skill versions/view/restore, read-only views. Workflow searchable project agents/skills/models/accounts/timezones, all schedule variants, concurrency/retry, graphical permissions. Execution directory distinct from definition storage root. API only supports saved schedule preview, explicitly labeled. Bound editor/root/client prevents redirecting save after project switch.

Backend storage: standalone workspace skills .agent/skills; native adapter .neoism/skills via discovery roots; workflows .agent/workflows. Fixed discovery/watch to include managed .agent/workflows when adapter roots .neoism. Real temp workspace tests verify selected root.

Review fixes: save success adopted immediately, refresh failure separate warning/retry only; UTF8 bundle content+files limits match backend (not JSON escaping); compatibility omitted vs explicit null preserved with presence-aware serde; conditional restore of concurrently deleted target fails412 and does not recreate; unconditional restore can recreate and strips historical write preconditions.

Todo scroll bug FIXED: inline current checklist was appended at transcript tail and chased every response. Timeline now anchors it to the first message where it became visible, renders latest updates in that SAME article/DOM, stable checkbox identity; returns to history still hide tool UI. New Timeline.todos.test covers later messages, updates, and reload visibility; no performance regression.

Verified latest: production build + TypeScript;549 GUI tests/54 files; native assets check; git diff check. Backend17 management,15 workflow,7 directory tests passed. No browser/Playwright, release builds, live user resource writes, or Neoism restart. User example image fetched through artifact API and native read-image tool (permitted; not browser).
