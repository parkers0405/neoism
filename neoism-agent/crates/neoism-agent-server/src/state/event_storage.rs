//! Lossless event-journal storage; never deletes events or changes sequence space.
//!
//! BLOB format: eight-byte magic, one-byte codec version (1 = zlib), u64 LE
//! uncompressed byte length, then exactly one zlib stream (including its Adler-32
//! checksum). TEXT remains the original JSON, including legacy oversized rows.
//!
//! The migration is an incremental primary-key seek, NOT a contiguous-sequence
//! walker: wire-only events legitimately leave gaps. All legacy rows must already
//! be durable when the upgraded single writer takes ownership. New writes MUST
//! use encode_event_async, including writes whose preallocated sequence arrives behind
//! the migration cursor. Concurrent older TEXT-only writers are unsupported: a
//! late insert behind the cursor will remain readable but may never be compressed.
//! No startup count/scan, repeated full-history pass, TTL, VACUUM, or checkpoint.
//! Once BLOBs are persisted, an older TEXT-only backend cannot replay this DB.
//! Rolling back the backend requires restoring a pre-upgrade backup (or an
//! explicit lossless offline decompression migration), never deleting history.
use super::{int, text, Db, EventPayload, SqlValue};
use anyhow::Context;
use flate2::{Compression, Decompress, FlushDecompress, Status};
use std::io::Write;
use std::time::Duration;

const MAGIC: &[u8; 8] = b"NEOEVT\0\0";
const VERSION: u8 = 1;
const HEADER: usize = 17;
const MAX_BYTES: usize = 64 * 1024 * 1024;
const MIN_BYTES: usize = 2 * 1024;
const BATCH_ROWS: usize = 128;
const BATCH_BYTES: usize = 2 * 1024 * 1024;
const MIGRATION: &str = "event-payload-deflate-v1";

/// Serialization preserves the complete EventPayload (id, type, sequence and
/// arbitrary properties); storage framing never appears on the wire.
#[cfg(test)]
pub(super) fn encode_event(event: &EventPayload) -> anyhow::Result<SqlValue> {
    encode_json(serde_json::to_string(event)?)
}

/// Large snapshot compression must not occupy an async provider/HTTP worker.
/// Small events avoid a blocking-pool hop; oversized legacy payloads stay TEXT.
pub(super) async fn encode_event_async(event: &EventPayload) -> anyhow::Result<SqlValue> {
    let json = serde_json::to_string(event)?;
    if (256 * 1024..=MAX_BYTES).contains(&json.len()) {
        tokio::task::spawn_blocking(move || encode_json(json))
            .await
            .context("event compression task stopped")?
    } else {
        encode_json(json)
    }
}

// Also used for legacy JSON: do not parse/reserialize and discard unknown fields
// or change formatting. Retain the exact original bytes inside the zlib stream.
fn encode_json(json: String) -> anyhow::Result<SqlValue> {
    Ok(match compressed_blob(&json)? {
        Some(blob) => SqlValue::Blob(blob),
        None => SqlValue::Text(json),
    })
}

// Borrow the exact legacy source so the revision guard needs no full-size clone.
fn compressed_blob(json: &str) -> anyhow::Result<Option<Vec<u8>>> {
    if !(MIN_BYTES..=MAX_BYTES).contains(&json.len()) {
        return Ok(None);
    }
    let mut blob = Vec::with_capacity(HEADER);
    blob.extend_from_slice(MAGIC);
    blob.push(VERSION);
    blob.extend_from_slice(&(json.len() as u64).to_le_bytes());
    // Write directly after the envelope, avoiding a second compressed buffer.
    let mut encoder = flate2::write::ZlibEncoder::new(blob, Compression::fast());
    encoder.write_all(json.as_bytes())?;
    let blob = encoder.finish()?;
    // At least 12.5% savings, including the envelope, without float rounding.
    if blob.len() > json.len() - json.len().div_ceil(8) {
        return Ok(None);
    }
    Ok(Some(blob))
}

/// Cheap replay budget check. BLOB framing/version/declared size are validated;
/// stream integrity is checked by decode_event/decode_json_bytes, not this probe.
pub(super) fn decoded_len(value: &SqlValue) -> anyhow::Result<usize> {
    match value {
        SqlValue::Text(json) => Ok(json.len()),
        SqlValue::Blob(blob) => {
            anyhow::ensure!(blob.len() >= HEADER, "truncated event storage envelope");
            anyhow::ensure!(&blob[..8] == MAGIC, "unknown event storage magic");
            anyhow::ensure!(
                blob[8] == VERSION,
                "unsupported event storage codec version"
            );
            let length = u64::from_le_bytes(blob[9..HEADER].try_into()?);
            anyhow::ensure!(length <= MAX_BYTES as u64, "event payload exceeds 64 MiB");
            anyhow::ensure!(
                blob.len() <= MAX_BYTES + 65536,
                "event storage input too large"
            );
            Ok(length as usize)
        }
        _ => anyhow::bail!("event payload is neither TEXT nor BLOB"),
    }
}

