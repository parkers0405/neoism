# SWE-bench Pro V2 HARD-51 comparison lab

This directory builds a reproducible comparison of Neoism, OpenCode V2, and Claude Code using the same direct Anthropic model. It is separate from the private synthetic evaluation in `../RESULTS.md`.

## State and boundaries

The implementation and generated plan are for lab validation, not a published benchmark result. Paid execution defaults to disabled. Building or generating a plan must not launch a CLI, container, test suite, network probe, calibration job, or model request. The current build has not been lab-tested; earlier fixture results do not validate subsequent changes.

No public benchmark model attempts have been launched. The previous private benchmark's spending authorization does not authorize this experiment. Do not read credentials or enable inference until the new approval and lab evidence are complete.

## Locked experiment

| Item | Setting |
| --- | --- |
| Dataset | Official SWE-bench Pro V2 HARD-51 |
| Upstream commit | `66f92766bba642462d4bbe5479e83f91f9211862` |
| Tasks | 51 official hard task IDs |
| Harnesses | Neoism `0.7.128`, OpenCode V2 `2.0.32`, Claude Code `2.1.292` |
| Provider/model | Direct Anthropic, `claude-sonnet-5-5` |
| Thinking/effort | Adaptive thinking, high effort |
| Per-request output ceiling | 32,000 tokens |
| Attempts | Exactly one fresh native session per task per harness: 153 slots |
| Agent deadline | 3,000 seconds total; inference stops at 2,955 seconds, reserving 45 seconds for cancellation/accounting |
| Scheduling | Sequential, seeded task order, each harness runs first/second/third 17 times |
| Automatic retries | Zero |
| Agent network | Only `api.anthropic.com`; no browsing tools |
| Setup/verifier network | Official task policy |
| Authoritative score | Fresh pristine-image patch replay and official verification |
| Tooling | Harbor `0.22.0`, Modal `1.5.1`; local Docker is the default backend |

One attempt may contain multiple native tool/model calls, subagents, and compaction. It does not mean one API request. The runner must never replace a failed/interrupted attempt with a fresh session.

## Native harness fidelity

All harnesses retain their own CLI, prompts, tools, permissions, and native agent loop. They run with private HOME/config/state directories and no host plugins or MCP servers. Global OpenCode V1, personal settings, and saved authentication remain unchanged. Every model selector, including small/fast/title/summary/compaction/subagent routes, is pinned to the same model.

OpenCode V2 is the privately installed `@opencode/cli@2.0.32`, not `opencode-ai` V1. Neoism's standalone configuration lives under `$XDG_CONFIG_HOME/agent/agent.json`. Claude uses isolated settings rather than `--bare`, because bare mode removes native subagents.

Frozen executable copies are addressed by SHA256 in `assets.lock.json`. Neoism's private loader/library bundle avoids upgrading official task-image libraries and does not export `LD_LIBRARY_PATH` into task tools. Compatibility checks on unrelated base images are not evidence that every official task image works.

A transparent loopback gateway observes the unchanged Anthropic protocol for all three harnesses. This is request auditing and budget admission, not cross-provider translation or a replacement agent loop. Requests with model/reasoning drift, unsupported routing, or incomplete accounting fail closed.

## Accounting and spend controls

The run ledger reserves a per-trial budget before dispatch. Within a trial, the provider gateway reserves a conservative full-context plus maximum-output request allowance before forwarding. Only authoritative complete usage can release that allowance. Missing/interrupted usage retains its reservation and blocks subsequent inference. A failed native session can still have a captured partial patch graded in a fresh sandbox; grading never authorizes another model attempt.

Dollar estimates use the frozen catalog pricing snapshot. Native CLI-reported cost is retained separately. Neither is a provider invoice. Binding budget halts must be reported as budget-limited, not as unrestricted harness outcomes. Pricing changes, account-specific surcharges, and activity outside this lab are not controlled by a local ledger. Set a provider-side account/project spending limit if an actual billing ceiling is required. Use a dedicated benchmark API credential rather than a shared production account.

The approval must bind the exact plan hash, model, dataset commit, attempt count, expiration, total cap, and per-trial cap. The explicit inference environment flag is also required. A positive budget or an `approved` boolean alone is insufficient. Unknown accounting, model drift, missing patch capture, or a dispatched-but-unreconciled ledger entry stops the run rather than guessing a cost or retrying.

The gateway reserves conservatively, so it may refuse a request even when its eventual actual price would have fit. For the current catalog, full-context 1-hour-cache-write reservation plus 32,000 output tokens is approximately $4.32 per concurrent request. This is an admission bound, not a measured average trial cost. Choose per-trial caps only after lab validation and an explicit budget decision.

## Artifact map

