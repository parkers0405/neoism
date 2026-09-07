---
name: "Host chat continuity: explicit directory association"
description: "Create Server associates exact local directory history with persistent hosted Agent namespace; selected ID reload; scoped operator-only boundary; tests passed"
type: "bug"
scope: "project"
origin: "implementation session"
created: "2026-09-06"
updated: "2026-09-06"
---

Implemented Create Server chat continuity in agent-server and standalone daemon. Operator `--workspace` startup calls POST /v2/hosting/associate with short-lived daemon HMAC credential subject neoism:host-chat-association; proxy callers cannot reach route, ordinary local/guest tokens cannot invoke handler. Store tables hosted_chat_directories(directory PK, workspace_id UNIQUE) + hosted_chat_sessions(session_id PK, workspace_id). Canonical exact-directory LOCAL/unowned root conversation families are associated transactionally, no session/history copies. First agent workspace namespace reused across repeat hosts/new daemon workspace IDs; daemon runtime mapping translates only after actual workspace root/auth resolution. Store overlays authoritative namespace on list/get reads, strips/recreates reserved neoismHostLocalAccess marker for local continuity. Owned other roots skipped; conflicting owned/out-of-root descendants cause atomic refusal, no stealing/splitting. Hosted child creation/local continuation preserve namespace; session-linked local artifacts follow authorized family; unattached local artifacts do not. Palette records selected session for explicit same-directory loopback port handoff; pane reloads selected ID through hosted auth without copying cached history. server.rs restores global no-bearer guards and narrower password-free fallback only for Shared workspace and no mandatory/provision auth. Checks passed cargo check -p neoism-workspace-daemon -p neoism; 3 hosting store/HTTP regressions, 11 daemon server tests, 1 desktop selection test. Existing legacy WebSocket AgentSession still uses global Agent token, outside scoped HTTP Alt+A flow; flagged to parent as separate preexisting authorization gap. No release builds, commits, index changes, or Mac perf edits.