/// Legacy TEXT is unchanged and is not subject to the compressed-input limit.
/// BLOB decoding is bounded before allocation, checks the zlib checksum, rejects
/// trailing/concatenated streams and requires exact input and output lengths.
pub(super) fn decode_json_bytes(value: &SqlValue) -> anyhow::Result<Vec<u8>> {
    let blob = match value {
        SqlValue::Text(json) => return Ok(json.as_bytes().to_vec()),
        SqlValue::Blob(blob) => blob,
        _ => anyhow::bail!("event payload is neither TEXT nor BLOB"),
    };
    let length = decoded_len(value)?;
    let mut output = vec![0; length + 1];
    let mut decoder = Decompress::new(true);
    let status = decoder
        .decompress(&blob[HEADER..], &mut output, FlushDecompress::Finish)
        .context("invalid event zlib stream or checksum")?;
    anyhow::ensure!(
        status == Status::StreamEnd,
        "truncated or oversized event zlib stream"
    );
    anyhow::ensure!(
        decoder.total_out() == length as u64,
        "event length mismatch"
    );
    anyhow::ensure!(
        decoder.total_in() == (blob.len() - HEADER) as u64,
        "trailing event storage bytes"
    );
    output.truncate(length);
    Ok(output)
}

pub(super) fn decode_event(value: &SqlValue) -> anyhow::Result<EventPayload> {
    // Avoid copying TEXT on the common small-event replay path.
    match value {
        SqlValue::Text(json) => Ok(serde_json::from_str(json)?),
        _ => Ok(serde_json::from_slice(&decode_json_bytes(value)?)?),
    }
}

pub(super) struct Compactor {
    cancel: tokio::sync::watch::Sender<bool>,
    task: tokio::sync::Mutex<Option<tokio::task::JoinHandle<()>>>,
}

impl Compactor {
    /// Idempotent/concurrent-safe. Never abort an active transaction or blocking
    /// compression job. Shutdown finishes at most the in-flight bounded batch;
    /// it does not drain history. Maintenance makes only one database busy attempt.
    pub(super) async fn shutdown(&self) {
        let _ = self.cancel.send(true);
        let mut task = self.task.lock().await;
        if let Some(handle) = task.as_mut() {
            if let Err(error) = handle.await {
                tracing::warn!(%error, "event storage compactor stopped unexpectedly");
            }
        }
        *task = None;
    }
}

impl Drop for Compactor {
    fn drop(&mut self) {
        // Detach, don't abort. The task owns only Db, never this handle/store.
        let _ = self.cancel.send(true);
    }
}

pub(super) fn spawn(db: Db) -> Compactor {
    let (cancel, mut receiver) = tokio::sync::watch::channel(false);
    let task = tokio::spawn(async move {
        let mut delay = Duration::from_millis(200);
        loop {
            tokio::select! {
                _ = tokio::time::sleep(delay) => {},
                _ = receiver.changed() => break,
            }
            if *receiver.borrow() {
                break;
            }
            delay = match tick_inner(&db, Some(&receiver)).await {
                Ok(stats) if stats.visited > 0 || stats.writer_busy => {
                    Duration::from_millis(200)
                }
                Ok(_) => Duration::from_secs(30),
                Err(error) => {
                    tracing::warn!(%error, "event storage compaction retrying");
                    Duration::from_secs(30)
                }
            };
        }
    });
    Compactor {
        cancel,
        task: tokio::sync::Mutex::new(Some(task)),
    }
}

#[derive(Debug, Default)]
pub(super) struct TickStats {
    /// Rows visited/advanced, not just rows that were worth compressing.
    pub(super) visited: usize,
    pub(super) compressed: u64,
    pub(super) source_bytes: usize,
    pub(super) writer_busy: bool,
}

struct Replacement {
    seq: i64,
    original: String,
    encoded: SqlValue,
}

struct Batch {
    cursor: i64,
    last: i64,
    visited: usize,
    source_bytes: usize,
    replacements: Vec<Replacement>,
}

fn cancelled(cancel: Option<&tokio::sync::watch::Receiver<bool>>) -> bool {
    cancel.is_some_and(|receiver| *receiver.borrow())
}

