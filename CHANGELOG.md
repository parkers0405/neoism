# Changelog

All notable user-facing changes to Neoism are documented here.

## [0.7.61] - 2026-08-27

### Fixed

- Fixed watched sessions freezing while the activity pill kept moving.
  Three mechanisms were closed: transcript snapshots no longer run on the
  SSE reader thread (a slow store read was stalling token delivery for
  its whole duration), the scoped event stream caches foreign-session
  verdicts instead of walking storage per event, and a lagged event
  subscriber is disconnected for clean reconnect recovery instead of
  continuing with a hole in its timeline. Two client-side watchdogs
  remain as safety nets: a dead-socket detector and a stalled-stream
  force-resubscribe with full reconciliation.
- Fixed joined workspaces failing with daemon-credential errors after a
  token rotation: every place a daemon token is signed or compared (the
  websocket handshake, the agent proxy, credential minting, and agent
  verification) now treats the on-disk token as the live trust root with
  the startup environment as fallback.

### SDK and API

- Every message part now has a typed schema in the OpenAPI contract:
  discriminated part union, status-discriminated tool states, and typed
  step-finish token usage and cost. Part events carry the typed part.
- Live events carry a wire-monotone sequence (durable rows persist the
  same value), fixing SDK event streams that previously dropped every
  event after the first; the SDK dedupes by event id and its event
  subscription yields the typed union.
- Added a runnable headless embedding example and handbook coverage for
  embedding, hosted multi-tenant authentication, and API stability.

## [0.7.60] - 2026-08-27

### Fixed

- A watched agent session no longer freezes after a provider-overload
  retry. The event-stream reader now detects a dead connection (45
  seconds without server keep-alives), reconnects, and reconciles the
  transcript — live token streaming resumes without closing and
  reopening the session.
- Retried responses no longer double their text: the retry's message
  reset is honored so re-streamed tokens replace the failed partial
  reply instead of appending to it. Late empty snapshots outside a
  retry still never regress streamed text.
- Failed release build legs now save their compile caches, so release
  retries start warm.

## [0.7.59] - 2026-08-27

This release ships the Agent V2 replatform: the agent server is now a
plugin-first platform with one versioned API, one ordered event bus, a
third-party plugin runtime, and a typed TypeScript SDK.

### Agent V2 platform

- Rebuilt the agent server around internal plugins: providers, tools, agents,
  MCP, LSP, PTY, VCS, workflows, semantic search, configuration, and the
  system prompt all load as plugins in immutable per-workspace generations
  that reload live when configuration changes.
- Every route is served through `/v2/` and described by a committed OpenAPI
  document; a parity test keeps plugin-dispatched routes and the spec in
  lockstep.
- Replaced the split live/durable event channels with one ordered event bus:
  snapshots and deltas broadcast synchronously in publish order, ending
  freeze-then-double-stream artifacts and out-of-order thinking cards during
  live streams.
- Restored real-time token streaming cadence end to end.
- The session coordinator is the sole in-memory authority on run ownership;
  plugin session access is descriptor-validated.
- Plugin generations retire only after active leases release, so in-flight
  requests never race a configuration reload.
- The Agent supervisor starts with the workspace daemon, and persisted Agent
  settings project into V2 configuration.

### Third-party plugins

- Added the serve-plugin runtime: long-lived `neoism-plugin/2` processes that
  register tools, hooks, and event subscriptions over newline-delimited JSON
  stdio.
- Plugins load from a command, a local entry file, or an npm package; npm
  installs happen in the background and the generation rebuilds live when the
  install completes.
- A failing plugin degrades with a visible reason instead of failing the
  workspace.
- Added the `@neoism/plugin` authoring package (`definePlugin`/`runPlugin`)
  with an SDK client wired to the host server.
- Windows resolves npm and batch shims through `cmd /C` with PATHEXT-aware
  executable resolution.

### SDK and typed events

- Typed the V2 event stream: all 34 published event types form a
  discriminated union in the OpenAPI document, exhaustively tested against
  the server's event vocabulary.
- The TypeScript SDK yields that typed union from its event subscription,
  with automatic reconnect and a sequence cursor that deduplicates replays.
- Session, message, artifact, permission, question, provider, catalog, and
  plugin operations are all exposed through the generated typed client.
- Added a version-locked npm publish pipeline for the SDK packages.
- The desktop and shared frontends now consume `neoism-agent-core` types
  directly for event classification and turn assembly.
- Added TLS to the desktop agent transport for `https://` servers.

### Transcript search

- Session search in the agent side panel now searches full transcripts, not
  just titles: matching excerpt chunks render under each session,
  word-wrapped to the panel width.
- Every occurrence of the search terms highlights inside the excerpts.
- Semantic ranking blends in when an embeddings provider is configured;
  keyword search works without one.
- Multi-word searches fall back to per-term matches when no single message
  contains every word.

### Fixed

- New agent responses no longer inherit the previous response's execution
  timer: quiescence settles at queue-worker exit, exempts the admitting
  prompt's own worker, and reconciles leaked `running` run rows that
  silently blocked executions from ever finishing.
- Manual compaction durably finishes its run record instead of leaking a
  permanently running row.
- Transcript search no longer fails when plugin routes deliver numeric query
  parameters as strings.
- Subagent activity status, timing, and live token streaming stabilized;
  live agent timelines stay chronological and reasoning order survives
  metadata arrival.
- Notes vault tests no longer race process-global state.

### Documentation

- Documented the V2 platform in the bundled handbook: new Server and API,
  Plugins, and SDK pages, an architecture overview, and refreshed Configure,
  Tools, and MCP pages.
- GitHub releases now carry these notes automatically from the changelog.

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