# Coding-agent speed audit

Audit date: 2026-11-18. Neoism source revision: `90ddba4e098df383b30ce97ca728d9a769c5ba9d`; installed executable: `neoism-agent 0.7.159-nightly.20261117.1`. The installed executable is not assumed to match this source tree or the older frozen evaluation image. This audit does not change production behavior or launch model inference.

## Bottom line

Existing measurements establish a speed weakness against one Claude Code setup, not yet against OpenCode or Codex CLI. In the completed private synthetic comparison, Neoism averaged 137.37 seconds per attempt versus 40.49 seconds, a 3.39x ratio of total times. Neoism passed 20/20 attempts and Claude Code passed 19/20. Different models/providers and an older Neoism binary prevent attributing this ratio to the current harness alone. See [RESULTS.md](RESULTS.md) for the original evidence and limitations.

The strongest measured leads are more model-message activity, background exploration, and lower reported prompt-cache reuse. The strongest newly checked source difference versus Codex is transport/continuation support. None has a measured causal share of the speed gap.

## Standard benchmarks to use

| Benchmark | What it tests | Fit for this investigation |
| --- | --- | --- |
| [Terminal-Bench 2](https://github.com/laude-institute/terminal-bench-2), through [Harbor](https://github.com/harbor-framework/harbor) | End-to-end terminal tasks with environments, reference solutions and independent verifiers | Best first public pilot for file/tool use, debugging, Git and running services. Harbor already has OpenCode and Codex adapters. Add or validate a Neoism adapter without replacing its native loop. |
| [SWE-bench Verified](https://github.com/SWE-bench/SWE-bench) | 500 human-validated real repository issues, scored by tests | Better repository bug-fixing coverage than our synthetic suite. Measure time as well as resolved issues; the pass rate alone is not a speed benchmark. |
| [SWE-bench Pro](https://github.com/scaleapi/SWE-bench_Pro-os) | More demanding real repository maintenance | An unfinished same-model HARD-51 lab already exists in [public/README.md](public/README.md). Validate that work rather than creating another competing runner or calling its plan a result. |
| [Aider Polyglot](https://aider.chat/docs/leaderboards/) | 225 coding exercises across C++, Go, Java, JavaScript, Python and Rust | Useful for focused edit/test correctness and time-to-fix. Less representative of broad repository exploration, background jobs or long sessions. Run the native harness on the tasks to compare harnesses, not just Aider's own leaderboard numbers. |

Public benchmark scores are not directly comparable unless task versions, model, effort, attempt policy, environment and grading match. A small selected pilot must be labeled a subset, not a full leaderboard score. Do not import a benchmark's reference solutions or hidden tests into contestant workspaces.

## Cloned sources

These are shallow clones in the ignored `.tmp/agent-benchmark-audit/` directory, not vendored dependencies or changes to the existing public lab's locks.

| Directory | Repository | Exact audit revision |
| --- | --- | --- |
| `harbor` | `harbor-framework/harbor` | `3ee9f53e6b459712fdec4e8ce6613d7aa5610299` |
| `terminal-bench-2` | `laude-institute/terminal-bench-2` | `2fd12b88aafdd04a52c298e3940bcb189f9766d6` |
| `opencode` | `anomalyco/opencode` | `a61f5ea83460c197229e67b9c07c49ea87615385` |
| `codex` | `openai/codex` | `ad5dd4a1104c37c1ab9cb7d122d27a464613815a` |

Harbor in this checkout is 0.26.0; the separate HARD-51 lab pins 0.22.0. Do not silently substitute one for the other. Re-cloning moving default branches later will not reproduce these revisions without explicitly checking out the recorded commits.

## Concrete task shortlist

These task descriptions were inspected in the cloned public dataset. They cover things a coding agent is expected to do; they are not claims about Neoism's performance on these tasks.

| Terminal-Bench task | Observable work | Weakness it can help expose |
| --- | --- | --- |
| `cancel-async-tasks` | Implement concurrency-limited async jobs with cancellation cleanup | Excessive planning/delegation on a compact coding problem; correctness after interruption |
| `fix-code-vulnerability` | Find and fix a Bottle vulnerability, report its CWE, pass tests | Search/read efficiency, context size and repeated test/edit cycles |
| `fix-git` | Recover lost site changes and merge them into master | Efficient diagnosis versus unnecessary broad exploration |
| `modernize-scientific-stack` | Port a Python 2 climate script and provide modern dependencies | Multi-file edits, setup overhead and verification discipline |
| `kv-store-grpc` | Define protobuf, generate Python bindings, implement and run a server | Shell/tool coordination, background-process completion and task acceptance |

The dataset's `large-scale-text-editing` task requires restricted Vim macros over one million rows. It is useful for terminal automation, but is not a clean benchmark of Neoism's native edit tool or editor rendering, so it should not be used to make that claim.

## Where Neoism is weak or at risk

### 1. Interaction-heavy execution: measured on the older suite

The recorded traces have 323 deduplicated assistant-message IDs for Neoism versus 125 for Claude Code, including 99 child messages and 19 background exploration launches. Neoism still has 224 root messages when children are excluded. Native tool-call IDs total 541 versus 112, although Claude's Bash chains can do multiple operations per call, so tool counts are not equivalent units of work. The native run did not record compose calls. This supports testing fewer unnecessary model round trips and better batching, not weakening file-access safeguards or removing useful parallel tools.

Current source also appends proactive delegation instructions when the Responses variant is `ultra` and the task tool is available: `crates/neoism-agent-builtins/src/provider_responses.rs:65-74,123-126`. Base agent guidance is already latency-aware; do not assume all configurations force delegation or that the old benchmark used this variant. Measure an off/on delegation ablation at fixed model effort rather than removing subagents indiscriminately.

### 2. Cache reuse: measured old deficit, source fix already present

The older comparison reported 26.20% cached-read input for Neoism versus 91.47% for Claude Code. These are cross-provider observations, not a same-provider cache experiment. The frozen image did not contain the subsequent cache-affinity fix.

Current `crates/neoism-agent-builtins/src/provider_openai.rs:1441-1494` already sets a scoped Codex `prompt_cache_key` and matching `session-id`; session lineage supplies affinity. The cloned OpenCode likewise derives `session-id` from session affinity in `packages/core/src/plugin/provider/chatgpt.ts:281-293`. Missing cache affinity must not be presented as an outstanding source bug. Check the installed version and measure controlled cold/warm reuse before claiming the fix improved latency.

### 3. Codex transport and large request handling: source difference, not measured impact

Neoism constructs conversation input, instructions and tools and posts JSON to the Codex endpoint, consuming SSE: `crates/neoism-agent-builtins/src/provider_responses.rs:65-90` and `provider_openai.rs:1475-1546`. This path does not implement Codex's incremental WebSocket continuation.

The cloned Codex documents cached WebSocket connections, sticky routing, prewarm and `previous_response_id` continuation in `codex-rs/core/src/client.rs:1-25,329-330`. The cloned OpenCode explicitly selects HTTP and defaults ChatGPT request encoding to zstd in `packages/core/src/plugin/provider/chatgpt.ts:296-309`; HTTP alone therefore does not distinguish Neoism from OpenCode. Neoism's inspected request builder uses ordinary `.json(&body)` without an analogous request-compression step.

Benchmark request bytes, serialization time, first-event latency and repeated tool steps with long history before choosing compression or WebSocket work. A POST can reuse an HTTP connection, and full-history replay can still hit a provider prompt cache. Neither the source difference nor Codex's prewarm establishes an end-to-end speedup by itself.

### 4. Long failure tails: source-established risk

The default provider inter-event idle timeout is 300 seconds (`crates/neoism-agent-server/src/session_loop.rs:10-12,73-79`). Recovery permits eight retries, with ordinary backoff starting at 1.5 seconds and capped at 30 seconds (`session_retry.rs:13-22,52-67`); provider retry headers can request substantially longer waits. A silent broken request can consequently feel stuck for minutes. This is not established as a cause of the old benchmark's normal-task latency. Deep reasoning can legitimately remain silent, so use injected-stall/recovery tests before tightening defaults.

### 5. Instrumentation is not sufficient for causal attribution

The older run's recorded synchronous tool intervals sum to 38.94 seconds, while total wall time is 2,747.44 seconds. Root-only provider-active counters sum to 2,033.09 seconds and omit child activity. The remaining time is not automatically database, transport or orchestration overhead. Comparable per-request first-token distributions are missing. Serial persistence/lock work on tool-result boundaries is another profiling candidate (`crates/neoism-agent-server/src/provider_stream_processor.rs:994-1007,1063-1082`), not a demonstrated bottleneck; same-step tool execution is already concurrent.

## What was actually checked

- Cloned the public task dataset, official Harbor harness, OpenCode source and Codex source at the revisions above.
- Installed Harbor's locked non-development dependencies in its ignored local virtual environment, without installing into the user's global Python environment.
- Ran `cancel-async-tasks` with Harbor's reference-solution `oracle` agent in a disposable Docker environment: reward 1.0, zero exceptions, approximately 52 seconds total job runtime including environment/setup/verification. This is benchmark calibration, not a Neoism attempt or agent speed measurement.
- Attempted the same task with the no-operation `nop` agent as a negative control. Its verifier stalled fetching the uv installer archive from GitHub and hit the 240-second outer limit. No valid negative-control score was obtained. The timed-out job remains under `.tmp/agent-benchmark-audit/jobs/cancel-async-nop/`; its container and network were explicitly removed after timeout. This is an infrastructure failure, not an acceptance failure or Neoism result.
- Ran the existing offline eval-lab tests: 57 discovered, 48 passed, nine opt-in Docker checks skipped. No new model calls, credential reads or subscription usage were performed.

Oracle reproduction from the Neoism repository root:

```sh
uv run --project .tmp/agent-benchmark-audit/harbor --locked --no-dev harbor run \
  --path "$PWD/.tmp/agent-benchmark-audit/terminal-bench-2/cancel-async-tasks" \
  --agent oracle --env docker --force-build --n-concurrent 1 --n-attempts 1 \
  --jobs-dir "$PWD/.tmp/agent-benchmark-audit/jobs" --job-name cancel-async-oracle
```

Use a fresh job name for subsequent executions. Artifacts from this audit are under `.tmp/agent-benchmark-audit/jobs/cancel-async-oracle/`. `--force-build` builds the task Dockerfile instead of relying on its named prebuilt image. This one-task calibration does not validate all other tasks, native agent adapters, credential routing or public-lab readiness.

Keep environment construction, native-agent setup, agent execution and verification timings separate. This task's verifier installs curl and downloads uv from GitHub at runtime; network/installer delays must not be charged to agent reasoning or reported as a Neoism failure.

## Smallest useful next live experiment

1. Use the five shortlisted public tasks, three fresh paired attempts each, initially Neoism versus OpenCode on exactly the same available model/provider/authentication path. Keep equal effort/output limits and task resources, randomized paired order and no automatic fresh-session replacement. This is a 30-attempt pilot, not a full benchmark score. Freeze the binaries, task commits and prompts first.
2. Compare Codex separately on a genuinely equivalent OpenAI backend/model path. Do not treat different subscription and direct-API routes as a clean harness isolation, or force Codex into an unsupported provider to match the Anthropic HARD-51 lab.
3. Record acceptance success and all-attempt durations, paired ratios, failures and successful-task time. With only 15 attempts per harness, a p95 is exploratory; larger tail-latency claims need more samples. Never report fast failures as a speed win.
4. Instrument actual request count across root and child sessions, send-to-first-provider-event, first-text/tool latency, request completion, input/cache/output/reasoning tokens, payload bytes, tool execution overlap, child waits, retries and last-tool-to-next-request gaps. Store monotonic timings without secrets or full prompts.
5. Run targeted cold/warm-cache and delegation ablations only after the baseline. For local orchestration and stall recovery, use a fake provider so those measurements require no model spend.

Live execution remains unstarted: the existing lab explicitly requires new approved usage/spending limits, and the public HARD-51 comparison still has unresolved validation/background-child accounting. Previous experiment budgets are not authorization for this pilot. No measured Neoism-versus-OpenCode or Neoism-versus-Codex speed ranking is claimed here.