/// Seek only INTEGER PRIMARY KEY sequences, then point-read one payload at a
/// time: even SQL typeof/length expressions may materialize whole VM records.
/// Budget admission happens in Rust before retaining a source for compression.
/// Discovering a row that does not fit may read one extra payload, never 128.
/// One oversized row goes alone. Legacy TEXT above 64 MiB must still be loaded
/// by the VM on that point-read, but is dropped unchanged without compression.
async fn prepare(
    db: &Db,
    cancel: Option<&tokio::sync::watch::Receiver<bool>>,
) -> anyhow::Result<Batch> {
    let cursor = db
        .fetch_optional(
            "SELECT cursor_updated FROM data_migrations WHERE name = ?",
            vec![text(MIGRATION)],
        )
        .await?
        .map(|row| row.get_opt_i64("cursor_updated"))
        .transpose()?
        .flatten()
        .unwrap_or(0);
    let sequences = db
        .fetch_all(
            "SELECT seq FROM events WHERE seq > ? ORDER BY seq LIMIT ?",
            vec![int(cursor), int(BATCH_ROWS as i64)],
        )
        .await?;
    let mut batch = Batch {
        cursor,
        last: cursor,
        visited: 0,
        source_bytes: 0,
        replacements: Vec::new(),
    };
    for row in sequences {
        if cancelled(cancel) {
            break;
        }
        let seq = row.get_i64("seq")?;
        let source = db
            .fetch_optional(
                "SELECT event_json FROM events WHERE seq = ?",
                vec![int(seq)],
            )
            .await?;
        let Some(source) = source else {
            // Concurrent canonical deletion: this sequence is no longer work.
            batch.last = seq;
            batch.visited += 1;
            continue;
        };
        // Move the fetched SQL value, not DbRow::get_str's cloned String.
        let value = source
            .values
            .into_iter()
            .next()
            .context("missing event payload")?;
        let size = match &value {
            SqlValue::Text(json) => json.len(),
            SqlValue::Blob(blob) => blob.len(),
            _ => anyhow::bail!("event payload is neither TEXT nor BLOB"),
        };
        if batch.visited > 0 && batch.source_bytes.saturating_add(size) > BATCH_BYTES {
            // value is dropped here; no over-budget source enters the batch.
            break;
        }
        batch.last = seq;
        batch.visited += 1;
        // Stored bytes, even for BLOB/small/oversized rows, bound admission.
        batch.source_bytes = batch.source_bytes.saturating_add(size);
        if let SqlValue::Text(original) = value {
            if (MIN_BYTES..=MAX_BYTES).contains(&size) {
                // Borrow the owned source inside the blocking task. Keep exactly
                // that String for the commit revision guard, with no source clone.
                let replacement = tokio::task::spawn_blocking(
                    move || -> anyhow::Result<Option<Replacement>> {
                        Ok(compressed_blob(&original)?.map(|blob| Replacement {
                            seq,
                            original,
                            encoded: SqlValue::Blob(blob),
                        }))
                    },
                )
                .await??;
                if let Some(replacement) = replacement {
                    batch.replacements.push(replacement);
                }
            }
        }
        if batch.source_bytes >= BATCH_BYTES {
            break;
        }
    }
    Ok(batch)
}

/// Nonblocking writer admission, with a second check at commit after preparation.
/// Useful to deterministic tests/maintenance callers; normal operation uses spawn.
#[cfg(test)]
pub(super) async fn tick(db: &Db) -> anyhow::Result<TickStats> {
    tick_inner(db, None).await
}

async fn tick_inner(
    db: &Db,
    cancel: Option<&tokio::sync::watch::Receiver<bool>>,
) -> anyhow::Result<TickStats> {
    // Avoid spending CPU preparing a batch while a canonical writer is active.
    match db.write_gate.try_lock() {
        Ok(guard) => drop(guard),
        Err(_) => {
            return Ok(TickStats {
                writer_busy: true,
                ..TickStats::default()
            })
        }
    }
    let batch = prepare(db, cancel).await?;
    if cancelled(cancel) {
        return Ok(TickStats::default());
    }
    commit(db, batch).await
}

