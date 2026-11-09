# GPUiX live rendering baseline

## Scope

Measurements on 2026-11-02, workspace source `e280d42b8`, Linux/Wayland, 120 Hz display. The existing installed frontend was observed without restarting it. Workspace benchmark scripts used the existing native artifacts, not newly compiled source. This is a baseline and an initial experiment, not a completed renderer migration or proof of the cause of intermittent lag.

The primary workload is real agent conversation activity, including tool output, native children, and LSP. A synthetic scrolling or token-streaming fixture is only a supplemental control; it cannot establish that real chats are responsive.

The reported regression is specifically rare, severe drops despite ordinary performance resembling the previous Sugarloaf frontend. This makes the new frontend/integration a leading suspect, but does not identify text rasterization, GPU execution, allocation/GC, scheduling, or server event delivery as the cause. Averages and even p99 from a short good interval cannot clear the regression. The original hard drop has not yet been captured and causally attributed.

## Real application measurements

The installed window had this investigation's conversation and another approximately 1,300-item conversation loaded. During observation the user switched to the larger conversation and its scroll offset changed. Two real native child tasks ran bounded read-only investigations. One performed four native LSP operations: status, workspace symbols for `LspRuntime`, references, and diagnostics. Rust was already connected. No builds, file mutations, extra windows, or nested children were requested from these tasks.

| Metric | Observation | Limit |
| --- | --- | --- |
| Frontend CPU | Mean 72.6%, range 56.9-84.4%, over fifteen 2-second samples | Percent of one logical CPU, all frontend threads; not specifically rasterization |
| Workspace daemon CPU | Mean 7.3%, range 2.0-19.5%, over the same samples | Does not measure tool queue latency |
| Frontend resident memory | Approximately 0.85-0.88 GiB during sampled intervals | RSS includes shared pages |
| Rust analyzer | Initially approximately 10.4 GiB RSS, rising to approximately 11.3 GiB; several samples at 83-100% of one CPU during LSP activity | Correlated with this interval, not proof that every byte or CPU spike came from these requests |
| Physical presentation feedback | 7,574 additional presentations over 64.247 seconds: 117.9 FPS | Aggregate across two SIGUSR2 snapshots; longer than the 30-second CPU interval; not input latency or a frame-time distribution |
| Native draw phase at final snapshot | Last 1,000 samples: p99 2.84 ms, max 6.53 ms | Does not include all upstream JS/model/native preparation or waiting time; not a GPU timestamp measurement |
| System pressure | About 30 GiB available RAM; 85-93% aggregate CPU idle in sampled intervals | Does not exclude a single UI thread bottleneck |
| Swap activity | 6-26 KiB/sec swap-in, zero swap-out in interval samples | The first vmstat line is a since-boot average and was excluded; existing 6.6 GiB swap occupancy is not evidence of current thrashing |

Collection used `top -b -n 16 -d 2 -p <frontend>,<daemon>,<rust-analyzer>`, `vmstat 2 16`, and the frontend's existing `SIGUSR2` diagnostic handler. Identified PIDs were checked before signaling. No existing application, daemon, or language server was stopped.

The frame feedback does not establish that occasional freezes, delayed clicks, expensive diff expansion, or LSP/tool waits are absent. There was no matched rendering-path A/B on the live conversation and no input-to-content-presentation measurement. The user continued interacting, so this is an observational workload, not an isolated deterministic benchmark.

`nvidia-smi pmon -c 15 -d 2 -s um` subsequently confirmed that the real frontend PID owns an RTX 3090 GPU context and approximately 300 MiB of framebuffer memory. Available per-process samples reported 10-54% SM utilization; many samples were unavailable (`-`), not zero. This establishes actual GPU use, not GPU saturation or the cause of a drop. CPU, GPU, compositor, and server data must be correlated at the same stall timestamp before attributing the regression.

## Rare-stall tracing

