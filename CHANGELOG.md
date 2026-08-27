# Changelog

All notable user-facing changes to Neoism are documented here.

## [0.7.56] - 2026-08-27

### Added

- Rebuilt the agent server as the plugin-first V2 platform: providers, tools,
  MCP, LSP, PTY, VCS, workflows, and configuration now load as internal
  plugins in immutable per-workspace generations that reload live on
  configuration changes.
- Added the serve-plugin runtime for third-party plugins: long-lived
  `neoism-plugin/2` processes registered from a command, a local entry file,
  or an npm package, exposing tools, hooks, and event subscriptions. Plugin
  failures degrade with a reason instead of failing the workspace.
- Added the `@neoism/plugin` TypeScript authoring package
  (`definePlugin`/`runPlugin`) alongside the typed SDK packages, with a
  version-locked npm publish pipeline.
- Typed the V2 event stream: every published event type is part of a
  discriminated union in the committed OpenAPI document, and the SDK event
  subscription yields the typed union with cursor-based reconnect.
- Added TLS support to the desktop agent transport (`https://` servers).
- Session search in the agent side panel now searches full transcripts:
  matching excerpt chunks render under each session, word-wrapped to the
  panel, with every occurrence of the search terms highlighted. Semantic
  ranking blends in when an embeddings provider is configured; keyword
  search works without one.
- Multi-word transcript searches fall back to per-term matches when no
  single message contains every word.
- Documented the V2 platform in the bundled handbook: new Server and API,
  Plugins, and SDK pages plus an architecture overview.

### Changed

- Replaced the split live/durable agent event channels with one ordered
  event bus: snapshots and deltas arrive in strict publish order, ending
  freeze-then-double-stream artifacts and out-of-order thinking cards.
- The session coordinator is now the sole in-memory authority on run
  ownership.
- Plugin-dispatched routes are validated against the OpenAPI document by a
  parity test, and plugin session access is descriptor-validated.
- Windows runs npm-based plugin installs and batch shims through `cmd /C`
  with PATHEXT-aware resolution.

### Fixed

- New agent responses no longer inherit the previous response's execution
  timer. Quiescence now settles from the queue worker's exit, exempts the
  admitting prompt's own worker, and reconciles leaked `running` run rows
  (for example from manual compaction) that silently blocked executions
  from ever finishing.
- Manual compaction durably finishes its run record instead of leaking a
  permanently running row.
- Transcript search no longer fails when plugin routes deliver numeric
  query parameters as strings.
- Restored real-time token streaming cadence and correct part ordering
  during live streams.

## [0.7.55] - 2026-08-24

### Added

- Added scheduled agent workflows loaded from project or global `workflow/` and
  `workflows/` directories.
- Added one-time, hourly, daily, weekly, monthly, and interval schedules with
  local or IANA timezones, DST handling, month-end clamping, and missed-run
  coalescing.
- Added workflow activation, pausing, manual runs, previews, diagnostics,
  durable run history, overlap prevention, and filesystem hot reload.
- Added workflow API routes and OpenAPI definitions for listing, inspecting,
  activating, pausing, running, previewing, and reading history.
- Added unattended workflow permission policies with explicit allow and deny
  patterns. Interactive permission requests are denied instead of blocking a
  scheduled run indefinitely.
- Added a bundled Scheduled Workflows guide and linked it from the Neoism Agent
  documentation.
- Added a rectangular presence-avatar rendering primitive for future shared
  presence surfaces.

### Changed

- Redesigned the agent side-panel usage display as a compact System 7-style
  context meter.
- Token totals in the side panel now use the mode indicator's rainbow scramble
  transition when their values change.
- Clicking the side-panel usage meter now opens the complete input, output,
  cache, reasoning, cost, and model breakdown on desktop and web.
- The global status line remains free of token-usage UI.
- Each subagent now notifies its parent immediately when that child finishes,
  without waiting for sibling agents. Workflows that need all results can make
  that waiting policy explicit in their prompt.
- Reused subagent sessions can report each later execution independently.
- Local MCP servers now reconnect after process failure, restart when relevant
  configuration changes, and shut down when disabled or removed.
- MCP discovery now reconciles stale runtime state before listing tools.

### Fixed

- Made subagent completion delivery durable and exactly once per logical child
  execution using generation identities, stable message IDs, and serialized
  child and parent reconciliation.
- Prevented duplicate or quadruple subagent completion notifications from
  wrapper, queue-idle, abort, restart, and concurrent sibling races.
- Prevented queued follow-ups from publishing stale wrapper results; one final
  completion is emitted after the queued drain finishes.
- Delayed completion acknowledgement until the runtime notification is durably
  appended to parent history, while keeping crash recovery idempotent.
- Repaired legacy and malformed persisted subagent completion metadata without
  panicking the agent server or duplicating notifications.
- Prevented late permission request and reply cleanup from resurrecting a
  finished subagent in the sidebar. Genuine task continuations still reactivate
  the child normally.
- Fixed streaming conversation teleports while reading above the bottom by
  replacing ambiguous message-ID anchors with stable source and content
  identities.
- Fixed timeline anchoring across duplicate or empty IDs, optimistic-to-durable
  ID transitions, grouped rows, history prepends, tail appends, row removal,
  and active wheel motion.
- Assigned optimistic user prompts their outbound message IDs before display
  and reconciled identical prompt text one-to-one.
- Made scheduled-run claiming and schedule-cursor advancement atomic for SQLite
  and Turso, preventing a crash from repeating a scheduled occurrence and its
  possible side effects.
- Added bounded MCP process shutdown and exact failed-runtime invalidation.

### Tests

- Added and expanded coverage for workflow parsing, validation, recurrence,
  timezones, DST, permissions, execution directories, hot reload, atomic run
  claims, and restart recovery.
- Added concurrency and recovery coverage for immediate, repeated, and
  exactly-once subagent completion delivery.
- Added desktop and shared regressions for permission terminal locks, optimistic
  prompts, stable timeline anchors, wheel targets, usage animation, hit targets,
  and details presentation.