async fn commit(db: &Db, batch: Batch) -> anyhow::Result<TickStats> {
    let Ok(_writer) = db.write_gate.clone().try_lock_owned() else {
        return Ok(TickStats {
            writer_busy: true,
            ..TickStats::default()
        });
    };
    // No Db::execute[_transaction] under this owned guard: that would deadlock.
    // Even the initial migration row is installed here (no startup writer wait).
    let mut statements = vec![(
        "INSERT OR IGNORE INTO data_migrations(name, state, cursor_updated, rows_done, updated_at) VALUES (?, 'pending', 0, 0, ?)".to_string(),
        vec![text(MIGRATION), int(super::store_i64(crate::now_millis()))],
    )];
    for replacement in batch.replacements {
        statements.push((
            "UPDATE events SET event_json = ? WHERE seq = ? AND event_json = ? AND typeof(event_json) = 'text'".into(),
            vec![replacement.encoded, int(replacement.seq), text(replacement.original)],
        ));
    }
    let replacements = statements.len() - 1;
    statements.push((
        "UPDATE data_migrations SET cursor_updated = ?, rows_done = rows_done + ?, state = ?, updated_at = ?, error = NULL WHERE name = ? AND COALESCE(cursor_updated, 0) = ?".into(),
        vec![int(batch.last), int(batch.visited as i64), text(if batch.visited == 0 { "complete" } else { "running" }), int(super::store_i64(crate::now_millis())), text(MIGRATION), int(batch.cursor)],
    ));
    let result = async {
        let connection = db.database.connect()?;
        // Explicit SQL ensures error paths finish ROLLBACK before returning or
        // retrying. Never depend on a transaction Drop scheduling async cleanup.
        connection.execute("BEGIN IMMEDIATE", ()).await?;
        let work = async {
            let mut compressed = 0;
            for (index, (sql, params)) in statements.iter().enumerate() {
                let changed = connection.execute(sql, params.clone()).await?;
                if index > 0 && index <= replacements {
                    compressed += changed;
                }
                if index == statements.len() - 1 && changed != 1 {
                    // Concurrent tick won the cursor. Roll back all replacements.
                    return Ok(None);
                }
            }
            Ok::<_, turso::Error>(Some(compressed))
        }
        .await;
        match work {
            Ok(Some(compressed)) => match connection.execute("COMMIT", ()).await {
                Ok(_) => Ok(Some(compressed)),
                Err(error) => {
                    connection.execute("ROLLBACK", ()).await?;
                    Err(error)
                }
            },
            Ok(None) => {
                connection.execute("ROLLBACK", ()).await?;
                Ok(None)
            }
            Err(error) => {
                connection.execute("ROLLBACK", ()).await?;
                Err(error)
            }
        }
    }
    .await;
    let result = match result {
        Err(error) if super::turso_error_is_busy(&error) => {
            // Maintenance yields immediately instead of holding the writer gate
            // through the canonical writer's multi-second busy retry policy.
            return Ok(TickStats {
                writer_busy: true,
                ..TickStats::default()
            });
        }
        other => other?,
    };
    Ok(match result {
        Some(compressed) => TickStats {
            visited: batch.visited,
            compressed,
            source_bytes: batch.source_bytes,
            writer_busy: false,
        },
        None => TickStats::default(),
    })
}

#[cfg(test)]
mod tests {
    use super::super::{SessionStore, TenantQueryScope};
    use super::*;
    use serde_json::json;

    fn event(size: usize, sequence: u64) -> EventPayload {
        let mut event = EventPayload::new(
            "message.part.updated",
            json!({
                "sessionID": "ses_codec", "tenantID": "tenant_a",
                "part": {"text": "\u{03bb}\n".repeat(size), "tool": {"nested": [null, true, 42, {"x": "y"}]}},
                "unknownFutureProperty": [1, 2, 3]
            }),
        );
        event.sequence = Some(sequence);
        event
    }

    #[test]
    fn full_event_and_legacy_bytes_are_lossless() {
        for sequence in [0, 42, u64::MAX] {
            let event = event(4000, sequence);
            let encoded = encode_event(&event).unwrap();
            assert!(matches!(encoded, SqlValue::Blob(_)));
            assert_eq!(
                decoded_len(&encoded).unwrap(),
                serde_json::to_vec(&event).unwrap().len()
            );
            assert_eq!(
                serde_json::to_value(decode_event(&encoded).unwrap()).unwrap(),
                serde_json::to_value(&event).unwrap()
            );
        }
        let mut small = EventPayload::new("tiny", json!({"a": null}));
        small.sequence = None;
        let encoded = encode_event(&small).unwrap();
        assert!(matches!(encoded, SqlValue::Text(_)));
        assert_eq!(
            serde_json::to_value(decode_event(&encoded).unwrap()).unwrap(),
            serde_json::to_value(small).unwrap()
        );
        let original = format!("  {{\n  \"unknown\": \"{}\"\n}}  ", "abcd".repeat(3000));
        let encoded = encode_json(original.clone()).unwrap();
        assert_eq!(decode_json_bytes(&encoded).unwrap(), original.as_bytes());
        let text = SqlValue::Text(original.clone());
        assert_eq!(decoded_len(&text).unwrap(), original.len());
        assert_eq!(decode_json_bytes(&text).unwrap(), original.as_bytes());
    }