The updated frontend supports `NEOISM_GPUIX_STALL_TRACE=1`. It retains at most 256 completed timed operations of at least 1 ms from the preceding five seconds, checks event-loop progress every 32 ms, and emits `[neoism-gpuix-stall]` records for at least 50 ms excess timer delay or a completed operation lasting at least 50 ms. Requested native display-wake wait time is now explicitly measured, so a late native callback can be reported even when JavaScript timers remain responsive. Inactive time with no requested native wake is not itself counted as a wake wait.

Records contain wall-clock timestamps, process CPU deltas, RSS, the last observed native draw sample, the trigger, and bounded operation metadata. No text, file paths, daemon credentials, or conversation content are recorded by these hooks. Emission is limited to once per second, with a suppressed-event count. Nested spans and waits overlap and must not be summed as CPU work. `input.*` observations are retained as context but cannot independently trigger a stall: no-op wheel input at a boundary may wait for an unrelated future draw. Ordinary two-second profiler logs remain off unless `NEOISM_GPUIX_PROFILE=1` is separately enabled.

This is opt-in diagnostic code, not a rendering fix. It requires launching the updated frontend with the environment flag and retaining its stderr; the currently running installed binary cannot acquire these hooks without an update/relaunch. No running user session was restarted. Tests exercise a bounded 130 ms event-loop block in a separate subprocess, plus an independently delayed native wake, so trace behavior is verified without freezing the user's app.

Limitations: completed-operation tracing cannot diagnose a permanent native hang by itself; minimized/background windows and OS suspension can delay native wakes; the last observed draw is not an input-to-content presentation acknowledgement. External process/GPU samples and the interaction state remain necessary. A two-second resource interval can also miss a brief spike that the event-loop trace captures.

## Full server and language-server capture

`scripts/profile-neoism-processes.py` now measures complete observed process ancestry using procps and sysstat. It separates the active frontend tree, other Neoism frontend/server roots, and unattached tool candidates, without treating every Node process as Neoism-owned. It excludes its own descendants, tracks observed exited ancestry, detects some PID reuse, and reports incomplete data rather than replacing unknowns with zero. It captures CPU, scheduler-wait samples, RSS, endpoint PSS, threads, major faults, disk I/O, and context switches. Generic `other` descendants may include measurement commands other than the profiler itself; these are not all application work.

A 60-second capture with two-second intervals ran against the existing frontend, workspace/embedded agent server, Node/TypeScript/YAML processes, rust-analyzer and helpers, tools, and seven other Neoism server roots. TypeScript symbol and Rust diagnostic requests were attempted during this interval; their combined tool response exceeded the tool-output retention limit, so no inference about returned diagnostics is made. The raw profiler output was also truncated by the agent tool's presentation layer, so the table below uses fully visible per-process records rather than claiming validation of every omitted summary field.

| Component | Mean CPU, one-core percent | Largest observed two-second CPU sample | Other observations |
| --- | --- | --- | --- |
| Active frontend | 51.49% | 53.5% | 35 peak sampled threads; no sampled major faults |
| Active workspace/embedded agent server | 2.10% | 8.5% | 84 threads; disk writes averaged 257 KiB/sec, peaked at 3,888 KiB/sec |
| Busiest active TypeScript server | 0.97% | 28.0% | Peak RSS about 812 MiB |
| Main rust-analyzer | 0.07% | 0.5% | About 11.3 GiB resident; warm requests here, unlike the earlier busy interval |
| Seven other workspace servers, excluding their children | 6.80% combined mean | Individual peaks 1.5-2.0% | Separate observed roots, not seven native child agents |

A subsequent two-second ownership/PSS endpoint capture returned both expected intervals, 46 known endpoint processes, no output omissions, and no tool flags. Summed known endpoint PSS was approximately 14.10 GiB for the active frontend tree and 2.65 GiB for the other seven Neoism trees. The active tree had two TypeScript language-server families containing four tsservers and two typings installers, together about 1.24 GiB PSS. One observed transient `other` process had already exited and had no endpoint PSS, so totals are known contributions rather than exact atomic machine totals. Multiple families are not by themselves proof of erroneous duplication or the cause of lag; ownership/configuration must be checked before stopping anything. No existing server was stopped.

