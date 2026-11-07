//! Recover auxiliary agent state without dropping potentially shared damaged pages.

use super::{text, Db, DbRow, SqlValue};
use anyhow::Context;
use futures::FutureExt;

const TABLES: &[(&str, &[(&str, &str)])] = &[
    (
        "prompt_queue",
        &[
            ("id", "text"),
            ("session_id", "text"),
            ("position", "integer"),
            ("request_json", "text"),
            ("created", "integer"),
            ("kind", "text"),
        ],
    ),
    (
        "execution_activity",
        &[
            ("root_session_id", "text"),
            ("execution_id", "text"),
            ("root_message_id", "text"),
            ("completed_ms", "integer"),
            ("revision", "integer"),
            ("updated", "integer"),
        ],
    ),
    (
        "execution_session_activity",
        &[
            ("root_session_id", "text"),
            ("execution_id", "text"),
            ("session_id", "text"),
            ("completed_ms", "integer"),
        ],
    ),
    (
        "execution_activity_segments",
        &[
            ("segment_id", "text"),
            ("root_session_id", "text"),
            ("execution_id", "text"),
            ("owner_instance_id", "text"),
            ("session_id", "text"),
            ("started_at", "integer"),
        ],
    ),
    (
        "execution_activity_owners",
        &[("owner_instance_id", "text"), ("heartbeat_at", "integer")],
    ),
    (
        "conversation_search_dirty",
        &[
            ("key", "text"),
            ("session_id", "text"),
            ("message_id", "text"),
            ("revision", "text"),
            ("ready_at", "integer"),
        ],
    ),
    (
        "event_sequences",
        &[("aggregate_id", "text"), ("seq", "integer")],
    ),
];

fn quote(value: &str) -> String {
    format!("\"{}\"", value.replace('"', "\"\""))
}

fn corrupt(error: &anyhow::Error) -> bool {
    let message = format!("{error:#}").to_ascii_lowercase();
    [
        "invalid bookkeeping",
        "invalid page type",
        "database disk image is malformed",
        "database corruption",
        "database is corrupt",
        "malformed record",
        "inconsistent overflow chain",
        "bookkeeping page reader panicked",
        "saved agent state needs repair",
    ]
    .iter()
    .any(|needle| message.contains(needle))
}

async fn validate(db: &Db, table: &str, columns: &[(&str, &str)]) -> anyhow::Result<()> {
    let read = async {
        // Inspect both the table and its indexes: a covering index can be damaged
        // even when the underlying table scan still succeeds.
        let mut scans = vec!["NOT INDEXED".to_string()];
        for index in db
            .fetch_all(&format!("PRAGMA index_list({})", quote(table)), vec![])
            .await?
        {
            // A partial index cannot serve an unconstrained full-table query.
            if index.get_i64("partial")? != 0 {
                continue;
            }
            scans.push(format!("INDEXED BY {}", quote(&index.get_str("name")?)));
        }
        let projection = columns
            .iter()
            .map(|(name, _)| quote(name))
            .collect::<Vec<_>>()
            .join(", ");
        for scan in scans {
            let rows = db
                .fetch_all(
                    &format!("SELECT {projection} FROM {} {scan}", quote(table)),
                    vec![],
                )
                .await?;
            for row in rows {
                for (name, expected) in columns {
                    let valid = matches!(
                        (row.value(name)?, *expected),
                        (SqlValue::Text(_), "text") | (SqlValue::Integer(_), "integer")
                    );
                    anyhow::ensure!(valid, "invalid bookkeeping field {table}.{name}");
                }
                if table == "prompt_queue" && row.get_str("kind")? != "compact" {
                    let request = row.get_str("request_json")?;
                    anyhow::ensure!(
                        serde_json::from_str::<neoism_agent_core::PromptRequest>(
                            &request
                        )
                        .is_ok(),
                        "invalid bookkeeping prompt request"
                    );
                }
            }
        }
        Ok::<_, anyhow::Error>(())
    };
    std::panic::AssertUnwindSafe(read)
        .catch_unwind()
        .await
        .map_err(|_| anyhow::anyhow!("bookkeeping page reader panicked"))?
}