    #[tokio::test(flavor = "current_thread")]
    async fn large_async_encoding_matches_sync_and_services_timer() {
        let event = event(2 * 1024 * 1024, 123);
        let expected = encode_event(&event).unwrap();
        // Exercise a timer on the same single-thread executor as the async
        // encoder, with a generous watchdog rather than timing-sensitive latency
        // assertions that become flaky under a saturated CI blocking pool.
        let (encoded, ()) = tokio::time::timeout(Duration::from_secs(30), async {
            tokio::join!(encode_event_async(&event), async {
                tokio::time::sleep(Duration::from_millis(1)).await;
            })
        })
        .await
        .unwrap();
        let encoded = encoded.unwrap();
        let (SqlValue::Blob(expected), SqlValue::Blob(actual)) = (expected, &encoded)
        else {
            panic!("large compressible event should be a BLOB");
        };
        assert_eq!(&expected, actual);
        assert_eq!(
            serde_json::to_value(decode_event(&encoded).unwrap()).unwrap(),
            serde_json::to_value(event).unwrap()
        );
    }

    #[test]
    fn threshold_savings_and_incompressible_input() {
        assert!(matches!(
            encode_json("a".repeat(MIN_BYTES - 1)).unwrap(),
            SqlValue::Text(_)
        ));
        assert!(matches!(
            encode_json("a".repeat(MIN_BYTES)).unwrap(),
            SqlValue::Blob(_)
        ));
        // Deterministic high-entropy valid UTF-8, not a repeated fixture string.
        let mut seed = 0x123456789abcdefu64;
        let input: String = (0..MIN_BYTES)
            .map(|_| {
                seed ^= seed << 13;
                seed ^= seed >> 7;
                seed ^= seed << 17;
                char::from((seed & 0x7f) as u8)
            })
            .collect();
        assert!(matches!(encode_json(input).unwrap(), SqlValue::Text(_)));
        let input = "x".repeat(MAX_BYTES + 1);
        let encoded = encode_json(input).unwrap();
        assert!(matches!(encoded, SqlValue::Text(_)));
        assert_eq!(decoded_len(&encoded).unwrap(), MAX_BYTES + 1);
    }

    #[test]
    fn rejects_corruption_truncation_unknown_versions_lengths_and_bombs() {
        let SqlValue::Blob(valid) = encode_event(&event(4000, 1)).unwrap() else {
            panic!()
        };
        for length in [0, 7, HEADER - 1, HEADER, valid.len() - 1] {
            assert!(decode_json_bytes(&SqlValue::Blob(valid[..length].to_vec())).is_err());
        }
        for offset in [0, 8, 9, valid.len() - 1] {
            let mut corrupt = valid.clone();
            corrupt[offset] ^= 0x7f;
            assert!(decode_event(&SqlValue::Blob(corrupt)).is_err());
        }
        let mut trailing = valid.clone();
        trailing.extend_from_slice(&[0]);
        assert!(decode_event(&SqlValue::Blob(trailing)).is_err());
        let mut oversized = valid.clone();
        oversized[9..HEADER].copy_from_slice(&((MAX_BYTES as u64) + 1).to_le_bytes());
        assert!(decoded_len(&SqlValue::Blob(oversized.clone())).is_err());
        assert!(decode_event(&SqlValue::Blob(oversized)).is_err());
        // Valid zlib, maliciously understated header: cap output at length + 1.
        let mut bomb = valid;
        bomb[9..HEADER].copy_from_slice(&1u64.to_le_bytes());
        assert!(decode_json_bytes(&SqlValue::Blob(bomb)).is_err());
        assert!(decode_event(&SqlValue::Null).is_err());
        assert!(decoded_len(&SqlValue::Integer(1)).is_err());
    }

    // Tests create unique, isolated SessionStore databases, never the user's DB.
    // Parent integration supplies the event worker field alongside SearchWorker.
    async fn store_at(path: std::path::PathBuf) -> SessionStore {
        let store = SessionStore::open(path).await.unwrap();
        if let Some(worker) = &store._search_worker {
            worker.shutdown().await;
        }
        if let Some(worker) = &store._event_compactor {
            worker.shutdown().await;
        }
        store
    }

    async fn store() -> (SessionStore, std::path::PathBuf) {
        let path = std::env::temp_dir().join(format!(
            "neoism-event-storage-{}.db",
            neoism_agent_core::new_session_id()
        ));
        (store_at(path.clone()).await, path)
    }

