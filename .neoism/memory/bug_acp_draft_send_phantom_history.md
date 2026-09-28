---
name: "ACP draft send and phantom OpenCode history"
description: "Draft ACP model lost on first send and empty OpenCode previews shown as chats"
type: "bug"
scope: "project"
origin: "user screenshots and provider-native read-only count"
created: "2026-09-25"
updated: "2026-09-25"
---

2026-09-25: User reported choosing OpenCode model/thinking in blank composer then Send switched both to defaults, and four phantom OpenCode history previews alongside two real chats, failed import with no user transcript. Root cause first-send `create_prompt_session` omitted draft `externalOptions` while `ensure_session` included them. Fixed by snapshotting draft selections in `PendingPromptDispatch`, sending them in both root creation paths, clearing stale preview and refreshing active options on PromptDispatched; block active Send while option POST pending. External sidebar New chat now stays draft (no premature EnsureSession), source-switch refreshes preview. OpenCode `session/list` catalog included provider-persisted empty sessions created by ephemeral ACP preview `session/new`; local read-only opencode.db showed 2 with user turns and 4 without for workspace. Backend catalog OpenCode-only read-only SQLite role query filters only exact-id/cwd rows known to have zero user messages, retaining unknown and imported roots; no provider data deleted. Tests native first-send queue/root body/sidebar (5 pass), native options (19), server catalog (2), preview (7), cargo check server/UI/native and diff --check pass. Running GUI NOT restarted/tested; filtered rows disappear only after new daemon/catalog refresh. Provider-owned empty sessions remain on disk.
