---
name: "RAM overload — measured RA indexing plus Cargo-check fanout"
description: "Live capture: one daemon-owned RA + its all-target Cargo children reached 12.70GiB PSS; real swap/PSI. Earlier test fanout also exists, not sole culprit."
type: "bug"
scope: "project"
origin: "neoism-agent"
created: "2026-09-06"
updated: "2026-09-06"
---

## Measured 2026-09-05 Linux RAM investigation

Diagnostic-only script added: `scripts/capture-memory-linux.py` (JSONL; 10s interval, 900s monotonic/awake duration, 16MiB cap, 0600 exclusive output; no builds/config edits/signals/raw argv/env dumps). Capture `/tmp/neoism-memory-20260905-2051.jsonl`, 90 samples, 2.24 MB. Wall time 20:51–21:30 because laptop suspended 21:01:48–21:26:28. Initial historical conclusion focused Cargo test fanout; LIVE CAPTURE materially refined it.

At 20:55:45 daemon PID 1371556 spawned ONE rust-analyzer PID 1657505 rooted /home/parkersettle/projects/neoism, plus two rust-analyzer-p proc-macro helpers (NOT three independent LSP servers). At 20:56:05 main RA PSS 6,144,867 KiB (~5.86GiB), its build descendants 5,492,360KiB, macro helpers 258,238KiB. At 20:58:06 same RA family totaled 13,319,947 KiB PSS (~12.70GiB): main 4,439,316, proc macros 219,848, Cargo/compiler descendants 8,660,783. RA+helpers additionally had 1,953,192 KiB swapped. Parent chains prove peak compilers belonged to RA cargo check --all-targets, NOT agent test builds. Main RA parent daemon, cargo check PID1676985 parent RA; 18 compiler/linker processes at sampled PSS peak. Peak compiler/linker count 24. At least 12 distinct RA-owned Cargo check --all-targets PIDs sampled over ~5 min (restart trigger not proven). Independent agent Cargo checks also observed parent daemon. All Neoism comm=neoism processes combined max PSS ~728MiB during capture; does not establish long-term absence of leaks.

Host MemAvailable dropped 22.11->12.02GiB; real PSI stalls (some avg10 6.50% before suspend), no oom_kill increment. Across whole capture pswpout +1,676,371 pages (~6.39GiB), pswpin +310,663 (~1.19GiB), including suspend/resume activity; do not attribute all these bytes to RA. At end RA family PSS ~2.51GiB plus VmSwap ~3.84GiB. Host was pressured/swapping but NOT proven exhausted/OOM.

Earlier history: kernel journald pressure at 11:35:05/07,12:06:00,14:25:14. Saved historical ps artifact `~/.cache/neoism/tool-output/tool-evt_0726ef0e1001dIVOblzDxkEW74.txt` at11:37:47 shows 4 cargo,12 rustc,4 rust-lld (30-row cap); many daemon integration-test binaries. Agent session ses_f8dd17468ffe0zK5Qe2MoBopKT launched triple background tests 11:32:51, again 11:35:36. At12:05:14–12:06:39 `cargo test -p neoism-workspace-daemon workspace::tests::editor_surface_registry_roundtrip --no-fail-fast` omitted --lib, causing integration target compilation. Historical snapshots lack RAM, so cannot assign those earlier pressure events definitively, especially14:25.

Actual config:24 CPUs; no jobs limit .cargo/config.toml; no user/ancestor Cargo config found; no CARGO_BUILD_JOBS in desktop/daemon env; no user/workspace Rust LSP override found. Dev profile incremental/unoptimized/full debug confirmed historical rustc -C debuginfo=2. Current main daemon VmHWM2.09GiB but initial PSS262MiB.

Next target is shared RA indexing footprint + RA-owned all-target check concurrency/restarts; do NOT conclude each agent duplicates LSP or that test-build fanout alone explains RAM. No runtime fixes/config changes implemented.