    async fn insert(db: &Db, seq: i64, source: String, tenant: &str, session: &str) {
        db.execute("INSERT INTO events(seq, event_id, tenant_id, kind, aggregate_id, aggregate_seq, session_id, event_json, created) VALUES (?, ?, ?, 'message.part.updated', ?, ?, ?, ?, 123)",
            vec![int(seq), text(format!("evt_test_{seq}")), text(tenant), text(session), int(seq * 2), text(session), text(source)]).await.unwrap();
    }

    async fn cursor(db: &Db) -> i64 {
        db.fetch_scalar_i64(
            "SELECT COALESCE(cursor_updated, 0) FROM data_migrations WHERE name = ?",
            vec![text(MIGRATION)],
        )
        .await
        .unwrap()
    }

    #[tokio::test]
    async fn row_batches_gaps_reopen_and_readable_late_encoded_insert() {
        let (store, path) = store().await;
        let json = serde_json::to_string(&event(0, 1)).unwrap();
        for i in 1..=130 {
            insert(&store.db, i * 3, json.clone(), "tenant_a", "ses_codec").await;
        }
        let first = tick(&store.db).await.unwrap();
        assert_eq!(first.visited, BATCH_ROWS);
        assert_eq!(cursor(&store.db).await, 384);
        drop(store);
        let store = store_at(path).await;
        assert_eq!(tick(&store.db).await.unwrap().visited, 2);
        assert_eq!(cursor(&store.db).await, 390);
        assert_eq!(tick(&store.db).await.unwrap().visited, 0);
        assert_eq!(
            store
                .db
                .fetch_scalar_i64(
                    "SELECT rows_done FROM data_migrations WHERE name = ?",
                    vec![text(MIGRATION)]
                )
                .await
                .unwrap(),
            130
        );
        // The upgraded encoder makes an asynchronous insertion behind the cursor
        // safe without requiring wire sequences to be contiguous or rescanning.
        let late = event(4000, 2);
        insert(
            &store.db,
            2,
            serde_json::to_string(&late).unwrap(),
            "tenant_a",
            "ses_codec",
        )
        .await;
        store
            .db
            .execute(
                "UPDATE events SET event_json = ? WHERE seq = 2",
                vec![encode_event(&late).unwrap()],
            )
            .await
            .unwrap();
        assert_eq!(tick(&store.db).await.unwrap().visited, 0);
        let replay = store
            .list_events_after(
                TenantQueryScope::Tenant("tenant_a"),
                0,
                10,
                Some("ses_codec"),
            )
            .await
            .unwrap();
        assert_eq!(replay[0].seq, 2);
        assert_eq!(replay[0].payload.sequence, Some(2));
    }

    #[tokio::test]
    async fn byte_budget_oversized_alone_and_physical_savings() {
        let (store, _) = store().await;
        let source = serde_json::to_string(&event(BATCH_BYTES / 2, 10)).unwrap();
        assert!(source.len() > BATCH_BYTES);
        for seq in 1..=3 {
            insert(&store.db, seq, source.clone(), "tenant_a", "ses_codec").await;
        }
        let before = store
            .db
            .fetch_scalar_i64(
                "SELECT sum(length(CAST(event_json AS BLOB))) FROM events",
                vec![],
            )
            .await
            .unwrap();
        for _ in 0..3 {
            let stats = tick(&store.db).await.unwrap();
            assert_eq!(stats.visited, 1);
            assert_eq!(stats.compressed, 1);
            assert_eq!(stats.source_bytes, source.len());
        }
        let after = store
            .db
            .fetch_scalar_i64(
                "SELECT sum(length(CAST(event_json AS BLOB))) FROM events",
                vec![],
            )
            .await
            .unwrap();
        eprintln!(
            "event payload stored bytes: before={before}, after={after}, saved={} ({:.2}%)",
            before - after,
            100.0 * (before - after) as f64 / before as f64,
        );
        assert!(
            after < before / 8,
            "stored payload bytes, not DB-file VACUUM size"
        );
        assert_eq!(cursor(&store.db).await, 3);
        assert_eq!(tick(&store.db).await.unwrap().visited, 0);
    }