Overlapping whole-GPU samples reported 25-28% SM utilization, 13-14% memory-engine utilization, graphics clocks around 495-540 MHz, about 1,035 MiB total framebuffer allocation, 51-52 W and 32 C. These samples do not show GPU saturation or overheating. They cannot rule out a brief driver/presentation stall between samples and include the compositor and other applications.

Reproduce a bounded capture from the repository root, using the current frontend PID rather than blindly reusing a saved PID:

```sh
python3 scripts/profile-neoism-processes.py --root-pid FRONTEND_PID --duration 60 --interval 2 --pidstat /path/to/pidstat
```

Sysstat 12.8.0 was obtained in an isolated Nix environment; no system packages or kernel permissions were changed. Fourteen Python attribution/aggregation tests passed. Fourteen Bun stall-recorder, display-clock and presentation-timing tests passed, along with TypeScript checking. No native rebuild or production rendering optimization was performed.

## Real multi-workspace experiment protocol

`neoism-frontend/gpuix/scripts/profile-real-agent-experiment.ts --run-real-models` creates two disposable projects and fresh real conversations, rather than importing fabricated messages. The checkout project delegates money, inventory, and order implementations to three native children; the telemetry project delegates statistics and rolling-window implementations to two. The parents integrate real edits and run Node tests. Prompts require real LSP queries, Bash tests, native file tools, and no network/packages or edits outside their disposable project. This is an explicit paid-model experiment, not a normal unit test.

An updated source frontend uses the existing native libraries, a private recovery-state file, and opt-in stall tracing. Twelve tabs cover two agent conversations, five TypeScript files, two Markdown documents, two Git working-diff surfaces, and a real Bash PTY. The driver cycles workspaces and tabs, scrolls visible content, and runs `node --version`, `git diff --stat`, and `npm test` inside the terminal while the agents work. `src/terminal/buffer.ts` confirms that this terminal loads libghostty for VT state and feeds its snapshots to GPUiX; it is separate from the Bash tool's agent-output cards.

The harness saves capped frontend traces and private results in `/tmp/neoism-real-agent-experiment-*`, preserves project files for verification, aborts only its owned sessions/children at teardown, closes only its own PTY/workspaces/window, and leaves existing user projects untouched. Action timings include automation and a deliberate 350 ms workspace-settle wait, so they are not input-to-presentation latency. The source frontend must be classified separately from generic Node processes in resource summaries; the process profiler recognizes its distinctive Bun/preload/entrypoint combination.

Initial incomplete attempts are excluded from successful workload results. They exposed harness mistakes: a fresh empty chat exposes a composer before a conversation view exists; tabs need visibility/settling; a PTY changes its tab title after running commands. The driver now caches tab identities rather than relying on immutable terminal titles. The automation library's `fill()` converts a long prompt into thousands of synchronous keystrokes, causing artificial approximately 1.9-second loop delays; prompts now enter through the production daemon SDK while their real model/tool activity renders in the real frontend. These artificial input bursts are not evidence of the reported spontaneous regression.

The latest focused verification has 16 Python tests and 15 Bun tests passing, plus TypeScript checking. The running installed session is not restarted to run this experiment.

### Completed real run

The complete run is preserved at `/tmp/neoism-real-agent-experiment-jJZzNE/results.json`, with `frontend-trace.log`, both projects, and their generated tests beside it. It finished in 177.9 seconds, completed eight workspace/tab cycles and 194 recorded navigation/scroll actions, and observed all five native children running concurrently. Eight command batches executed in the actual libghostty-backed Bash terminal. Checkout passed 26 tests and telemetry passed 21 tests. Both parent queues and all five child queues were idle at the post-run audit.

The audited seven real conversations contained 82 top-level tool parts: 76 completed and six intermediate errors. Direct tool parts included 12 Bash calls and seven LSP calls, in addition to nested operations inside compose. Errors included attempts to read not-yet-created test files, one incorrect compose tool namespace, and transient test failures during parallel implementation; the projects were subsequently integrated and both final test suites passed. Parent-only live counters in `results.json` intentionally do not include all child tool parts.

