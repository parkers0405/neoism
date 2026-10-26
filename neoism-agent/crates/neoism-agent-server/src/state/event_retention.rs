//! Time-bounded retention for the durable event journal.
//!
//! The journal exists only for SSE catch-up (`since` / `Last-Event-ID`).
//! Canonical session, message and part rows are the source of truth, so
//! journal rows older than the retention window are deleted in bounded
//! batches. (OpenCode V2 does not persist raw events at all by default.)
//!
//! Two durable values in `event_journal_state` keep pruning invisible to
//! clients:
//! - `high_water`: the largest sequence ever persisted. The wire counter seeds
//!   from `max(MAX(events.seq), high_water)`, so it never moves backwards even
//!   when every row has been pruned.
//! - `replay_floor`: every sequence `<= replay_floor` may be gone. A cursor
//!   below the floor is answered with an explicit `events.replay.expired`
//!   event instead of a silently incomplete replay.
//!
//! Each batch advances both values in the SAME transaction as its DELETE.
//! Like the payload compactor, the worker only takes the writer gate with
//! `try_lock` and yields on busy, so live writes are never delayed.
use super::{int, Db, SqlValue};
use std::time::Duration;

const DEFAULT_RETENTION_DAYS: u64 = 7;
const RETENTION_ENV: &str = "NEOISM_AGENT_EVENT_RETENTION_DAYS";
const BATCH_ROWS: i64 = 500;
const DAY_MS: i64 = 24 * 60 * 60 * 1000;

pub(super) const SCHEMA: &str = r#"
    CREATE TABLE IF NOT EXISTS event_journal_state (
        id INTEGER PRIMARY KEY CHECK (id = 1),
        high_water INTEGER NOT NULL DEFAULT 0,
        replay_floor INTEGER NOT NULL DEFAULT 0
    )
"#;