    #[tokio::test]
    async fn byte_budget_keeps_exact_sequence_prefix() {
        let (store, _) = store().await;
        let source = serde_json::to_string(&event(200_000, 3)).unwrap();
        assert!(source.len() * 2 <= BATCH_BYTES);
        assert!(source.len() * 3 > BATCH_BYTES);
        for seq in [3, 7, 12] {
            insert(&store.db, seq, source.clone(), "tenant_a", "ses_codec").await;
        }
        let batch = prepare(&store.db, None).await.unwrap();
        assert_eq!((batch.last, batch.visited), (7, 2));
        assert_eq!(batch.source_bytes, source.len() * 2);
        assert_eq!(
            batch
                .replacements
                .iter()
                .map(|row| row.seq)
                .collect::<Vec<_>>(),
            vec![3, 7]
        );
        assert_eq!(commit(&store.db, batch).await.unwrap().compressed, 2);
        assert_eq!(cursor(&store.db).await, 7);
        let batch = prepare(&store.db, None).await.unwrap();
        assert_eq!((batch.last, batch.visited), (12, 1));
        assert_eq!(batch.source_bytes, source.len());
        assert_eq!(commit(&store.db, batch).await.unwrap().compressed, 1);
        assert_eq!(cursor(&store.db).await, 12);
    }

    #[tokio::test]
    async fn replacement_and_cursor_revision_guards_and_nonblocking_admission() {
        let (store, _) = store().await;
        let source = serde_json::to_string(&event(4000, 7)).unwrap();
        insert(&store.db, 7, source, "tenant_a", "ses_codec").await;
        let batch = prepare(&store.db, None).await.unwrap();
        store
            .db
            .execute(
                "UPDATE events SET event_json = ? WHERE seq = 7",
                vec![text("replacement")],
            )
            .await
            .unwrap();
        assert_eq!(commit(&store.db, batch).await.unwrap().compressed, 0);
        let row = store
            .db
            .fetch_optional("SELECT event_json FROM events WHERE seq = 7", vec![])
            .await
            .unwrap()
            .unwrap();
        assert_eq!(row.get_str("event_json").unwrap(), "replacement");
        let writer = store.db.lock_writer().await;
        let stats = tokio::time::timeout(Duration::from_millis(100), tick(&store.db))
            .await
            .unwrap()
            .unwrap();
        assert!(stats.writer_busy);
        drop(writer);
        insert(
            &store.db,
            9,
            serde_json::to_string(&event(4000, 9)).unwrap(),
            "tenant_a",
            "ses_codec",
        )
        .await;
        let stale = prepare(&store.db, None).await.unwrap();
        let winner = prepare(&store.db, None).await.unwrap();
        assert_eq!(commit(&store.db, winner).await.unwrap().visited, 1);
        assert_eq!(commit(&store.db, stale).await.unwrap().visited, 0);
        assert_eq!(
            store
                .db
                .fetch_scalar_i64(
                    "SELECT rows_done FROM data_migrations WHERE name = ?",
                    vec![text(MIGRATION)]
                )
                .await
                .unwrap(),
            2
        );
    }

    #[tokio::test]
    async fn transaction_failure_rolls_back_payload_and_cursor() {
        let (store, _) = store().await;
        let source = serde_json::to_string(&event(4000, 1)).unwrap();
        insert(&store.db, 1, source.clone(), "tenant_a", "ses_codec").await;
        let batch = prepare(&store.db, None).await.unwrap();
        // The isolated test DB deliberately lacks a column used ONLY by the
        // final cursor UPDATE, after payload replacement has already succeeded.
        store
            .db
            .execute(
                "ALTER TABLE data_migrations RENAME COLUMN error TO broken_error",
                vec![],
            )
            .await
            .unwrap();
        assert!(commit(&store.db, batch).await.is_err());
        let row = store
            .db
            .fetch_optional("SELECT event_json FROM events WHERE seq = 1", vec![])
            .await
            .unwrap()
            .unwrap();
        assert_eq!(row.get_str("event_json").unwrap(), source);
        // Metadata might have been initialized by the briefly running worker;
        // either way no event sequence may have advanced on failed commit.
        if let Some(row) = store
            .db
            .fetch_optional(
                "SELECT cursor_updated FROM data_migrations WHERE name = ?",
                vec![text(MIGRATION)],
            )
            .await
            .unwrap()
        {
            assert_eq!(row.get_i64("cursor_updated").unwrap(), 0);
        }
        store
            .db
            .execute(
                "ALTER TABLE data_migrations RENAME COLUMN broken_error TO error",
                vec![],
            )
            .await
            .unwrap();
        assert_eq!(tick(&store.db).await.unwrap().compressed, 1);
    }