The trace contained seven trigger records: a roughly 198 ms startup loop delay, a roughly 54 ms early loop delay, and five requested native-frame callback waits of 52-76 ms. All five late native callbacks fell approximately 0.5-0.6 seconds after activating the Bash tab and sending its test command batch. Their concurrent JavaScript timer excess was below 0.1 ms. Thus these are a concrete terminal/native-scheduling correlation, not evidence that JavaScript was blocked for those waits, nor proof that libghostty parsing itself was slow. They are not confirmed physical-presentation freezes. The larger spontaneous hard drop reported by the user did not reproduce in this run.

During the overlapping two-minute full-process capture, the instrumented experiment frontend averaged 29.05% of one CPU (largest two-second sample 69%), while the still-open original user frontend averaged 62.93%. The shared workspace/agent server averaged 12.52%, peaked at 34%, and wrote about 2.52 MiB/sec on average, with a 7.55 MiB/sec peak. These are concurrent applications sharing one daemon, not isolated single-window measurements. Aggregate GPU samples ranged from 15-64% SM utilization at changing clocks, with temperature 32-35 C and no sustained saturation established.

### Verified cleanup/lifecycle finding

Closing the disposable workspaces did not stop their TypeScript language-server families. Nine wrapper roots remained across the incomplete attempts and completed run, each verified by `pwdx` to be rooted in one of this experiment's disposable project directories. The completed run alone had two families per project, consistent with separate editor and agent LSP ownership. Across the capture there were 11 TypeScript wrappers, 22 tsserver workers, and 11 typings installers including the pre-existing user families. This is not one LSP per native child.

After verifying the real task queues were idle, only the nine owned wrapper roots and their descendants were sent SIGTERM: 36 experiment-owned processes total. The pre-existing user language servers and seven older Neoism daemon trees were left untouched. Four defunct test PTY shells were also observed under the shared daemon; these are already exited, consume no working-set memory/CPU, and require parent-side reaping rather than killing an unrelated daemon. The server-lifecycle retention is a separately reproduced resource issue, not a demonstrated cause of the original UI hard drop.

The post-cleanup check confirmed that all nine targeted wrapper roots had exited with zero RSS, but remained unreaped zombies under the shared daemon. The two pre-existing user TypeScript wrapper roots were still running. No daemon was restarted merely to clear zombie entries. Later harness revisions retain timestamped events and automatically audit parent/child tool histories and idle queues before declaring completion; the completed run above received that audit separately through the same production SDK.

## Implemented fix: display-rate terminal publication

The follow-up isolated terminal replay found that every incoming PTY packet synchronously rebuilt the visible terminal frame and serialized its canvas display list. The terminal already used native GPUiX text commands, so replacing CPU text-run images was not the relevant change in this path. During a finite 1,000-line log burst, 1,008 Ghostty feed calls produced 1,001 viewport paints and about 6.5 MB of batch JSON.

`src/terminal/TerminalView.tsx` now keeps Ghostty ingestion and terminal responses immediate, while coalescing visible frame publication through the existing native presentation scheduler. Each display frame samples the latest buffer state, not an old captured packet. Cursor animation remains independently coalesced on the same display clock. Unmount cancels pending publication. No bytes, scrollback, selection state, or protocol replies are dropped.

The actual-component regression failed before the fix with 20 immediate frame reads for 20 packets, then passed with zero reads before presentation and exactly one latest-state publication on the next display frame. It also checks final output, dense ANSI scrollback, selection highlighting, and cancellation during engine disposal. Twenty-nine relevant terminal/display/stall tests and TypeScript checking passed.

The replay command uses the completed real project's tests and a bounded Node log stream, without making further model requests:

```sh
bun scripts/profile-terminal-replay.ts /tmp/neoism-real-agent-experiment-jJZzNE/checkout
```

