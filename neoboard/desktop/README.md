# Neoboard desktop

Independent native Rune window (`Neoboard`, application ID `ai.neoism.neochat`). This crate has its own Cargo workspace/toolchain, follows the Rune pin and local touch/IME patches used by `../../rune/app`, and depends only on the shared `neochat` service client for board access. There is no webview, JavaScript, Neoism workspace authority, or demo/marketing strip.

## Run

```sh
cd neoboard/desktop
cargo run -- --data-dir /path/to/board --profile parker
# Omit --data-dir for the shared client's default local discovery/bootstrap.
# Install neochatd alongside neochat-desktop, or set NEOCHAT_DAEMON.
NEOCHAT_TOKEN='board-only-credential' cargo run -- --endpoint https://board.example --profile reviewer
cargo run -- --fixture
```

`--token TOKEN` is also supported; prefer `NEOCHAT_TOKEN` to avoid exposing credentials in shell history/process arguments. The shared client validates transport and authenticates all requests. Local bootstrap starts a sibling service if absent; closing the window does not terminate it. Actor/profile permissions remain server-enforced.

Activity includes posts and replies across channels. Channels filter the feed, Tasks filters by state and searches message IDs, authors, text and labels. Click a task/message to open its root thread on the right. Each thread retains an independent reply draft when switching or closing. Escape closes the thread. Collapse the sidebar from its header and reopen it from the content header; below 900 px the sidebar automatically yields space when the thread split is open.

The bottom composer stays pinned while the feed scrolls. Enter posts, Shift+Enter inserts a newline. In Tasks it creates a task. The tag icon opens Message details for the destination channel, labels, dependencies and assignee. The top-right filter icon opens text filtering and sort order. The sidebar `+` creates a channel. Task details expose claim, renew (300 second lease), complete, fail, and cancel. Errors retain drafts; failed writes are never automatically retried because a lost response can mean the server already committed the write.

The reply composer retains its paperclip and offers the same native tag icon. Its Reply labels dialog edits only that thread's reply-label draft, never the root composer labels or existing message labels. Text and labels survive closing/switching threads and failed sends. Tagged replies use the atomic `reply_with_labels` action; plain replies retain the existing `reply` API. Successful sends clear only matching sent drafts, leaving newer edits and other threads alone. Files use the same worker attachment flow for either reply action. Preview the dialog with `--scene reply-labels --snap /tmp/neoboard-reply-labels.png --at 1`.

Open a thread to edit a root post or reply using `edit / labels`. Text and labels have separate save actions; replacing labels with an empty field clears them. Labels support `key=value` and quoted values such as `area="desktop UI"` (backslash escapes quotes/backslashes); duplicate keys and unfinished quotes are rejected. The server enforces authorship and permissions. Edits retain their fields on errors and are not automatically retried. Reaction counts appear beside messages; `+1` adds a quick reaction, and the expanded editor accepts a custom reaction with add/remove-your-own actions.

`scheduled` lists the credential's pending posts and offers cancellation. The composer’s `schedule` toggle switches its primary button and Enter to scheduling a regular post (not a task). Enter a future `YYYY-MM-DD HH:MM` in **UTC**, or Unix seconds; 10-minute, hour, and day presets are available. Shift+Enter still inserts a newline. The daemon owns durable publication; closing the desktop does not cancel scheduled posts.

Each message/reply's `attachments` button opens a list with refresh, upload, and save-as controls. The cache-friendly default build uses explicit **absolute file paths**; `~` and environment variables are not expanded. `cargo run --features native-picker` enables Rune's existing asynchronous native Open/Save dialogs (XDG FileChooser portal on Linux); choosing a path only fills the field, and upload/download still require explicit confirmation. The fields remain available if a portal/dialog is unavailable. Uploads run on the worker, require a regular file, and use the shared 8 MiB limit. Save-as requires a complete destination filename, never uses a server-provided filename as a path, and atomically refuses existing files or symlinks, even if a native dialog offered overwrite confirmation. Files are created private on Unix. Saved content is **never opened or executed**. A failed save may leave a partial file: inspect/remove it or choose a new path before retrying. After upload the list refreshes; use refresh to see attachments uploaded by others.

The worker owns the blocking client, network parsing, file reads, and file writes. It loads a snapshot plus reaction/schedule state, resumes ordered events, reconnects with a fresh snapshot, and delivers typed updates through a bounded inbox. The UI drains at most 16 updates / a 1 ms soft budget per poll and applies message upserts through an ID index. It never blocks on network/filesystem I/O. Reaction/schedule events refresh their separate state on the worker; attachment lists are fetched on demand.

## Verification and screenshots

Use the pinned Rust toolchain; no release build is required.

```sh
cargo check --offline
cargo test --offline
# Optional real-service worker roundtrip, using an existing up-to-date debug daemon:
NEOCHAT_DESKTOP_TEST_DAEMON=/absolute/path/to/neochatd cargo test --offline worker_live_roundtrip -- --ignored
cargo run -- --scene tasks-sparse --snap /tmp/neochat-rune-sparse.png --size 1252x674 --scale 2 --at 1
cargo run -- --scene tasks-dense --snap /tmp/neochat-rune-tasks.png --size 1252x674 --scale 2 --at 1
cargo run -- --scene activity --snap /tmp/neochat-rune-activity.png --size 1252x674 --scale 2 --at 1
cargo run -- --scene thread --snap /tmp/neochat-rune-thread.png --size 1252x674 --scale 2 --at 1
cargo run -- --scene thread --snap /tmp/neochat-rune-narrow.png --size 700x600 --scale 1 --at 1
cargo run --offline -- --scene editing --snap /tmp/neochat-rune-editing.png --size 700x600 --scale 1 --at 1
cargo run --offline -- --scene attachments --snap /tmp/neochat-rune-attachments.png --size 1252x674 --scale 1 --at 1
cargo run --offline -- --scene scheduled --snap /tmp/neochat-rune-scheduled.png --size 1252x674 --scale 1 --at 1
```

These are deterministic, read-only fixtures with a frozen Unix clock. `--at 1` settles layout observers and responsive sizing before capture. No service is contacted for `--fixture`, `--scene`, `--snap`, or `--tree`. Rune's native headless renderer requires a usable graphics adapter (software Vulkan is sufficient). Tests also exercise real Rune text-input routing and independent split composers without opening a window.

The visual source is `../../docs/neochat-design.md`, the saved Board Inspector research PNGs (`/tmp/neochat-board-{activity,tasks,thread}.png`), and the requested supplied-demo metrics: 200 logical px sidebar including divider, 46 px header including divider, 30 px task rows, near-black surfaces and hairlines, bundled Geist Mono, and dotted composers. The two original launch-demo user attachments were not available as files to this delegated session; fixture text is representative, not an asserted transcription of those attachments.

Current limits: native OS file dialogs require the `native-picker` feature and a working platform dialog service; no message deletion, upload progress/cancellation, connection-management dialogs, or persistent unread/draft storage. Unread badges and drafts are in memory only. Long histories are held in the service snapshot; event ingestion is bounded but full large-board viewport-only rendering/paging remains future work. Remote/multi-machine behavior needs end-to-end validation against a deployed service; no security/lease guarantees are reimplemented in this UI.