- `protocol.json`: fixed experiment policy; default paid execution remains disabled.
- `hard51.lock.json`: official IDs, upstream checksums, source task policies and resource requirements.
- `images.lock.json`: immutable task-image digests; missing layer metadata means unknown download size, not zero.
- `assets.lock.json`: frozen CLI and private runtime hashes.
- `model-catalog.lock.json`: catalog snapshot hash, model capabilities and public pricing metadata.
- `plan.json`: 153 slots, immutable job/regrade config hashes and source provenance.
- `.cache/hard51/`: derived task definitions; only image tags are replaced with digests.
- `.cache/job-configs/` and `.cache/regrade-configs/`: one-task jobs so source patch selection is unambiguous.
- `neoism_harbor/`: installed-agent adapters, credential-free probe, replay agent and native runtime supervisors.
- `lab.py`: explicitly gated infrastructure/testing/calibration commands; no paid inference fallback.
- `readiness.py`: evidence and immutable-plan admission rules.
- `run.py`: sequential paid-run admission, durable ledger and result reporting.
- `runs/`: ignored private lab evidence, approvals/admissions, logs, source jobs and pristine regrades.

Do not publish raw credentials, private approval/admission files, or unreviewed traces. Keep raw provider usage, native reported cost, catalog estimates, partial accounting and both grades distinguishable in any exported report.

## Build and lab workflow

Work from `neoism-agent/evals/public`. `uv sync --frozen` installs only the pinned Python tooling. `prepare.py plan --seed 0` generates local immutable configs; it does not run tests or agents. Image metadata resolution is a separate explicit operation, `prepare.py images --execute`, which contacts the registry but does not pull or execute images. Regenerate the plan after changing image metadata, executable sources, dependency locks, or assets. Such changes invalidate earlier approval and lab evidence.

The commands below are the future lab phase. They are documented here, not executed by building this directory. Omitting `--execute` from a lab operation only describes it. Each executed operation writes fresh evidence under `runs/lab/`; failures must not be relabeled as passes.

```bash
.venv/bin/python lab.py unit --execute
.venv/bin/python lab.py wire-tests --execute
.venv/bin/python lab.py network-probe --execute
.venv/bin/python lab.py install-only --execute
.venv/bin/python lab.py calibration --execute
.venv/bin/python lab.py pristine-replay --execute --source-dir <calibration-evidence-directory>
.venv/bin/python readiness.py --evidence lab-evidence.json
```

Run image metadata preparation before container work. The 51 pinned images contain 61,763,016,086 bytes (about 57.5 GiB) of unique compressed layers; unpacked storage is larger and is not yet measured. Container checks require their images to be explicitly staged first. Image downloads and unpacked task environments can be large. `lab.py pull-images --execute` additionally requires an explicit `--budget-gib` and maintains `--reserve-gib` free disk space. It never prunes user images. Use repeatable `--task <official-ID>` on image pulls, network probes, installation, and calibration to stage representative or bounded batches instead of requiring the full cache at once. Successful same-plan evidence accumulates, but readiness still requires full51 network/oracle/nop/replay coverage. Do not bypass disk admission by manually downloading the entire suite or blindly rerunning failed calibration.

Wire checks use dummy credentials, frozen native executables and a loopback fake provider. They must prove actual root, auxiliary, child and reasoning/output serialization without real API calls. Network probes make unauthenticated metadata requests, never inference requests. Install-only checks stop at CLI installation/version readiness. Oracle/empty-patch calibration and pristine replay use the official test machinery, not models.

The lab must separately establish native background-family completion and automatic-compaction behavior. OpenCode background-child completion is not yet authoritative: an observed background child causes the adapter to reject accounting rather than silently omit work. Do not treat selector fixtures as proof of automatic compaction or assume the paid comparison is validated because configuration generation succeeded.

After all lab checks pass, `run.py approval-template` prints the exact unapproved approval schema with the current plan hash. Fill in a newly approved total/per-trial budget and UTC expiration only after that decision is made. The paid command remains a distinct operation:

```bash
NEOISM_PUBLIC_EVAL_ALLOW_INFERENCE=1 .venv/bin/python run.py run --execute --approval <approval.json> --readiness lab-evidence.json
```

This command is documentation, not current authorization. `run.py plan`, `run.py status`, and `run.py report` do not read provider authentication or launch inference. Never remove or edit `runs/public-ledger.json`, admission-consumed markers, or source job directories to get another attempt. An unresolved dispatched slot must be reconciled, not retried. Reports retain partial/infrastructure states and withhold complete-subset claims until all 153 pristine grades and accounting records are valid.

## Reporting rules

Call this a HARD-51 subset result, never a full 642-task SWE-bench Pro V2 score. Publish the exact model, effort, output cap, versions, image/code hashes, one-attempt policy, date, failure categories, durations and cost basis. Missing or infrastructure-failed verification is not a demonstrated solution failure and must be reported separately. Use paired task outcomes when comparing harnesses; close subset scores do not establish universal superiority.

The earlier private Sol-versus-Sonnet benchmark used a different task set, model comparison and attempt policy. Do not combine its scores, cache measurements, cost, or authorization with this experiment.