| Output-burst metric | Baseline | Patched run 1 | Patched run 2 |
| --- | --- | --- | --- |
| Ghostty feed calls | 1,008 | 1,008 | 1,007 |
| Viewport paints | 1,001 | 132 | 129 |
| Batch calls in the busy reporting interval | 1,069 | 175 | 172 |
| Batch JSON bytes in that interval | 6,491,161 | 874,721 | 856,337 |
| Post-startup recorded slow wakes/loop stalls | One 59 ms wake during burst | None | None |

Regular repeated `npm test` output also fell from 316 viewport paints to 48 and 49 in the two patched runs. Startup still had slow wakes and is not claimed fixed. This is a measured reduction of roughly 87% in redundant burst paints and batch traffic, not an 87% whole-app speedup or proof that every reported hard drop is solved. The before/after traces and metrics are in `/tmp/neoism-terminal-replay-9OdRhG`, `/tmp/neoism-terminal-replay-mK2ezb`, and `/tmp/neoism-terminal-replay-HAeFe9`.

Stall diagnostics now distinguish an actual queued-animation wake from an empty lookahead reservation and capture native draw statistics at a trigger. A late reservation or a no-op input observation alone must not be mistaken for proof that user-visible content froze.

### Full real-workflow verification after the terminal fix

The repeated real-model run at `/tmp/neoism-real-agent-experiment-MLWAQs/results.json` completed in 267.0 seconds with five concurrent children, twelve workspace/tab cycles, and twelve terminal command batches. All seven queues were idle at completion. Its audited top-level operations included 17 Bash calls and seven LSP calls, and the resulting projects passed 63 tests (44 checkout, 19 telemetry). The sole recorded stall trigger was a roughly 212 ms startup loop delay before the experiment was ready; there were no post-startup triggers. This is another complete workload, not an identical-message A/B, and Rust verification work was also running on the machine during part of it.

A separate read-only replay loaded 3,119 history items from the existing real conversation, paginated and scrolled it at full window width, and saved results at `/tmp/neoism-live-profile-u3MHDV/results.json`. It recorded startup delay and one early 63 ms native wake, but did not reproduce the original large hard drop during pagination/scrolling. Its last 1,000 native draw samples had p99 4.23 ms and max 4.70 ms; these draw-only figures are not complete frame/input latency. The user's installed window and real conversation were not reset, aborted, or sent synthetic prompts.

## Implemented lifecycle corrections

PTY ownership is now tracked explicitly. Natural exit caches the reaped status, and destruction does not signal an already-reaped PID. Ordinary close first sends SIGHUP so shell cleanup can run; an off-thread reaper gives 250 ms grace before SIGKILL escalation for a still-owned running leader. Close no longer performs an unbounded worker join on its caller. SIGCHLD is registered before child creation rather than adding a permanent polling timer. Final queued output is drained before the exit notification, and explicit close racing natural exit emits only one registry close event.

LSP editor documents now have reference-counted surface leases released on close, replacement, disconnect, and registered workspace-surface closure. Ordered workers coalesce adjacent snapshots but preserve save/query/close barriers. Last-owner close releases document state and worker routes, while reconnects reuse the FIFO so an old close cannot overtake a new open. Engine close happens outside the global route mutex, preventing an unresponsive server from blocking unrelated document admission.

Agent/editor LSP engines retire unpinned clients after 60 idle seconds, swept every five seconds. Open editor documents, admitted operations, and actual LSP-consuming task-generation tokens protect live consumers; passive websocket/bus generation leases do not keep cached servers alive forever. This is not indiscriminate root shutdown on workspace close and does not merge editor/agent unsaved-document authority. LSP process-group cleanup must retain ownership of an unreaped leader until signalling and waiting are finished, including when the wrapper exits before its helpers.

These Rust changes require the normal updated daemon build/relaunch to affect the installed app. The real UI replay used the existing daemon, so its four retained test language-server families were again verified by exact disposable working directory and cleaned up separately (16 owned processes); this was not presented as live verification of the newly edited daemon. Existing user servers were not killed. The daemon-wide PTY test command hit a 180-second dependency-compilation timeout before its registry tests ran, rather than a failing registry assertion. Targeted PTY process/unit regressions passed; detailed final backend verification is recorded with the implementation results.

### Final regression verification