/// Retention window, or `None` when pruning is disabled (`=0`).
pub(crate) fn retention_window() -> Option<Duration> {
    let days = std::env::var(RETENTION_ENV)
        .ok()
        .and_then(|value| value.trim().parse::<u64>().ok())
        .unwrap_or(DEFAULT_RETENTION_DAYS);
    (days > 0).then(|| Duration::from_secs(days * 24 * 60 * 60))
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub(crate) struct JournalState {
    pub(crate) high_water: i64,
    pub(crate) replay_floor: i64,
}

pub(super) async fn journal_state(db: &Db) -> anyhow::Result<JournalState> {
    let row = db
        .fetch_optional(
            "SELECT high_water, replay_floor FROM event_journal_state WHERE id = 1",
            vec![],
        )
        .await?;
    Ok(match row {
        Some(row) => JournalState {
            high_water: row.get_i64("high_water")?,
            replay_floor: row.get_i64("replay_floor")?,
        },
        None => JournalState::default(),
    })
}

#[derive(Debug, Default, Eq, PartialEq)]
pub(crate) struct PruneStats {
    pub(crate) deleted: u64,
    /// More expired rows remain after this batch.
    pub(crate) more: bool,
    pub(crate) writer_busy: bool,
}

/// Delete one bounded batch of journal rows created before `cutoff_ms`.
pub(super) async fn prune_batch(db: &Db, cutoff_ms: i64) -> anyhow::Result<PruneStats> {
    if db.write_gate.try_lock().is_err() {
        return Ok(PruneStats { writer_busy: true, ..PruneStats::default() });
    }
    let floor = journal_state(db).await?.replay_floor;
    // Inspect timestamps in sequence order: wall-clock changes can make them
    // nonmonotonic. Only an entirely expired prefix may advance the replay floor.
    let rows = db
        .fetch_all(
            "SELECT seq, created FROM events WHERE seq > ? ORDER BY seq LIMIT ?",
            vec![int(floor), int(BATCH_ROWS)],
        )
        .await?;
    let mut end = None;
    let mut expired = 0;
    for row in &rows {
        if row.get_i64("created")? >= cutoff_ms {
            break;
        }
        end = Some(row.get_i64("seq")?);
        expired += 1;
    }
    let Some(end) = end else {
        return Ok(PruneStats::default());
    };
    let full_batch = expired == BATCH_ROWS;
    let Ok(_writer) = db.write_gate.clone().try_lock_owned() else {
        return Ok(PruneStats { writer_busy: true, ..PruneStats::default() });
    };
    // No Db::execute[_transaction] under this owned guard: that would deadlock.
    let result = async {
        let connection = db.database.connect()?;
        connection.execute("BEGIN IMMEDIATE", ()).await?;
        let work = async {
            connection
                .execute(
                    "INSERT OR IGNORE INTO event_journal_state (id, high_water, replay_floor) VALUES (1, 0, 0)",
                    (),
                )
                .await?;
            // Record the high-water mark BEFORE rows can disappear.
            connection
                .execute(
                    "UPDATE event_journal_state SET high_water = max(high_water, COALESCE((SELECT seq FROM events ORDER BY seq DESC LIMIT 1), 0)), replay_floor = max(replay_floor, ?) WHERE id = 1",
                    vec![SqlValue::Integer(end)],
                )
                .await?;
            connection
                .execute(
                    "DELETE FROM events WHERE seq > ? AND seq <= ?",
                    vec![SqlValue::Integer(floor), SqlValue::Integer(end)],
                )
                .await
        }
        .await;
        match work {
            Ok(deleted) => match connection.execute("COMMIT", ()).await {
                Ok(_) => Ok(deleted),
                Err(error) => {
                    connection.execute("ROLLBACK", ()).await?;
                    Err(error)
                }
            },
            Err(error) => {
                connection.execute("ROLLBACK", ()).await?;
                Err(error)
            }
        }
    }
    .await;
    match result {
        Err(error) if super::turso_error_is_busy(&error) => {
            Ok(PruneStats { writer_busy: true, ..PruneStats::default() })
        }
        Err(error) => Err(error.into()),
        Ok(deleted) => Ok(PruneStats { deleted, more: full_batch, writer_busy: false }),
    }
}

/// Prune until no expired rows remain (offline maintenance and tests).
pub(super) async fn prune_all(db: &Db, cutoff_ms: i64) -> anyhow::Result<u64> {
    let mut deleted = 0;
    loop {
        let stats = prune_batch(db, cutoff_ms).await?;
        deleted += stats.deleted;
        if stats.writer_busy {
            tokio::time::sleep(Duration::from_millis(50)).await;
            continue;
        }
        if !stats.more {
            return Ok(deleted);
        }
    }
}

pub(super) fn cutoff_ms(window: Duration) -> i64 {
    super::store_i64(crate::now_millis()).saturating_sub(window.as_millis().min(i64::MAX as u128) as i64)
}

pub(super) struct RetentionWorker {
    cancel: tokio::sync::watch::Sender<bool>,
    task: tokio::sync::Mutex<Option<tokio::task::JoinHandle<()>>>,
}

impl RetentionWorker {
    /// Finishes at most the in-flight bounded batch.
    pub(super) async fn shutdown(&self) {
        let _ = self.cancel.send(true);
        let mut task = self.task.lock().await;
        if let Some(handle) = task.as_mut() {
            if let Err(error) = handle.await {
                tracing::warn!(%error, "event retention worker stopped unexpectedly");
            }
        }
        *task = None;
    }
}

impl Drop for RetentionWorker {
    fn drop(&mut self) {
        let _ = self.cancel.send(true);
    }
}

pub(super) fn spawn(db: Db) -> Option<RetentionWorker> {
    let window = retention_window()?;
    let (cancel, mut receiver) = tokio::sync::watch::channel(false);
    let task = tokio::spawn(async move {
        // Let startup and the first conversations settle before maintenance.
        let mut delay = Duration::from_secs(30);
        let mut pass_deleted = 0u64;
        loop {
            tokio::select! {
                _ = tokio::time::sleep(delay) => {},
                _ = receiver.changed() => break,
            }
            if *receiver.borrow() {
                break;
            }
            delay = match prune_batch(&db, cutoff_ms(window)).await {
                Ok(stats) if stats.writer_busy => Duration::from_millis(250),
                Ok(stats) if stats.more => {
                    pass_deleted += stats.deleted;
                    Duration::from_millis(25)
                }
                Ok(stats) => {
                    pass_deleted += stats.deleted;
                    if pass_deleted > 0 {
                        tracing::info!(deleted = pass_deleted, "event journal retention pass complete");
                    }
                    pass_deleted = 0;
                    Duration::from_secs(60 * 60)
                }
                Err(error) => {
                    tracing::warn!(%error, "event journal retention retrying");
                    Duration::from_secs(5 * 60)
                }
            };
        }
    });
    Some(RetentionWorker { cancel, task: tokio::sync::Mutex::new(Some(task)) })
}

#[cfg(test)]
mod tests {
    use super::super::{SessionStore, TenantQueryScope};
    use super::*;

    async fn store() -> (SessionStore, std::path::PathBuf) {
        let path = std::env::temp_dir().join(format!(
            "neoism-event-retention-{}.db",
            neoism_agent_core::new_session_id()
        ));
        let store = SessionStore::open(path.clone()).await.unwrap();
        if let Some(worker) = &store._search_worker {
            worker.shutdown().await;
        }
        if let Some(worker) = &store._event_compactor {
            worker.shutdown().await;
        }
        if let Some(worker) = &store._event_retention {
            worker.shutdown().await;
        }
        (store, path)
    }

    async fn insert(db: &Db, seq: i64, created: i64) {
        db.execute(
            "INSERT INTO events (seq, event_id, tenant_id, kind, aggregate_id, aggregate_seq, owner_id, session_id, event_json, created) VALUES (?, ?, 'local', 'session.updated', 'agg', ?, NULL, 'ses_1', ?, ?)",
            vec![
                int(seq),
                SqlValue::Text(format!("evt_{seq}")),
                int(seq),
                SqlValue::Text(format!(
                    r#"{{"id":"evt_{seq}","type":"session.updated","properties":{{"sessionID":"ses_1"}}}}"#
                )),
                int(created),
            ],
        )
        .await
        .unwrap();
    }

    #[tokio::test]
    async fn prunes_only_expired_rows_and_advances_floor_atomically() {
        let (store, _path) = store().await;
        for seq in 1..=1_200 {
            insert(&store.db, seq, if seq <= 1_000 { 10 } else { 1_000 }).await;
        }
        let deleted = prune_all(&store.db, 500).await.unwrap();
        assert_eq!(deleted, 1_000);
        let state = journal_state(&store.db).await.unwrap();
        assert_eq!(state.replay_floor, 1_000);
        assert_eq!(state.high_water, 1_200);
        let remaining = store
            .list_events_after(TenantQueryScope::LocalAll, 0, 128, None)
            .await
            .unwrap();
        assert_eq!(remaining.first().map(|event| event.seq), Some(1_001));
        // Idempotent: nothing further is expired.
        assert_eq!(prune_all(&store.db, 500).await.unwrap(), 0);
    }

    #[tokio::test]
    async fn clock_rollback_does_not_prune_unexpired_rows() {
        let (store, _path) = store().await;
        insert(&store.db, 1, 10).await;
        insert(&store.db, 2, 1_000).await;
        insert(&store.db, 3, 10).await;
        assert_eq!(prune_all(&store.db, 500).await.unwrap(), 1);
        assert_eq!(store.event_replay_floor().await.unwrap(), 1);
        let rows = store.db.fetch_all("SELECT seq FROM events ORDER BY seq", vec![]).await.unwrap();
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0].get_i64("seq").unwrap(), 2);
        assert_eq!(rows[1].get_i64("seq").unwrap(), 3);
    }

    #[tokio::test]
    async fn sequence_seed_never_moves_backwards_after_full_prune() {
        let (store, _path) = store().await;
        for seq in 1..=40 {
            insert(&store.db, seq, 10).await;
        }
        assert_eq!(prune_all(&store.db, 500).await.unwrap(), 40);
        assert_eq!(store.latest_event_sequence().await.unwrap(), 40);
        assert_eq!(store.event_replay_floor().await.unwrap(), 40);
    }

    #[tokio::test]
    async fn yields_while_a_writer_holds_the_gate() {
        let (store, _path) = store().await;
        insert(&store.db, 1, 10).await;
        let _writer = store.db.write_gate.clone().lock_owned().await;
        let stats = prune_batch(&store.db, 500).await.unwrap();
        assert!(stats.writer_busy);
        assert_eq!(stats.deleted, 0);
    }
}