pub(super) async fn recover(db: &Db) -> anyhow::Result<()> {
    for (table, columns) in TABLES {
        let Some(schema) = db
            .fetch_optional(
                "SELECT sql FROM sqlite_master WHERE type = 'table' AND name = ?",
                vec![text(*table)],
            )
            .await?
        else {
            continue;
        };
        match validate(db, table, columns).await {
            Ok(()) => continue,
            Err(error) if corrupt(&error) => {}
            Err(error) => {
                return Err(error).context("could not inspect agent bookkeeping")
            }
        }

        if *table == "event_sequences" {
            recover_journal(db).await?;
            continue;
        }

        let token = neoism_agent_core::Id::ascending(neoism_agent_core::IdKind::Event)
            .to_string();
        let quarantine = format!("recovery_{table}_{token}");
        let mut statements = vec![
            (
                format!(
                    "ALTER TABLE {} RENAME TO {}",
                    quote(table),
                    quote(&quarantine)
                ),
                vec![],
            ),
            (schema.get_str("sql")?, vec![]),
        ];
        // Keep the old indexes with the quarantine. Dropping corrupt trees can
        // free pages that overlap other tables, so new indexes get fresh names.
        let indexes = db.fetch_all(
            "SELECT name, sql FROM sqlite_master WHERE type = 'index' AND tbl_name = ? AND sql IS NOT NULL",
            vec![text(*table)],
        ).await?;
        for (number, index) in indexes.into_iter().enumerate() {
            statements.push(recreate_index(&index, &token, number)?);
        }
        db.execute_transaction(statements)
            .await
            .context("could not preserve damaged agent bookkeeping")?;
        validate(db, table, columns)
            .await
            .context("recreated agent bookkeeping is not readable")?;
        tracing::warn!(
            table,
            quarantine,
            "Recovered damaged agent bookkeeping; conversations preserved"
        );
    }
    Ok(())
}

async fn recover_journal(db: &Db) -> anyhow::Result<()> {
    let high = db
        .fetch_scalar_i64("SELECT COALESCE(MAX(seq), 0) FROM events", vec![])
        .await?
        .max(super::event_retention::journal_state(db).await?.high_water);
    let checkpoint = db.fetch_optional(
        "SELECT seq,event_id,tenant_id,kind,aggregate_id,aggregate_seq,owner_id,session_id,event_json,created FROM events ORDER BY seq DESC LIMIT 1", vec![],
    ).await?;
    let token =
        neoism_agent_core::Id::ascending(neoism_agent_core::IdKind::Event).to_string();
    let mut statements = Vec::new();
    for table in ["events", "event_sequences"] {
        let schema = db
            .fetch_optional(
                "SELECT sql FROM sqlite_master WHERE type='table' AND name=?",
                vec![text(table)],
            )
            .await?
            .context("missing event journal schema")?;
        statements.push((
            format!(
                "ALTER TABLE {} RENAME TO {}",
                quote(table),
                quote(&format!("recovery_{table}_{token}"))
            ),
            vec![],
        ));
        statements.push((schema.get_str("sql")?, vec![]));
    }
    for (number, index) in db.fetch_all(
        "SELECT sql FROM sqlite_master WHERE type='index' AND tbl_name='events' AND sql IS NOT NULL", vec![],
    ).await?.iter().enumerate() {
        statements.push(recreate_index(index, &token, number)?);
    }
    if let Some(mut row) = checkpoint {
        // Seed both the persisted and in-memory AUTOINCREMENT allocators. This
        // checkpoint is not replayed because it is at the expired cursor floor.
        row.values[0] = super::int(high);
        statements.push(("INSERT INTO events(seq,event_id,tenant_id,kind,aggregate_id,aggregate_seq,owner_id,session_id,event_json,created) VALUES (?,?,?,?,?,?,?,?,?,?)".into(), row.values));
    }
    // A jump above the global high-water mark cannot reuse an aggregate's old
    // counter. Old cursors are explicitly expired, forcing a canonical snapshot.
    for row in db.fetch_all("SELECT id FROM sessions", vec![]).await? {
        statements.push((
            "INSERT INTO event_sequences(aggregate_id,seq,owner_id) VALUES (?,?,NULL)"
                .into(),
            vec![text(row.get_str("id")?), super::int(high)],
        ));
    }
    statements.push(("INSERT INTO event_journal_state(id,high_water,replay_floor) VALUES(1,?,?) ON CONFLICT(id) DO UPDATE SET high_water=excluded.high_water,replay_floor=excluded.replay_floor".into(), vec![super::int(high), super::int(high)]));
    db.execute_transaction(statements).await?;
    tracing::warn!(high_water = high, "Recovered damaged event journal; saved conversations preserved and replay cursors refreshed");
    Ok(())
}