The optimized-default daemon test compilation timeout was resolved by reusing the low-optimization test profile; both actual daemon registry tests then passed. These were Cargo tests, not extracted or mocked copies of the registry. Final verified coverage includes 11 agent lifecycle tests, three editor live-sync tests, two daemon PTY registry tests, four real-process PTY lifecycle tests, and 12 PTY unit tests. The real-process suite also passed five concurrent runs. Focused Rust `cargo check --tests`, TypeScript checking, and 48 relevant frontend tests passed. Windows runtime behavior was not exercised.

The LSP reaper review added explicit consumed-child ownership and Unix `waitid(WNOWAIT)` observation. An exited but unreaped leader reserves its numeric identity while helpers are terminated; after reaping or `ECHILD`, later cleanup does not signal or wait through that numeric PID/PGID. Tests cover an exited leader with a live helper, already-reaped ownership, and shutdown while another writer has saturated the server's stdin pipe.

Working daemon regression command:

```sh
CARGO_BUILD_JOBS=2 cargo test -q -p neoism-workspace-daemon --config 'profile.test.package.neoism-agent-server.opt-level=0' --config 'profile.test.package.neoism-workspace-daemon.opt-level=0' --lib sessions::tests::lifecycle_ -- --test-threads=1
```

Local production packaging uses `make build`; `node scripts/launch-gpuix-preview.mjs --user-config /home/parkersettle/projects/neoism` launches the resulting frontend with its newly built daemon, user configuration, and private runtime/state without replacing the existing running chat.

## Background timeline interpretation

The dump lists retained native timeline engines, including engines whose views have been unmounted. A background engine reporting `user_orb`, `streaming`, or `running_tool` is not proof that it is currently scheduling frames. Normal tab switching unmounts the old view and disposes its scheduler. A visible but unfocused split can still animate. `user_orb` is also independently active for a visible user-message decoration, even when the session is Idle.

Source checks found existing transcript-event routing and changed-message layout invalidation, not blanket transcript rebuilding for every child delta. Broader lifecycle updates can still request a visible-timeline frame. Frame generation, cached raster reuse, and actual raster cache misses must be measured separately. See `src/agent/AgentTimelineView.tsx`, `src/agent/timeline-event-routing.ts`, and `native/src/agent_timeline.rs` under `neoism-frontend/gpuix`.

Separate editor/agent LSP ownership and document/process lifecycle risks remain as documented in [the resource audit](agent-lsp-resource-audit.md). This test did not prove a duplicate server started or quantify LSP lock contention.

## Supplemental rendering-path experiment

A four-second code-scroll fixture used the same 1260x674 window size, touchpad-style input, and approximately the same scroll range in separate processes. It compared existing row-image rendering with the existing native canvas text fallback, not a redesigned shared glyph/display-list pipeline.

| Metric | Current row-image path | Native canvas text fallback |
| --- | --- | --- |
| Presented FPS | 119.99 | 119.99 |
| Process CPU time over four seconds | 1,592 ms | 1,979 ms |
| Bytes passed to image upload API | 5,473,848 | 389,336 |
| Batch JSON bytes | 919,683 | 8,514,486 |
| Native draw-phase p99 | 1.11 ms | 2.46 ms |

This single pair reduced explicit image-upload bytes but increased process CPU by approximately 24% and native drawing cost. It does not justify simply disabling rasterization as a performance fix. The pair is not statistically repeated, has small scroll-position differences, and is not a real-chat result. Image API call time excludes asynchronous GPU execution, and its byte counter does not cover internal native glyph-atlas uploads.

An earlier synthetic run generally reached 120 FPS for code, Markdown, and agent output. A subsequent run lost focus and changed window geometry during several Markdown/agent phases; those phases are excluded from comparative conclusions. A large root-draw gap during a scroll-boundary interval cannot be classified as a stall merely from its duration.

## Benchmark changes and reproduction

