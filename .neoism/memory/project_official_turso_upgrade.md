---
name: "Official Turso upgrade and lossless event storage"
description: "Official Turso0.8.3 newest stable pinned; vendor removed; FTS fixture migration and codec checks pending; do not activate live"
type: "project"
scope: "project"
origin: "Omarchy lag task and user rejection of engine fork"
created: "2026-10-25"
updated: "2026-10-25"
---

## Official Turso upgrade in progress

User explicitly rejected vendoring/forking Turso and asked for newest release. Removed this task's vendor/turso_core directory, root crates.io path override and vendor workspace exclusion, vendor-specific NOTICE entry and docs/turso-fts-catalog-patch.md. Cargo.toml now pins official crates.io turso = "=0.8.3"; cargo update -p turso --precise 0.8.3 succeeded. Direct crates.io API check confirms max_stable_version/default_version 0.8.3; newest publication 0.9.0-pre.1 is prerelease. Do not claim 0.8.3 is newest including prereleases. Server sqlx use is read-only external OpenCode SQLite interoperability in external_agent/catalog.rs, not duplicate Neoism canonical storage. SQL remains Turso's query language.

Official 0.8.3 FTS format v2 requires derived conversation_search_fts DROP/CREATE before search workers. General agent ses_e8b899b9effeYBljDxUOi1fnPI owns conversation_search.rs and isolated old-format fixture helpers, no Cargo builds allowed for child. Must prove cold official-0.6.1 fixture opens, drop/rebuild, canonical byte preservation, genuine update, rollback/retry, reopen, DELETE/OPTIMIZE. No live DB opens/mutations/restarts allowed. Another general child ses_e8b866080ffec9lU8F5UseQwjQ owns session_actions.rs investigating four unresolved lifecycle failures, also no builds.

Parent build job_17477a50a001i9vWOeipKicRYl runs cargo check --locked --profile activation -p neoism-agent-server --tests. Only one coordinated pipeline; do not start competing Cargo jobs. Previous own vendored build process group368592 explicitly terminated before dependency deletion; cancellation is intentional, not a regression failure.

## Pending second-stage codec verification

New state/event_storage.rs losslessly compresses event JSON into versioned zlib BLOBs; TEXT remains readable. Background compactor retains every seq/event, bounded128 rows/2MiB, nonblocking single writer attempt, no vacuum/checkpoint/deletion. state.rs async event_statements integrated; append_event_with_owner passes &aggregate_id. Replay sequence-only seeks, point reads with scope revalidation, max128 rows, budgets BOTH stored and declared decoded bytes8MiB (one oversized first event), blocking decode. P2 corrupt-length memory hole fixed by stored-byte budget. Regression replay_budgets_stored_bytes_before_trusting_declared_length added to codec tests. HTTP compressed_replay_crosses_byte_and_row_pages_before_live_family_baseline added. None of these new tests has completed yet. Older backend rollback incompatible with persisted BLOBs without consistent pre-upgrade backup or verified offline decompression.

First-stage35 focused tests previously passed; four broader lifecycle failures remain unattributed. First-stage debug candidate hash b8da1984b2f280bf8fe9f37a986fae94349233d1e79225fb207c4962fb77f75b excludes new codec/official upgrade. No second-stage activation. Production GUI4790/daemon4867 owns current controller; don't kill/restart while live work/agents/tools exist. Custom activation profile uses production paths without release LTO; output target/activation. Must safely activate in agreed quiet window and measure comparable live I/O before claiming lag resolved.

See bug_history_queries_fts_noop_and_replay_safety.md and reference_history_io_candidate_verified.md for first-stage findings. docs/agent-database-activity.md now describes official engine and codec, marks verification pending.
