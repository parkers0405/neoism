# One-line patch latency probe, 2026-11-18

Six sequential edits alternated `after` and `before` in the same disposable text file in the Neoism checkout. Each patch matched the preceding edit's output. Both tools succeeded on every edit. No model, subagent or network request was part of the timed commands.

| Invocation | Six edits | Mean per edit |
| --- | ---: | ---: |
| Running Neoism native `apply_patch` via one compose batch | 93.29 ms | 15.55 ms |
| Installed Codex CLI 0.163.0 binary, `--codex-run-as-apply-patch`, new process per edit | 3171.90 ms | 528.65 ms |

Neoism's batch was bracketed by shell clock samples; two adjacent clock samples with no patches between them took 62.53 ms. No timing overhead was subtracted. Codex's batch was timed with Python `perf_counter` around six checked subprocess invocations. These are invocation-path measurements, not a ratio of patch algorithm speeds or a comparison of full agent sessions. They exclude model deliberation, time between model requests and UI rendering. A spot check of the Neoism response showed cached diagnostics from unrelated Rust files even for this text edit; their latency impact was not isolated.