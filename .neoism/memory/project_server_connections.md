---
name: "Application server connections"
description: "Final per-window daemon ownership, server-scoped subscriptions, and manager behavior"
type: "project"
scope: "project"
origin: "completed implementation and final invariant audit"
created: "2026-05-08"
updated: "2026-05-08"
---

Final correction: Local Server is resolved/retained independently from the initial window endpoint. On Unix `resolve_daemon(None)` always establishes Local; initial window uses CLI/SSH explicit endpoint when present, otherwise Local. Windows retains `ws://127.0.0.1:7878/session` as Local. Thus even if process starts with `--daemon-url`/`--ssh-host`, every later normal New Window starts Local. Initial explicit session id is `startup-explicit`; detach still inherits source.

Final UI details: multi-field modal form uses Address/Name/masked Token, tab and pointer focus, click/Enter structured submit; edit Ctrl+E prefilled, Delete confirms removal; inactive authenticated probes update gray/green/red list rows; active watch drives top rack green/amber/red. Form hidden ID skipped and duplicate form-height bug fixed. Add/edit/remove dead delimiter/action scaffolding removed.

Final verified commands: cargo check desktop; desktop lib registry 4 tests; shared server-form, picker, chrome tests; wasm check; web tsc; git diff check. Manual visual/two-daemon validation remains recommended but no known compile/test blockers.