fn recreate_index(
    index: &DbRow,
    token: &str,
    number: usize,
) -> anyhow::Result<(String, Vec<SqlValue>)> {
    let sql = index.get_str("sql")?;
    let upper = sql.to_ascii_uppercase();
    let on = upper
        .find(" ON ")
        .context("unsupported bookkeeping index definition")?;
    let unique = if upper.starts_with("CREATE UNIQUE INDEX") {
        "UNIQUE "
    } else {
        ""
    };
    Ok((
        format!(
            "CREATE {unique}INDEX {}{}",
            quote(&format!("recovered_{token}_{number}")),
            &sql[on..]
        ),
        vec![],
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{atomic::AtomicBool, Arc};
    use tokio::sync::Mutex;

    async fn database() -> Db {
        Db {
            database: turso::Builder::new_local(":memory:").build().await.unwrap(),
            write_gate: Arc::new(Mutex::new(())),
            fail_next_assistant_insert: Arc::new(AtomicBool::new(false)),
        }
    }

    async fn queue(db: &Db) {
        db.execute("CREATE TABLE prompt_queue (id TEXT PRIMARY KEY, session_id TEXT NOT NULL, position INTEGER NOT NULL, request_json TEXT NOT NULL, created INTEGER NOT NULL, kind TEXT NOT NULL DEFAULT 'prompt')", vec![]).await.unwrap();
        db.execute(
            "CREATE INDEX queue_session ON prompt_queue(session_id, position)",
            vec![],
        )
        .await
        .unwrap();
        db.execute("CREATE UNIQUE INDEX queue_compact ON prompt_queue(session_id) WHERE kind = 'compact'", vec![]).await.unwrap();
    }

    #[tokio::test]
    async fn bookkeeping_recovery_preserves_bad_queue_and_history() {
        let db = database().await;
        queue(&db).await;
        db.execute(
            "CREATE TABLE messages (id TEXT PRIMARY KEY, body TEXT)",
            vec![],
        )
        .await
        .unwrap();
        db.execute(
            "INSERT INTO messages VALUES ('message', 'keep my history')",
            vec![],
        )
        .await
        .unwrap();
        db.execute(
            "INSERT INTO prompt_queue VALUES ('bad', X'00', 0, '{}', 0, 'prompt')",
            vec![],
        )
        .await
        .unwrap();
        recover(&db).await.unwrap();
        assert_eq!(
            db.fetch_scalar_i64("SELECT count(*) FROM prompt_queue", vec![])
                .await
                .unwrap(),
            0
        );
        let preserved = db.fetch_all("SELECT name FROM sqlite_master WHERE type='table' AND name LIKE 'recovery_prompt_queue_%'", vec![]).await.unwrap();
        assert_eq!(preserved.len(), 1);
        let name = preserved[0].get_str("name").unwrap();
        assert_eq!(
            db.fetch_scalar_i64(
                &format!("SELECT count(*) FROM {}", quote(&name)),
                vec![]
            )
            .await
            .unwrap(),
            1
        );
        assert_eq!(
            db.fetch_optional("SELECT body FROM messages", vec![])
                .await
                .unwrap()
                .unwrap()
                .get_str("body")
                .unwrap(),
            "keep my history"
        );
        db.execute(
            "INSERT INTO prompt_queue VALUES ('first', 'session', 0, '{}', 0, 'compact')",
            vec![],
        )
        .await
        .unwrap();
        assert!(db.execute("INSERT INTO prompt_queue VALUES ('second', 'session', 1, '{}', 0, 'compact')", vec![]).await.is_err());
    }

    #[tokio::test]
    async fn bookkeeping_recovery_keeps_valid_execution_state_and_is_idempotent() {
        let db = database().await;
        db.execute("CREATE TABLE execution_activity_owners (owner_instance_id TEXT PRIMARY KEY, heartbeat_at INTEGER NOT NULL)", vec![]).await.unwrap();
        db.execute(
            "INSERT INTO execution_activity_owners VALUES ('owner', 123)",
            vec![],
        )
        .await
        .unwrap();
        recover(&db).await.unwrap();
        recover(&db).await.unwrap();
        assert_eq!(
            db.fetch_scalar_i64(
                "SELECT heartbeat_at FROM execution_activity_owners",
                vec![]
            )
            .await
            .unwrap(),
            123
        );
        assert_eq!(
            db.fetch_scalar_i64(
                "SELECT count(*) FROM sqlite_master WHERE name LIKE 'recovery_%'",
                vec![]
            )
            .await
            .unwrap(),
            0
        );
        db.execute(
            "UPDATE execution_activity_owners SET heartbeat_at = X'00'",
            vec![],
        )
        .await
        .unwrap();
        recover(&db).await.unwrap();
        recover(&db).await.unwrap();
        assert_eq!(
            db.fetch_scalar_i64("SELECT count(*) FROM execution_activity_owners", vec![])
                .await
                .unwrap(),
            0
        );
        assert_eq!(db.fetch_scalar_i64("SELECT count(*) FROM sqlite_master WHERE type='table' AND name LIKE 'recovery_%'", vec![]).await.unwrap(), 1);
    }

    #[tokio::test]
    async fn bookkeeping_recovery_preserves_pending_compact_controls() {
        let db = database().await;
        queue(&db).await;
        db.execute("INSERT INTO prompt_queue VALUES ('compact', 'session', -1, '{}', 0, 'compact')", vec![]).await.unwrap();
        db.execute("INSERT INTO prompt_queue VALUES ('prompt', 'session', 0, '{\"parts\":[]}', 1, 'prompt')", vec![]).await.unwrap();
        recover(&db).await.unwrap();
        assert_eq!(
            db.fetch_scalar_i64("SELECT count(*) FROM prompt_queue", vec![])
                .await
                .unwrap(),
            2
        );
    }

    #[tokio::test]
    async fn bookkeeping_recovery_rebuilds_journal_without_reusing_sequences() {
        let db = database().await;
        db.execute("CREATE TABLE sessions (id TEXT PRIMARY KEY)", vec![])
            .await
            .unwrap();
        db.execute("INSERT INTO sessions VALUES ('session')", vec![])
            .await
            .unwrap();
        db.execute("CREATE TABLE event_sequences (aggregate_id TEXT PRIMARY KEY, seq INTEGER NOT NULL, owner_id TEXT)", vec![]).await.unwrap();
        db.execute(
            "INSERT INTO event_sequences VALUES ('session', X'00', NULL)",
            vec![],
        )
        .await
        .unwrap();
        db.execute("CREATE TABLE events (seq INTEGER PRIMARY KEY AUTOINCREMENT, event_id TEXT NOT NULL, tenant_id TEXT NOT NULL, kind TEXT NOT NULL, aggregate_id TEXT, aggregate_seq INTEGER, owner_id TEXT, session_id TEXT, event_json TEXT NOT NULL, created INTEGER NOT NULL)", vec![]).await.unwrap();
        db.execute(
            "CREATE INDEX events_aggregate ON events(aggregate_id, aggregate_seq)",
            vec![],
        )
        .await
        .unwrap();
        db.execute("INSERT INTO events VALUES (100,'event','local','message.updated','session',10,NULL,'session','{}',1)", vec![]).await.unwrap();
        db.execute(super::super::event_retention::SCHEMA, vec![])
            .await
            .unwrap();
        db.execute("INSERT INTO event_journal_state VALUES (1,150,20)", vec![])
            .await
            .unwrap();
        recover(&db).await.unwrap();
        assert_eq!(
            db.fetch_scalar_i64(
                "SELECT seq FROM event_sequences WHERE aggregate_id='session'",
                vec![]
            )
            .await
            .unwrap(),
            150
        );
        assert_eq!(
            db.fetch_scalar_i64("SELECT replay_floor FROM event_journal_state", vec![])
                .await
                .unwrap(),
            150
        );
        db.execute("INSERT INTO events(event_id,tenant_id,kind,event_json,created) VALUES ('new','local','message.updated','{}',2)", vec![]).await.unwrap();
        assert!(
            db.fetch_scalar_i64("SELECT MAX(seq) FROM events", vec![])
                .await
                .unwrap()
                > 150
        );
        assert_eq!(db.fetch_scalar_i64("SELECT count(*) FROM sqlite_master WHERE type='table' AND name LIKE 'recovery_events_%'", vec![]).await.unwrap(), 1);
    }

    #[test]
    fn bookkeeping_recovery_does_not_reset_for_operational_errors() {
        for message in [
            "database is locked",
            "database busy",
            "disk full",
            "permission denied",
            "I/O error",
        ] {
            assert!(!corrupt(&anyhow::anyhow!(message)));
        }
    }
}
