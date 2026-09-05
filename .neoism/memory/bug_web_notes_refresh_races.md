---
name: "web-notes-refresh-races"
description: "Robust web Notes vault binding and generation-guarded all-or-nothing recursive refresh"
type: "bug"
scope: "project"
origin: "coding-agent"
created: "2026-08-01"
updated: "2026-08-01"
---

---
name: web-notes-refresh-races
description: Robust web Notes vault binding and generation-guarded all-or-nothing recursive refresh
metadata:
  type: bug
  status: fixed
---

Fixed intermittent empty/stale web Notes state in `neoism-frontend/web`:
- `App.handlePtyCreated` passes cached `activeWorkspaceVaultPath` into each new `TerminalPanel`.
- `applyNotesVault` always synchronizes the mounted panel, including same-vault broadcasts.
- `clearTerminal` drops connection-scoped client/service/session/workspace/root/vault/host caches; rehome preserves only `event.workspaceId` as a fresh destination selection intent.
- `TerminalPanel` Notes refreshes use `NotesRefreshCoordinator`: monotonically increasing generation plus exact adapter identity and normalized vault checks before clear/commit. Vault changes and disposal invalidate pending work.
- recursive Notes collection is all-or-nothing, propagates child failures, and retry starts with a new accumulator so partial first-attempt rows cannot leak.
- Pure logic/tests live in `terminal/notesRefresh.ts` and `.test.mts`.

Verification: web `npm run typecheck` and `npm test` (120 passing).
