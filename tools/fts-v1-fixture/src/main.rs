use std::{error::Error, io::Write, path::PathBuf};

include!("../fixture_data.rs");

#[tokio::main]
async fn main() -> Result<(), Box<dyn Error>> {
    let path = PathBuf::from(
        std::env::args_os()
            .nth(1)
            .ok_or("usage: neoism-fts-v1-fixture NEW_DATABASE_PATH")?,
    );
    // Refuse to open any existing database, especially a developer's live store.
    drop(
        std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&path)?,
    );
    let database = turso::Builder::new_local(path.to_str().ok_or("non-UTF8 path")?)
        .experimental_index_method(true)
        .build()
        .await?;
    let connection = database.connect()?;
    for sql in include_str!("../fixture.sql")
        .split(';')
        .map(str::trim)
        .filter(|sql| !sql.is_empty())
    {
        connection.execute(sql, ()).await?;
    }
    // Twelve independent batches exercise multi-page old native backing, not
    // just a two-row in-memory-sized index. Each projection has canonical JSON.
    for batch in 0..PAGE_DOCUMENTS / 32 {
        connection.execute("BEGIN IMMEDIATE", ()).await?;
        for index in batch * 32..(batch + 1) * 32 {
            let id = format!("msg_page_{index:04}");
            let body = page_body(index);
            let message = serde_json::json!({
                "info": {"role":"user", "id":id, "sessionId":"ses_fixture", "time":{"created":index + 10}, "agent":"build", "model":{"providerId":"test", "modelId":"test"}},
                "parts":[{"type":"text", "id":format!("prt_page_{index:04}"), "sessionId":"ses_fixture", "messageId":id, "text":body}],
                "opaque":{"index":index,"preserve":"  bytes 雪  "}
            }).to_string();
            connection
                .execute(
                    "INSERT INTO messages VALUES (?, 'local', 'ses_fixture', ?, ?, ?)",
                    (
                        id.as_str(),
                        message.as_str(),
                        index as i64 + 10,
                        index as i64 + 1,
                    ),
                )
                .await?;
            connection.execute("INSERT INTO conversation_search_docs VALUES (?, 'ses_fixture', ?, 'user', ?, ?, ?)",
                (format!("message:{id}"), id, index as i64 + 10, format!("Page {index:04}"), body)).await?;
        }
        connection.execute("COMMIT", ()).await?;
    }
    for compressed in [false, true] {
        let json = event_json(compressed);
        let value = if compressed {
            // Exact production event_storage framing: 8-byte magic, version 1,
            // LE u64 uncompressed byte length, then one complete zlib stream.
            let mut blob = b"NEOEVT\0\0".to_vec();
            blob.push(1);
            blob.extend_from_slice(&(json.len() as u64).to_le_bytes());
            let mut encoder =
                flate2::write::ZlibEncoder::new(blob, flate2::Compression::fast());
            encoder.write_all(json.as_bytes())?;
            turso::Value::Blob(encoder.finish()?)
        } else {
            turso::Value::Text(json)
        };
        let seq = if compressed { 202 } else { 101 };
        let id = if compressed {
            "evt_fixture_blob"
        } else {
            "evt_fixture_text"
        };
        connection.execute("INSERT INTO events(seq,event_id,tenant_id,kind,aggregate_id,aggregate_seq,owner_id,session_id,event_json,created) VALUES (?,?,'local','message.part.updated','ses_fixture',?,'fixture-owner','ses_fixture',?,12345)",
            vec![turso::Value::Integer(seq), turso::Value::Text(id.into()), turso::Value::Integer(seq * 2), value]).await?;
    }
    let mut rows = connection.query("PRAGMA page_size", ()).await?;
    let page_size = rows
        .next()
        .await?
        .ok_or("missing page size")?
        .get::<i64>(0)?;
    drop(rows);
    let mut rows = connection.query("SELECT sum(length(bytes)) FROM __turso_internal_fts_dir_conversation_search_fts", ()).await?;
    let backing_bytes = rows
        .next()
        .await?
        .ok_or("missing FTS backing")?
        .get::<i64>(0)?;
    drop(rows);
    assert!(
        backing_bytes > 4 * page_size,
        "old FTS backing did not cross pages"
    );
    connection
        .execute(
            "INSERT INTO fixture_metrics VALUES ('old_fts_backing_bytes', ?)",
            [backing_bytes],
        )
        .await?;
    connection
        .execute(
            "INSERT INTO fixture_metrics VALUES ('page_size', ?)",
            [page_size],
        )
        .await?;

    // Confirm that the old engine really built a searchable native index.
    let query = "SELECT key, fts_score(title, body, 'legacyquartz') AS score FROM conversation_search_docs WHERE fts_match(title, body, 'legacyquartz') ORDER BY score DESC LIMIT 10";
    let mut plan = connection
        .query(&format!("EXPLAIN QUERY PLAN {query}"), ())
        .await?;
    let mut native = false;
    while let Some(row) = plan.next().await? {
        native |= row.get::<String>(3)?.contains("QUERY INDEX METHOD fts");
    }
    assert!(
        native,
        "fixture producer used scalar fallback instead of FTS"
    );
    drop(plan);
    let mut rows = connection.query(query, ()).await?;
    assert_eq!(
        rows.next()
            .await?
            .ok_or("missing legacy match")?
            .get::<String>(0)?,
        "message:msg_fixture"
    );
    assert!(rows.next().await?.is_none());
    drop(rows);
    let mut rows = connection.query("SELECT key, fts_score(title, body, 'fixturepage') AS score FROM conversation_search_docs WHERE fts_match(title, body, 'fixturepage') ORDER BY score DESC LIMIT 512", ()).await?;
    let mut page_hits = 0;
    while rows.next().await?.is_some() {
        page_hits += 1;
    }
    assert_eq!(page_hits, PAGE_DOCUMENTS);
    drop(rows);
    for index in [0, PAGE_DOCUMENTS - 1] {
        let token = format!("pageprobe{index:04}");
        let mut rows = connection.query("SELECT key, fts_score(title, body, ?1) AS score FROM conversation_search_docs WHERE fts_match(title, body, ?1) ORDER BY score DESC LIMIT 10", [token]).await?;
        assert_eq!(
            rows.next()
                .await?
                .ok_or("missing page boundary match")?
                .get::<String>(0)?,
            format!("message:msg_page_{index:04}")
        );
        assert!(rows.next().await?.is_none());
    }
    // Drain the checkpoint statement, not just its preparation. The fixture is
    // published only after all handles have gone away and no WAL is required.
    let mut rows = connection
        .query("PRAGMA wal_checkpoint(TRUNCATE)", ())
        .await?;
    while let Some(row) = rows.next().await? {
        assert_eq!(row.get::<i64>(0)?, 0, "checkpoint busy");
    }
    drop(rows);
    drop(connection);
    drop(database);
    let wal = PathBuf::from(format!("{}-wal", path.display()));
    if wal.exists() && std::fs::metadata(wal)?.len() != 0 {
        return Err("fixture still requires WAL; do not copy it".into());
    }
    println!("Cold official Turso 0.6.1 FTS fixture: {}", path.display());
    Ok(())
}
