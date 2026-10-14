---
name: "First terminal cd frozen by performance shortcut"
description: "FIRST/root terminal cd must follow LOCAL workspace root; .119 nightly regression caused by cached-root early return, hotfix .2 pushed"
type: "bug"
scope: "project"
origin: "neoism-agent"
created: "2026-10-07"
updated: "2026-10-07"
---

REGRESSION introduced by this assistant's performance pass, owned and shipped by parent. Exact conversation ses_eeb832ccfffeD7Io3AMgoVLeBq; delegated general agent task ses_eeb6fb6eeffehBmenezDNKCfro titled Cache workspace and Git probes. Underlying model ID not exposed by task metadata, do not invent it. Bad commit94807cb7c shipped in v0.7.119-nightly.20261007.1/57f59e098: active_pane_workspace_root in desktop/src/screen/panes/grid_workspace.rs added !quick_ssh + existing active_workspace_root => return existing BEFORE root-terminal cwd discovery. This froze local Explorer/workspace cwd after first terminal cd. Parent reviewed/approved it and missed behavioral regression.
CRITICAL distinction: FIRST/ROOT terminal cd MUST update LOCAL active workspace/Explorer directory. SPLIT terminals stay local helpers and must not re-root. Joined shared workspaces keep HOST declared root. Only explicit force=true root changes persist daemon-owned workspace root; first-terminal cwd-follow force=false must not persist to daemon. Earlier memory 'workspace=declared root, terminal cd local' was overgeneralized: NEVER freeze local first-terminal root following as a performance optimization.
Fix commit51f1b3038f1f4aaaf13ceb365011ca0bfda28032 removes bad early-return, preserves unchanged-path normalization fast path, extracts terminal_workspace_root production selector with root-shell cwd precedence over cached fallback and lazy split isolation. Adds3tests repeated first-terminal cd transitions, split never probes cwd, missing-cwd fallback. User explicitly demanded fix/push/new real nightly, don't stop to apologize without acting.
New version v0.7.119-nightly.20261007.2 committed and atomically pushed to origin/nightly plus tag. Real Release Neoism run37632399001 https://github.com/parkers0405/neoism/actions/runs/37632399001 queued at write time. Servo remains excluded and original CI guard retained. Local test+check job /tmp/neoism-nightly-cwd-hotfix-{tests,check}.log in progress. Release watcher /tmp/neoism-nightly-cwd-hotfix-release-watch.log in progress. Do not claim published until run passes and isDraft=false verified. Stable remains0.7.118. Memory/scratch left uncommitted.
