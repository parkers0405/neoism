# Coding-agent evaluation lab

An offline-first pilot comparing the **real Neoism harness** with Claude Code. `--suite smoke` selects three plumbing/calibration tasks. `--suite repository` selects ten larger multi-file Python mini-projects; two attempts per setup produce a 40-trial comparison. These are private synthetic tasks, not real upstream issues or a recognized public benchmark. Scores describe this suite, not a universal product ranking.

## Contestants

| Setup | Harness | Model |
| --- | --- | --- |
| `neoism-sol` | Installed `neoism-agent serve`, native V2 session API | `gpt-6.1-sol` |
| `claude-sonnet` | Claude Code 2.1.292, noninteractive streaming mode | `claude-sonnet-5-5` |
| `claude-opus` | Claude Code 2.1.292, noninteractive streaming mode | `claude-opus-5-5` |

Sol and Sonnet are the primary base-API-price-matched comparison; Opus is an optional higher-cost reference. OpenAI OAuth uses subscription access instead of API billing, so the saved-auth lineup compares correctness, time, and token usage, not equivalent dollar spend. Model availability and account authorization are **not** established by installing the tools. Comparing different models measures complete setups, not harness quality alone. A later same-model comparison is needed to isolate the harness.

Official model references: [GPT-6.1 Sol](https://developers.openai.com/api/docs/models/gpt-6.1-sol), [Claude Sonnet 5.5](https://platform.claude.com/docs/en/models/sonnet-5-5/overview), [Claude Opus 5.5](https://platform.claude.com/docs/en/models/opus-5-5/overview).

## Offline preparation

Requirements: Linux, Python 3.10+, a non-root user with access to a local Docker daemon, and an executable Linux `neoism-agent`. No Python packages are required. The image build downloads OS dependencies and the pinned Claude Code package, but never requests inference. Its build context contains only the selected native binary, Dockerfile, and public harness scripts, not this repository or your home directory.

Run from the repository root:

```sh
python3 -m unittest discover -s neoism-agent/evals -p 'test_*.py' -v
python3 neoism-agent/evals/lab.py build
python3 neoism-agent/evals/lab.py doctor
python3 neoism-agent/evals/lab.py self-test
EVAL_DOCKER_TESTS=1 python3 -m unittest discover -s neoism-agent/evals -p 'test_*.py' -v
```

`build --neoism-agent /absolute/path` selects another native binary. The Ubuntu base digest and Claude package version are pinned; the final image ID, native binary SHA-256, and native version are recorded. OS package repositories are not snapshot-pinned, so rebuilding is not guaranteed byte-identical. Runs use the resolved image ID, never a mutable tag after initial resolution.

`doctor` starts both CLI version checks and Neoism's actual server inside a network-disabled container, checks health, creates an empty session, and reads its transcript. It never submits a task prompt. By default it reports whether the two API-key environment variables are present. `doctor --use-neoism-auth` instead privately reads the saved Neoism auth store and prints only selected credential types and whether their validity is sufficient for a smoke trial. Neither mode authenticates online, prints credential values, or passes credentials into preflight containers.

`self-test` verifies every original broken workspace fails its hidden acceptance suite and every reference solution passes, in fresh network-disabled verifier containers. A missing dependency, broken verifier, timeout, or invalid candidate is not accepted as successful calibration. Reference solutions and verifier files are never mounted in contestant containers.

## Task contract

Each directory in `tasks/` contains:

- `task.json`: ID, description, agent deadline, and verifier deadline.
- `instruction.md`: the identical user request for both contestants.
- `workspace/`: starting implementation and public tests.
- `verifier/verify.py`: independent acceptance checks; zero exit status means acceptance passed.
- `solution/`: reference changes used only for offline calibration.

The smoke suite covers nested configuration merging, note text normalization, and movement aggregation. The repository suite covers routing, interval planning, expression parsing, transactional JSON patching, dependency graphs, event reduction, migration planning, archive manifests, CSV pipelines, and cache policy. All use Python's standard library. Calibration runs before any model calls; task files and image IDs are fingerprinted before execution. Use `self-test --suite repository` to calibrate the expanded suite independently. A complete private Python comparison is still narrower than an external multi-language benchmark: fresh held-out real repository issues, task-level uncertainty estimates, and broader language coverage remain future work. [Harbor](https://docs.harborframework.com/) is a suitable later integration for established datasets and distributed execution; this runner does not reimplement a model/tool loop or claim Harbor compatibility.

## Paid execution is opt-in

**Do not launch a paid or subscription-usage run until the user approves the experiment's usage/spending limits.** Use `--use-neoism-auth` to reuse the saved local `openai` OAuth connection and `anthropic` API-key connection. Without that flag, `OPENAI_API_KEY` and `ANTHROPIC_API_KEY` remain supported. Never place keys in task files, the image, or command arguments.

```sh
python3 neoism-agent/evals/lab.py doctor --use-neoism-auth
```

Saved-auth path precedence matches Neoism: `NEOISM_AGENT_AUTH_PATH`, then `$XDG_STATE_HOME/neoism-agent/auth.json`, then `$HOME/.local/state/neoism-agent/auth.json`. `--auth-path PATH` can override it. Legacy and V2 connection stores are supported. V2 selection uses the local, non-workspace default or sole eligible connection; ambiguity fails closed. Use `--openai-connection ID` / `--anthropic-connection ID` for an explicit selection. No unrelated credentials or personal configuration are exported. `claude-code` proxy credentials are not substituted for an Anthropic API key.

Only a minimal selected credential and task prompt cross Docker stdin; no secret is a command argument, Docker environment setting, mounted host file, image layer, or provenance field. Inside the worker, auth is injected only into the appropriate agent child environment. The original auth store is never mounted or modified. OpenAI receives a read-only access-only OAuth projection with the true expiry and an empty refresh field, **never the real refresh token**. Before each trial the parent re-reads the store and requires expiry to exceed the entire outer deadline, provider refresh margin, and safety allowance. The worker rechecks validity. If expiry is too close, refresh through the normal Neoism session before trying again; the lab will not rotate or overwrite credentials used by your live session. API-key and access-token values are redacted from stdout/stderr as the streams are captured, including chunk boundaries and JSON-escaped values. Credentials remain accessible to the contestant that needs them; do not treat a container as preventing exfiltration of its own authorized credential.

An illustrative expanded configuration uses ten repository tasks, two contestants, and two attempts each. It permits up to $5 for an individual trial while tracking a $20 **total API budget**, not $20 per task:

```sh
python3 neoism-agent/evals/lab.py run \
  --suite repository --repeat 2 \
  --allow-paid --budget-usd 20 --trial-budget-usd 5 \
  --use-neoism-auth --accept-soft-budget \
  --arms neoism-sol claude-sonnet --effort high
```

This example is **not** authorization to spend or consume subscription quota. Execution is refused without `--allow-paid`, both explicit limits, remaining authorized budget, and usable selected credentials. Before each API-billed trial its runtime limit is the smaller of the per-trial limit and remaining total API budget. Completed known API costs decrement the budget; unknown costs stop execution. `--prior-run DIRECTORY` counts that run's known API costs and its prior-run ancestry exactly once toward the same total. API trials are durably marked in-flight/accounting-incomplete before inference starts; the marker clears only after a known cost is recorded. Interrupted, incomplete, or inconsistent prior accounting is rejected rather than silently forgetting spend. Subscription trials do not debit fictitious dollars from the API budget; their per-trial value is only an optional catalog-estimate cutoff alongside the deadline, not quota pricing. `budget.json` records the ledger. Add `claude-opus` only if it fits the authorized experiment. `--task ID` can be repeated within the selected suite. `--seed` controls shuffled contestant order. Identical effort names do not imply identical compute across models. The repository suite allows ten minutes per trial for both setups and uses native default turn limits, rather than an artificial Neoism-only step cap.

### Dollar-limit caveat

Neoism does not currently expose a verified hard dollar cap. Its adapter observes reported costs across root and child sessions and requests abort when the trial threshold is reached. Usage arrives after model calls complete, so this can overshoot, including concurrent subagent calls. Claude's `--max-budget-usd` is also a runtime control, not a guarantee of exact invoiced spend. `--accept-soft-budget` acknowledges this limitation explicitly. **Use a provider-side enforced limit or budget-enforcing gateway if a hard monetary ceiling is required.** The runner will not silently describe an observed-cost cutoff as a hard cap.

Further trials stop on unknown API cost, adapter/instrumentation failure, a model mismatch (even when another error also occurred), or a budget overrun. A bounded subscription timeout or model error is still a failed trial, but does not abort the rest of the plan merely because subscription usage lacks a dollar invoice; incomplete token accounting is retained explicitly and never awarded a successful score. Interrupted API inference is conservatively treated as incomplete accounting, with observed spend preserved only as a lower bound. For OAuth, a successful trial requires complete positive token usage, and its report is explicitly marked `subscription (not USD-priced)`. Catalog dollar estimates are retained separately when present, not used to rank subscription spend against API invoices. Usage includes observed child sessions without counting step-finish usage a second time. Native API costs remain estimates; validate pricing and billing before using them for economic claims.

## Permission policy

Neither contestant requires human permission approval in the lab. Claude Code uses `--dangerously-skip-permissions`. Neoism receives an isolated global allow policy, root-session wildcard allow, and an explicit `external_directory: * -> allow` rule, which its native child-session inheritance preserves. A wildcard alone is not inherited by subagents. The adapter also auto-allows any remaining permission request belonging to the current conversation family and deterministically rejects questions, matching an unattended task rather than waiting for a human. Decisions are audited. These settings affect only disposable containers, never the user's normal Neoism/Claude configuration. Host mounts, read-only filesystems, resource limits, and separate verification remain the security boundary.

## Completion semantics

Neoism's root `/wait` endpoint can return while background subagents are still running. The adapter therefore polls the authoritative family runtime snapshot and waits for `execution.finished`, no active provider segments, and an empty running-background-job list under the full trial deadline. A temporarily idle parent is not task completion. Root and child usage is collected throughout; cancellation aborts the discovered family. Fixed short child-settlement timeouts are not used to score a trial.

## Isolation and artifacts

Each agent receives a fresh workspace and temporary HOME/config/state, no host home, no host Docker socket, and no verifier or reference solution. The original task seed is mounted read-only; contestant edits live in a 64 MiB container tmpfs, not a writable host mount. Both have the same image/tools and Docker limits: non-root UID, read-only image, dropped capabilities, no privilege escalation, two CPUs, 2 GiB RAM, PID limit, and bounded temporary storage. Captured stdout and stderr are each capped at 16 MiB; reaching a cap terminates the trial. Only a validated regular-file export (at most 500 files, 2 MiB per file, 8 MiB total) is reconstructed on the host. A hard outer timeout or killed container may prevent export, which makes the trial unsuccessful rather than grading the untouched seed. Noninteractive permission bypass is used only inside these containers. Neoism's question tool is disabled and the shared task suffix requests autonomous work.

Paid agents use Docker bridge networking to reach model APIs; this is **not an outbound allowlist or protection against access to host-network services**. Do not run hostile tasks on a sensitive host. Containers are not a security boundary for untrusted kernel exploits, and a model API credential necessarily exists inside its agent container. Verifiers have no network or credentials and mount the candidate workspace read-only. The trusted verifier supervisor runs as container uid0 with only SETUID/SETGID capabilities, receives hidden test source over stdin, and never imports candidate code. Each tested function runs in a separate uid/gid65534 subprocess with bounded JSON request/response, no supplementary groups, and a per-call deadline. Only the supervisor evaluates assertions and emits scores; candidate stdout cannot forge a successful verification event. Mutation probes check input/result independence across the process boundary. Each call starts a fresh process, so these smoke tasks do not test cross-call module state. Candidate symlinks, special files, excessive file counts, and large files are rejected before host-side artifact reads. The lab does not run candidate-controlled Git hooks on the host.

Ignored, private `runs/` directories retain task/source fingerprints, image IDs, execution plans, transcripts, stderr, workspace snapshots, diffs, verifier output, JSON results, and Markdown reports. Known selected access tokens/API keys and their JSON-escaped forms are redacted before captured log bytes reach disk. This is not a guarantee against arbitrary encodings or a malicious agent writing credentials into candidate files; traces can contain task data and other sensitive material, so do not publish them without review. Cleanup forcibly removes the specific trial container on completion or deadline; this is not a claim that Neoism's standalone server supports graceful shutdown. A killed host process or machine crash can leave containers; their names start with `neoism-eval-`.

```sh
python3 neoism-agent/evals/lab.py report neoism-agent/evals/runs/COMPARISON_DIRECTORY
```

Reports count acceptance failures, timeouts, adapter/model errors, and incomplete accounting as unsuccessful trials, preserve acceptance results separately, and never turn unknown costs into zero. They include success rate, observed total cost, cost per successful task (including failed-attempt costs when known), median successful duration, and paired outcomes for two-arm runs. Baseline/oracle calibration is kept separate from contestant scores. Partial/aborted plans must not be presented as completed experiments.
