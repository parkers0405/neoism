---
name: "History I/O candidate verified, activation pending"
description: "History I/O candidate built, hash b8da1984;35 focused tests pass; NOT installed/restarted; live activation and actual I/O validation pending"
type: "reference"
scope: "project"
origin: "Final build observation and test execution"
created: "2026-10-25"
updated: "2026-10-25"
---

Verified desktop candidate exists at `/home/parkersettle/projects/neoism/target/debug/neoism`, reports `neoism 0.7.139`, SHA256 `b8da1984b2f280bf8fe9f37a986fae94349233d1e79225fb207c4962fb77f75b`. User's terminal build completed it; our competing Cargo jobs were canceled so user's build was left alone. File freshness checked against changed state/search/interaction/tool-runtime sources. HEAD observed at final verification92ceca651; checkout contains uncommitted scoped-history/no-op-FTS fixes, so do not attribute artifact solely to that commit.

Final test executable `target/debug/deps/neoism_agent_server-100b25d4dfdb190e` passed35 focused tests: descendant/family4, search21, interactions3, pending completion1, live baselines5, raw late-baseline reconstruction1. Full lib test binary compiled successfully. Separate broader session_actions run had4 unresolved lifecycle failures; do not call entire suite clean or those failures preexisting without proof. Independent review found no actionable correctness findings in this patch.

Candidate was NOT installed or launched as a desktop; only `--version` executed. Installed `/home/parkersettle/.local/bin/neoism` SHA256 at verification `4bee566f7eb1e42dcc504d93cf089e09deb2bd3e8c5292e00fb3b0bbf3f68aa0`, running GUI PID4790 and daemon4867 remained untouched. No live DB data mutations/cleanup or retention switch. User strongly requires preserving live click-away/back and subagent navigation; do not restart active runs automatically. Activation needs user-approved safe restart point, then actual before/after daemon I/O measurement. Candidate is debug-profile, so do not blindly replace installed release app; respect user's own build/dev loop.

Changes are scoped canonical child seeks, cycle-safe task-tree walk, pending completion/interaction authorization scoped to relevant IDs, no-op FTS projection suppression with transactional revision/source/projection guards, and separate native-index-change accounting so dirty cleanup cannot rearm OPTIMIZE. Raw journal retention and genuine-change Turso FTS catalog scan amplifier remain unresolved. Read `bug_history_queries_fts_noop_and_replay_safety.md` and `docs/agent-database-activity.md` before continuing.
