# Neoism vs Claude Code: first completed comparison

## Result

All **40/40 planned trials completed**: ten private synthetic multi-file Python tasks, two independent attempts per setup. Neoism + GPT-6.1 Sol passed **20/20**; Claude Code + Sonnet 5.5 passed **19/20**. Claude Code finished substantially faster. One discordant attempt on a small Python suite is not strong evidence of a general quality advantage.

| Metric | Neoism + GPT-6.1 Sol | Claude Code + Sonnet 5.5 |
| --- | ---: | ---: |
| Successful attempts | 20/20 (100%) | 19/20 (95%) |
| Mean time, all attempts | 137.37 s | 40.49 s |
| Median time, successful attempts | 138.44 s | 43.97 s |
| Total attempt time | 2,747.44 s | 809.84 s |
| Cumulative input + output tokens | 2,874,875 | 2,696,916 |
| Tokens per attempt | 143,744 | 134,846 |
| Tokens per successful result, including failed-attempt usage | 143,744 | 141,943 |
| Reported cached-read share of input | 26.20% | 91.47% |
| Reported API cost for this suite | Subscription, not USD-priced | $2.0869 |

Claude Code completed its attempts in approximately **29.5% of Neoism's total time**, a **3.39x time ratio**. Neoism consumed approximately **6.60% more cumulative tokens**, not 3.39 times more. Once failed-attempt usage is included, tokens per successful result were close: approximately 144k versus 142k.

Reported Claude API cost including the earlier smoke test was **$2.2016861**, against the authorized $20 experiment budget. Runtime accounting is not a verified billing invoice. The subscription side is not labeled free and is not assigned a hypothetical API invoice.

## Only acceptance failure

Claude's first archive-manifest attempt returned `path_conflict` where the specified collision cases required `collision`. Four acceptance assertions failed, so that attempt did not pass the all-tests success criterion. Its second attempt passed. Every other task passed on both attempts for both setups. This was an error-contract mismatch, not evidence that the implementation allowed unsafe archive paths.

## Scope and controls

This compares complete setups, not harnesses in isolation: models, providers, tokenizers, and authentication/billing paths differ. The tasks are private synthetic Python projects, not real upstream issues or a recognized external benchmark. Each task has three interacting source modules and independent acceptance checks. Across the ten tasks there are 162 acceptance-test methods. Broken baselines failed real assertions and all reference solutions passed before model execution. The suite, instructions, verifier, installed agent versions, and image identity were frozen for the completed run.

Both contestants operated without human permission prompts in fresh isolated containers with the same tools, two CPU cores, 2 GiB memory, and ten-minute trial deadlines. Neoism used installed native agent 0.7.128; Claude Code was pinned to 2.1.292. The native-only artificial step cap was removed before this clean comparison. Earlier aborted lab-configuration attempts are preserved separately and excluded from these 40 scores.

## Usage interpretation

Token counts are cumulative across model turns and subagents, including repeated context and cache reads; they are not one context-window size. Native input excludes cached reads/writes and native output excludes separately recorded reasoning, so those components are added back once. Anthropic input is reconstructed as uncached input plus cache creation plus cache reads; its output already includes thinking and is not increased a second time. Different tokenizers and cache prices mean equal token counts are not equivalent compute or dollars.

The cache-share difference is measured, but it does not prove caching caused the full latency difference. Model/provider latency, request count, delegation, and tool execution also need examination. The separate source fix for Codex cache affinity was requested after this baseline run was underway; it was not built into the frozen benchmark image. Do not describe these numbers as post-fix performance or claim a speedup until a controlled same-model experiment measures it.

## What the execution traces show

Deduplicated assistant-message IDs total **323 for Neoism versus 125 for Claude Code**. Neoism has 224 root messages and 99 child messages, with **19 background exploration subagents**; no Claude subagents were observed. These are logical message counts, not proven HTTP request counts. Even excluding child messages, Neoism took a more interaction-heavy path.

Neoism recorded 541 model-visible tool-call IDs versus 112 for Claude. Native calls included 225 reads, 137 bash calls, 88 edits, 31 writes, 30 globs, 19 task launches, and 11 greps. Claude primarily used Bash, which can batch multiple operations. These are not equivalent-work units, so a 4.83x call-count ratio does not establish 4.83x wasted work. Native recorded 52 error-status tool calls versus nine Claude error results; some can be expected failing test runs, not unnecessary retries or final acceptance failures.

Native was slower on **all 20 matching attempts**, with per-attempt time ratios from 1.88x to 5.81x. Restricting to the 19 common passes still gives a 3.50x ratio of total times. Delegation is not the complete explanation: the second archive attempt had no native child and nine assistant IDs on each side, yet native took 94.09 s versus Claude's 44.01 s.

Native recorded synchronous tool intervals sum to only 38.94 s, so those recorded intervals alone do not explain the approximately 1,938-second overall wall-time gap. Its final provider-active counters sum to 2,033.09 s but report only root activity, despite child transcripts; the remaining 714.36 s cannot safely be labeled transport or orchestration overhead. Claude's CLI reports 761.76 s in `duration_api_ms`, but that is not necessarily the same measurement as native's counters. Comparable per-request latency/first-token distributions are unavailable.

The supported explanation is a combination of **more recorded model-message activity, aggressive background exploration, and substantially lower reported cache reuse**. The exact contribution of each, and of model/provider latency, remains unproven. The cache-affinity source patch is a focused follow-up, not evidence that the entire speed gap is fixed.

```sh
python3 neoism-agent/evals/trace_report.py neoism-agent/evals/runs/comparison-20261018T034905Z-064f1b8b
```

## Reproduction and artifacts

Completed run: `runs/comparison-20261018T034905Z-064f1b8b/`. Its private ignored artifacts contain `manifest.json`, `results.json`, `budget.json`, `report.md`, individual acceptance outputs, candidate diffs, and sanitized agent traces. Raw traces can still contain sensitive task data; do not publish them without review.

```sh
python3 neoism-agent/evals/lab.py report neoism-agent/evals/runs/comparison-20261018T034905Z-064f1b8b
python3 neoism-agent/evals/usage_report.py neoism-agent/evals/runs/comparison-20261018T034905Z-064f1b8b/results.json
```