    #[tokio::test]
    async fn production_replay_tenant_session_and_sequences_are_unchanged() {
        let (store, _) = store().await;
        for (seq, tenant, session) in [
            (10, "tenant_a", "ses_codec"),
            (20, "tenant_b", "ses_other"),
            (40, "tenant_a", "ses_child"),
        ] {
            insert(
                &store.db,
                seq,
                serde_json::to_string(&event(4000, seq as u64)).unwrap(),
                tenant,
                session,
            )
            .await;
        }
        let before = store
            .list_events_after(TenantQueryScope::LocalAll, 0, 128, None)
            .await
            .unwrap();
        let metadata = "SELECT seq, aggregate_seq, aggregate_id, event_id, tenant_id, session_id, created FROM events ORDER BY seq";
        let original = store.db.fetch_all(metadata, vec![]).await.unwrap();
        assert_eq!(tick(&store.db).await.unwrap().compressed, 3);
        let after = store
            .list_events_after(TenantQueryScope::LocalAll, 0, 128, None)
            .await
            .unwrap();
        for (before, after) in before.iter().zip(&after) {
            assert_eq!((before.seq, before.created), (after.seq, after.created));
            assert_eq!(
                serde_json::to_value(&before.payload).unwrap(),
                serde_json::to_value(&after.payload).unwrap()
            );
        }
        let rows = store.db.fetch_all(metadata, vec![]).await.unwrap();
        for (original, row) in original.iter().zip(rows) {
            assert_eq!(
                format!("{:?}", original.values),
                format!("{:?}", row.values)
            );
        }
        assert_eq!(after.len(), 3);
        assert_eq!(store.latest_event_sequence().await.unwrap(), 40);
        let scoped = store
            .list_events_after(TenantQueryScope::Tenant("tenant_a"), 10, 128, None)
            .await
            .unwrap();
        assert_eq!(
            scoped.iter().map(|row| row.seq).collect::<Vec<_>>(),
            vec![40]
        );
        let scoped = store
            .list_events_after(
                TenantQueryScope::Tenant("tenant_b"),
                0,
                128,
                Some("ses_codec"),
            )
            .await
            .unwrap();
        assert!(scoped.is_empty());
        let scoped = store
            .list_events_after(TenantQueryScope::LocalAll, 0, 128, Some("ses_child"))
            .await
            .unwrap();
        assert_eq!(
            scoped.iter().map(|row| row.seq).collect::<Vec<_>>(),
            vec![40]
        );
    }

    #[tokio::test]
    async fn replay_budgets_stored_bytes_before_trusting_declared_length() {
        let (store, _) = store().await;
        let source = serde_json::to_string(&event(0, 1)).unwrap();
        insert(&store.db, 1, source.clone(), "tenant_a", "ses_codec").await;
        insert(&store.db, 2, source, "tenant_a", "ses_codec").await;
        // A corrupt envelope can lie about decoded size. It must not be retained
        // alongside a full page merely because its declared output is tiny.
        let mut blob = Vec::from(MAGIC.as_slice());
        blob.push(VERSION);
        blob.extend_from_slice(&1u64.to_le_bytes());
        blob.resize(8 * 1024 * 1024, 0);
        store.db.execute(
            "UPDATE events SET event_json = ? WHERE seq = 2",
            vec![SqlValue::Blob(blob)],
        ).await.unwrap();
        let page = store.list_events_after(TenantQueryScope::LocalAll, 0, 128, None)
            .await.unwrap();
        assert_eq!(page.iter().map(|event| event.seq).collect::<Vec<_>>(), vec![1]);
        // The next cursor still encounters corruption, rather than losing it.
        assert!(store.list_events_after(TenantQueryScope::LocalAll, 1, 128, None)
            .await.is_err());
    }

    #[tokio::test]
    async fn background_shutdown_drop_and_cancel_before_commit() {
        let (store, _) = store().await;
        insert(
            &store.db,
            1,
            serde_json::to_string(&event(4000, 1)).unwrap(),
            "tenant_a",
            "ses_codec",
        )
        .await;
        let (cancel, receiver) = tokio::sync::watch::channel(true);
        assert_eq!(
            tick_inner(&store.db, Some(&receiver))
                .await
                .unwrap()
                .visited,
            0
        );
        drop(cancel);
        let worker = spawn(store.db.clone());
        tokio::time::timeout(Duration::from_secs(5), async {
            tokio::join!(worker.shutdown(), worker.shutdown());
        })
        .await
        .unwrap();
        let worker = spawn(store.db.clone());
        let task = worker.task.lock().await.take().unwrap();
        drop(worker);
        tokio::time::timeout(Duration::from_secs(5), task)
            .await
            .unwrap()
            .unwrap();
        // Active foreground writer does not keep shutdown queued behind its lock.
        let writer = store.db.lock_writer().await;
        let worker = spawn(store.db.clone());
        tokio::time::sleep(Duration::from_millis(250)).await;
        tokio::time::timeout(Duration::from_secs(1), worker.shutdown())
            .await
            .unwrap();
        drop(writer);
    }
}
