# Official Turso 0.6.1 → 0.8.3 derived FTS fixture

This isolated workspace builds a **synthetic** old-format database with the official crates.io Turso 0.6.1 driver and storage engine. It does not patch or add dependencies to the application. Never point either helper at a real database. The producer refuses an existing output path. No generated database is checked in.

From the repository root, after the parent has completed the official 0.8.3 application dependency upgrade:

```sh
# Optional provenance check: these must be registry packages, not local/git patches.
cargo tree --manifest-path tools/fts-v1-fixture/Cargo.toml -i turso_core@0.6.1

# Choose a NEW path. Wait for the producer to exit successfully before consuming.
cargo run --manifest-path tools/fts-v1-fixture/Cargo.toml -- /tmp/neoism-official-061-fts.db

# The consumer uses the application's official 0.8.3 dependency and private Db API.
NEOISM_FTS_V1_FIXTURE=/tmp/neoism-official-061-fts.db \
  cargo test -p neoism-agent-server official_061_fts_fixture_migrates_to_v2 -- --ignored --nocapture

# Nonignored migration regression: real deferred-FK COMMIT failure, explicit rollback,
# writer release while the failed connection remains alive, retry and idempotency.
cargo test -p neoism-agent-server fts_format_migration_commit_failure_rolls_back_and_retries -- --nocapture

# Existing projection/search regression coverage (includes rollback and maintenance).
cargo test -p neoism-agent-server state::conversation_search::tests -- --nocapture
```

The producer writes 386 projection documents, including 384 page-sized synthetic message bodies in twelve transactions with matching canonical message JSON. It asserts that the official 0.6.1 FTS backing stores more than four database pages of bytes and records that metric; both engines verify all 384 common-token hits and unique boundary-document tokens. It also writes two canonical events: legacy TEXT and a compressed BLOB using the exact production envelope (`NEOEVT\0\0`, version 1, little-endian u64 decoded byte length, zlib stream). Shared deterministic JSON includes whitespace, Unicode, and unknown properties; compression uses isolated-helper flate2, not an application dependency addition.

The producer verifies a native FTS query plan and old-token hit, drains `PRAGMA wal_checkpoint(TRUNCATE)`, drops all connections/database handles, rejects a remaining nonempty WAL, and then exits. Only after that successful process exit is its database a cold fixture. Copying just the main file is intentional and safe only under those conditions. Keep its isolated generated Cargo.lock when preserving a fixture for reproducibility; inspect it/tree output to confirm registry provenance. This helper's nested `[workspace]` prevents application workspace patches from supplying a rejected vendor implementation.

The ignored consumer test requires the environment variable rather than silently skipping if a fixture is missing. It copies the cold file into a unique temporary directory, never opens the source database, verifies producer provenance, and uses the production DROP/CREATE/marker sequence inside a transaction that deliberately fails and rolls back. It cold-reopens and retries the production migration, verifies exact canonical JSON bytes plus canonical metadata, all document fields, dirty revisions, and both existing backfill records. Event storage class, exact encoded payload bytes, and every canonical event metadata column are included in every snapshot; production `event_storage::decoded_len`, `decode_json_bytes`, and `decode_event` validate both event representations before migration and after rollback, migration, and reopens. Events are appended at snapshot index 5, preserving the original canonical `[..2]`, changed-doc `[2]`, and unchanged-state `[3..]` slice assertions. Every search assertion checks the native FTS query plan to exclude scalar fallback. It tests failed and successful genuine UPDATE (old-token removal/new-token addition), persisted marker idempotency, real SessionStore startup/reopen, OPTIMIZE, DELETE, and a final reopen. Failed tests retain the temporary database for diagnosis; successful tests remove it.

## Migration contract and remaining validation

Turso 0.8.3's `turso_core/index_method/fts/format.rs` declares storage version 2. `fts/mod.rs::unsupported_format_error` explicitly documents that DROP INDEX bypasses opening an incompatible store. The application checks `conversation-search-fts-format-v2` in `data_migrations` under an immediate writer transaction; if incomplete/absent it drops only `conversation_search_fts`, recreates it over the existing docs, and commits the completion marker with it. No docs/canonical tables are cleared and no backfill marker is reset. Explicit connection-level BEGIN/COMMIT/ROLLBACK avoids the driver's consumed-Transaction deferred rollback path. Both rebuild and commit errors explicitly await rollback before busy retry; a rollback failure is terminal and cannot enter busy retry. The nonignored regression forces a real COMMIT failure with a deferred foreign key and verifies marker/data rollback, immediate writer acquisition from a second connection while the first is still alive, successful retry, and marker idempotency. First upgrade is synchronous and scales with the existing derived docs, not canonical conversation history.

No builds, fixture generation, tests, or live database opens were performed when adding this helper. The parent must run the commands above. Pending risks are actual official-driver compile/API compatibility, transactional rollback of old-format DROP/recreation, native UPDATE correctness, checkpoint behavior, and the complete application startup path. These are assertions to validate, not claimed passing results. The fixture exercises SQL failure/rollback/retry, not process-kill/power-loss recovery or concurrent application instances. It is synthetic, not a sanitized copy of user history.
