---
name: "No empty or diagnostic PR sidebar row"
description: "No PR means no sidebar row/status/banner; explicit Create/Open in menu; fixed native declared-root readiness; new T3 inbox scope requested"
type: "bug"
scope: "project"
origin: "User screenshots rejected PR status banner and requested full inbox/detail workspace"
created: "2026-10-22"
updated: "2026-10-22"
---

User rejected always-present PR status row ('Pull request · The declared workspace is not ready for pull requests.'). Correct policy: sidebar row only for REAL linked PR (#number title), or transient explicitly requested numeric lookup. No PR/loading/automatic discovery/auth/readiness errors => NO row, NO reserved26pt height, NO hit rect, NO CTA. Create/Open live in Git actions menu. Existing Open slot9 preserved; Create appended slot10. Create performs fresh Discover and uses open_create intent to open form only after discovery; creates real row immediately on Created if still same branch. Explicit Create/Open failures go via take_pull_request_notices to native notification/Chrome notifications; automatic discovery failures silent and don't poison local Git status. No internal 'declared workspace' diagnostics as sidebar content. See bug_pr_ui_terminal_font.md for typography.

Native root cause: daemon_client_attached was incorrectly treated as authoritative root readiness, so missing cached summary overrode valid active_workspace_root and cached None. Transition install_io(None) also replaced scoped provider independently of cached identity. Fixed desktop: authoritative daemon root then explicit current active_workspace_root for local native (NEVER repo/cwd fallback), centralized scoped installation, reinstall if provider absent/host-driven despite sameidentity. Only newpanel construction uses install_io(None). GitDiffIo::pull_requests_ready defaults true; native false while scoped rootNone. Shared automatic discovery gates readiness BEFORE pending or retry timestamps. Native explicit notices drained in pump_pull_request_views. Parent shared tests36 PR/123Git/28Chrome passed and nativecheck passed after rootfix; full current goal verification will supersede as inbox added.

User then requested screenshot-literal T3-like full PR inbox and adjacent detail pane: search/sort/filter/grouped authored/reviewrequests/assigned, sparse large rows metadata andstatus, select row preserves LEFT list and RIGHT Summary/Timeline/Code detail with title/author/base-head/checkout. This is NEW scope beyond initial branch-only PR review; don't claim existing feature already does it. Maximum2 agents. Current followup still being implemented.