`tests/support/scenario-fps-window.tsx` now reports process CPU time, image API bytes/calls/time, batch bytes/calls/time, input callback gaps, and code/Markdown scroll range. `NEOISM_BENCH_NATIVE_CODE_TEXT=1` selects and checks the code-text fallback. Scenarios return a failing exit status on exceptions or detected focus/geometry invalidation. Focus/size validity is checked only at interval endpoints; unknown focus is reported as `null`, not silently certified. These changes affect the test harness, not production rendering.

Run from `neoism-frontend/gpuix`, with a working Bun runtime and the native artifacts' runtime libraries available:

```sh
NEOISM_BENCH_MAXIMIZED=1 NEOISM_BENCH_SCROLL=touchpad-regular NEOISM_BENCH_MS=4000 bun scripts/native-scenario-fps.ts code
NEOISM_BENCH_MAXIMIZED=1 NEOISM_BENCH_SCROLL=touchpad-regular NEOISM_BENCH_MS=4000 NEOISM_BENCH_NATIVE_CODE_TEXT=1 bun scripts/native-scenario-fps.ts code
```

For the next real-chat comparison, retain the real history/tool/diff/subagent structure and instrument event arrival, model application, layout/preparation, raster misses, native batch submission, and content presentation. Record distributions and pauses alongside CPU and process-tree resources, with one/three/five active children and warm/cold LSP activity separated. Do not call average FPS, draw-only duration, or reduced upload traffic an input-latency improvement.
## Per-frame miss attribution (2026-11-09)

Presentation feedback on Hyprland reports `sequence=0` for every frame, so missed vblanks must be derived from presentation timestamps, not sequence numbers. `NEOISM_GPUIX_FRAME_TRACE=1` logs frame-wake arrivals and every JavaScript operation of at least 0.3 ms (or 64 KiB) on the same monotonic clock as `GPUI_WAYLAND_PRESENTATION_TRACE=1`, plus native timeline phases (`[core-timeline]`), the slowest painted transcript row (`[neoism-timeline-row-slow]`), and large daemon messages by variant and size. `scripts/analyze-gpuix-frames.py` replays the FPS pill's 500 ms buckets and classifies each missed vblank (late native wake, long or late JavaScript, GPU blocking, long UI frame, or compositor latch). Tracing adds JavaScript overhead; compare traced runs only with traced runs.

Measured on a mirror of the live window (same session, 1252x674 logical at scale 2, 120 Hz), all without restarting the user's app or daemon:

- Idle, real wheel scrolling of loaded history, streaming, and tool-heavy turns lost 0-0.4% of vblanks. Nearly all of the remainder had the wake and JavaScript on time with the UI frame near 1 ms; the compositor latched the frame one vblank late.
- Paging into unloaded history caused the reported dips (pill readings of 80, 93 and 104). Each older page arrived as one 0.3-1.9 MB message and cost about 11 ms per MB on the display thread: WebSocket parse, a JSON re-encode for the native timeline and a native parse, all inside the display-frame batch.
- Every completed turn re-fetched a 200-message head and re-sent the entire merged transcript to the native timeline: 13 ms for 699 rows, growing with loaded history.
- A real `cargo build` sharing the window's cgroup lowered pill readings to 86-100. Scheduler wait stayed below 20 us per wakeup; UI, JavaScript and Hyprland work instead took about 1.7x longer (SMT siblings, all-core clocks, cache pressure), and Hyprland itself delivered some frame callbacks a frame late.

Changes:

- Agent events are applied after the display frame's batch commits (`setImmediate` from the frame request) instead of inside it.
- Older pages are sized by bytes: the next request scales the previous limit toward 160 KiB of message text (8-64 server messages, starting at 24) and is requested further ahead.
- A head refresh of an already hydrated transcript delivers only the rows that changed, as `MessageUpdated`, when they keep their order or extend the end. Inserted or reordered rows still deliver the complete merged transcript.

Untraced source-mode A/B on the 2,115-message conversation, fresh process each: paging 1.00% missed (pill minimum 92) before, 0.57% (minimum 112) after; scrolling loaded history 0.50% before, 0.17% after; wheel scrolling during a real release build 2.82% before, 2.26% after. Build-load sensitivity remains largely outside the frontend's control.
