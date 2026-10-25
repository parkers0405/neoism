---
name: "Systems connections and cross-system Agent workflows"
description: "Background Systems + master-only owner transport; federated root chats, root system picker, pinned execution and safe reconnects; new daemon build required."
type: "feature"
scope: "project"
origin: "neoism-agent"
created: "2026-10-23"
updated: "2026-10-23"
---

# Systems connections and cross-system Agent workflows

Top-right opens shared Systems/Servers palette. Systems persist SSH recipes in OS-user systems.json, separate from hosting servers and credentials. Existing modal styling, host/alias + optional name. Credentials/tunnel endpoints remain in memory. Systems Connect/Add are strictly background-only: never activate/create/re-root a workspace or cancel unrelated workspace switches. Old open_on_ready/workspace activation behavior was a bug and is removed.

## Owner transport

SSH reads remote user's master daemon-token. New /agent/system proxy and GET /agent/system/info require exact master token, never trust-local/device/workspace guest credentials. Descriptor: {systemId,name,homeDirectory,capabilityVersion:1}. Version capability required; older daemons show update-required rather than falling back. BOTH desktop client and remote daemon stack need the new build; prior v0.7.135 does not contain this endpoint.

Daemon mints explicit 60-second signed system-workflow credentials, no workspace, local tenant. Shared exact method/path allowlist in agent-service-api/daemon_credential.rs enforced by proxy and Agent middleware. Permits full machine session-projects/history/workflows/directories and positively projected safe provider/model/agent metadata. Denies management/config (including config/defaults), provider auth/OAuth/connections, hosting and global plugin routes. Configured/resolver/workspace callers cannot spoof reserved system subject. Guests remain scoped. Test fixture must start catalog backfill and await readiness or bare-router list queries return 503 indexing.

## Source and catalog model

Desktop AgentSystemSource (agent/system.rs) is ephemeral: saved id, descriptor systemId, generation, label, canonical home, agent base, shared atomic authorization flag. Register credentials separately for exact /agent/system base. App background connections publish via Screen::set_agent_system_sources; Drop revokes source and unregisters token. Production same-user standalone SSH daemon and ordinary desktop use the same default agent.turso.db. Debug desktop profiles intentionally use isolated neoism-dev paths; match state/upstream explicitly to see debug history.

Root/home dashboard federates local + connected authorized sources via complete /v2/session-projects and per-project pagination. Runtime identities: neoism-system:<JSON [savedId,generation,value]> for projects/sessions, avoiding same paths/IDs on two hosts. Source-safe streams, title search, pin/rename/delete and pagination reject revoked/stale origins. Remote session activation pins pane backend/cwd without creating a workspace. Workspace sync must use sync_workspace_server so it does not overwrite explicit execution bindings. Main sidebar remains workspace/catalog scoped independently of active conversation cwd.

## Composer

ComposerAction::ChangeSystem and PickerKind::System use existing inline picker. ComposerSystemControl(label,selectable) renders left of directory. Shared neoism_ui_impl_agent_user_input macro MUST delegate composer_system() to desktop composer_system_control(); default None otherwise silently hides chip. An actual-trait regression now guards this. Root fresh draft can choose system; declared-directory defaults remain owner-bound; existing conversation cannot casually retarget. Preserve input, attachments, cursor and draft identity on selection; choose canonical remote home, never transplant local cwd. Remote folder browser calls selected-backend /v2/directories with captured identity/generation, not current workspace FS.

## Isolation and reconnect

System file/tool/diff links explicitly report unsupported before any local exists/search/editor resolution; never open editable local same-path file. Remote session.moved updates pane cwd only, never queues local SwitchWorkspace. Passive local-only config/goals/MCP/management polling and local Git/plugin footer fallback are skipped for system backends.

Pinned panes reconnect only if saved id AND descriptor systemId match. Rebind transport/channels/streams, preserve session/cwd/draft/images/cursor, invalidate stale callbacks. Never auto-replay uncertain/queued submissions. Different machine under same saved id stays blocked. Unavailable sources stop streams and session/background ingestion; same-epoch temporary recovery also rebinds safely. Applies to parked tabs as well. No local fallback.

Global plugin/MCP/goals/skills management, provider auth/config and remote semantic transcript search are unsupported through workflow v1. Authorized title search works. Agent conversation tabs themselves are not currently persisted/restored as chats: snapshot Agent classification falls through to terminal/workspace root, not raw chat ID/cwd, so no discovered restart-time remote-chat/local-backend fallback. Qualified source IDs are runtime catalog identities, not durable restoration records.

## Verification

29 desktop system tests passed, including real rendered trait chip, same-path/session collision and independent pages, source revocation, binding sync, input preservation, reconnect identity/no replay, suspended ingestion, remote movement/file guards, manager isolation. 63 shared composer tests passed. Backend credential, machine-history/guest isolation, master proxy gate and metadata-redaction tests passed. Desktop/shared cargo check --tests, wasm32 check, web typecheck and scoped formatting/diff checks passed. No release build, live UI manipulation, or two-machine runtime test of the new endpoint; running remote binaries were not redeployed by this coding increment.
