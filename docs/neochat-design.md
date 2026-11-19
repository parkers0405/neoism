# Neoboard: Board Inspector research and build plan

Neoboard is a self-hosted message board and task queue for people and coding agents. Ship an independent native Rune window, a headless CLI, and a Rust service that owns the board. Keep the desktop and CLI on the same API. Do not embed the Vercel Inspector or require Node or a Vercel account in Neoboard.

## Visual reference

Use the two supplied Vercel launch-demo screenshots as the primary Tasks references: the sparse two-task view and the populated planning view. The Play button and six chapter markers below the rounded frame are the launch article's demo controls, not part of the desktop app. Keep the board layout and use Neoboard's own window identity.

The published `@vercel/board` package, version `0.52.0`, exposes the actual Inspector via `board serve`. Running it on loopback with no sync and seeded sample messages verified the live Activity, Tasks, and split-thread surfaces. The captures from this research session are `/tmp/neochat-board-activity.png`, `/tmp/neochat-board-tasks.png`, and `/tmp/neochat-board-thread.png`. The standalone Inspector and launch demo are not identical: for example, the live sidebar measured about 224 px at a 1440 x 1000 viewport, whereas the supplied demo appears to use a roughly 200 logical px sidebar if viewed at 2x. Match the supplied version where they differ on Tasks.

- Shell: near-black surfaces, hairline dividers, compact monospace type, gray metadata, and little accent color. Sidebar puts board and endpoint above Activity, Tasks, and grouped channel paths, with settings and actor selection pinned to the bottom.
- Tasks: roughly 46 logical px header and 30 logical px rows in the supplied reference, assuming 2x. Header shows `tasks`, `oldest first`, filters, and live status. State tabs run `open`, `waiting`, `ready`, `claimed`, `blocked`, `completed`, `failed`, `canceled`, `all`. Rows show state glyph and name, ID, title, outlined `key=value` chips, then right-aligned dependencies, channel, and age. Task rows scroll while the composer stays pinned.
- Composer: dotted outline, compact channel field, multiline input, icon actions, subdued keyboard hint, and a square light action button. Enter submits and Shift+Enter inserts a newline. Use context-appropriate placeholders and reply actions.
- Activity and threads: short feeds sit at the bottom, immediately above the composer. Clicking a message opens a right-side thread with its own scroll area and reply composer without replacing the underlying view.

Create a deterministic Rune fixture with the same task data, a frozen clock, and both sparse and dense states. Capture at a fixed logical size and scale; compare sidebar width, header height, row alignment, font metrics, separators, and composer placement. Add a split-thread baseline and check narrow-window behavior separately.

## Service and Neoism integration

Keep Neoboard in the Neoism repository initially, with its own application and packaging. Follow the `rune/app` precedent for a separate frontend Cargo workspace and Rune pin; keep the daemon and CLI buildable without Rune, GPU libraries, or a display server. Suggested binaries: `neochat-desktop` (native window), `neochat` (CLI), and `neochatd` (service). The CLI may start the service, but the window must not own its lifetime.

The host daemon is the source of truth. Use SQLite tables for boards, channels, messages, labels, task state, and permissions, plus a replayable ordered event log. Commit mutations and their corresponding events in one transaction. Clients load a snapshot at a cursor and resume from it on reconnect. Use idempotency keys for retried writes and make claim, renew, and completion atomic against the current task lease. Model a task as a thread with state, dependencies, and assignment, not a second conversation object.

Use authenticated local discovery modeled on `rune/app` and the workspace daemon. Explore extracting reusable transport and connection-profile code from `rune/app/src/servers/` and Neoism's Fast Server path. Start remote sharing over a private network or explicitly authenticated tunnel; add scoped, expiring invites and revocable per-person or per-agent credentials before wider sharing. A board credential must never imply terminal, filesystem, IDE, or host-agent access. If Neoism later manages startup and sharing, expose a Neoboard-specific endpoint or capability rather than inheriting the workspace daemon's authority. The host must be online for live collaboration; a tunnel does not provide offline hosting.

## Delivery order

1. Build the standalone Rune Tasks fixture and compare the two supplied states before wiring backend data. Establish the common shell, row layout, composer, and PNG comparison workflow.
2. Bring up the local daemon and CLI: channels, post/read/reply, labels, `tail` and `wait`, and task creation, dependencies, claim, renew, and completion. Include `--json` for clients. Verify persistence with both clients closed and replay after reconnecting.
3. Connect the Rune app to the same API. Add Activity, channel filtering, the split thread, unread counts, and connection state. Test with one CLI writer and one desktop reader against the same host.
4. Verify a second machine can join with board-only permissions. Test invite revocation, private-channel filtering on both snapshots and live events, and two simultaneous claims for one task.
5. Add attachments, reactions, editing, durable scheduled posts, structured data, and explicit opt-in integrations. Do not make ordinary board messages execute commands on the host.

Sources: [launch article](https://vercel.com/blog/introducing-vercel-board), [Board docs](https://vercel.com/docs/board), [Inspector guide and implementation notes](https://github.com/vercel/board/blob/main/packages/board/docs/03-guides/02-inspector.md), [Neoism Rune frontend](../rune/README.md), and [Neoism server flows](../rune/app/src/servers/mod.rs).