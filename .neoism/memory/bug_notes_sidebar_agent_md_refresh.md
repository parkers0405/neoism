---
name: "bug_notes_sidebar_agent_md_refresh"
description: "Alt+N missed agent-created .md notes until refresh; folders showed. FileTree debounce coalesced later creates inside the 200ms window."
type: "bug"
scope: "project"
origin: "neoism-agent"
created: "2026-09-21"
updated: "2026-09-21"
---

---
name: notes sidebar missed agent md until refresh
description: Alt+N missed agent-created .md notes until refresh; folders showed. FileTree debounce coalesced later creates inside the 200ms window.
type: bug
origin: coding-agent
created: 2026-08-31
updated: 2026-08-31
---

Symptom: model `notes.create` / vault write sometimes did not appear in Alt+N until a manual refresh. Folders usually showed immediately. Intermittent because a later write after the debounce window DID refresh.

Cause:
1. Vault watcher (`sync_notes_fs_watcher`) and file-tree watcher both emit `PrepareRefreshFileTree`.
2. `debounce_follow_up` coalesced: first event (often mkdir / dir create) started a 200ms `Topic::FileTree` timer; a later `.md` create in that window was ignored. Walk ran before the file existed → folder row yes, note row no.

Fix:
- `postpone_follow_up` for `PrepareRefreshFileTree` (later events push the 200ms window).

Do not auto-expand parent folders of new notes. Closed folders stay closed.

Do not use coalesce-if-already-scheduled for FileTree/notes vault watches.
