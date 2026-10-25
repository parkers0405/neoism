// Included inside state::conversation_search::tests so the consumer exercises
// the production migration with the application's official Turso dependency.
mod fts_fixture_data {
    include!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../../tools/fts-v1-fixture/fixture_data.rs"
    ));
}

async fn fts_fixture_db(path: &std::path::Path) -> Db {
    Db {
        database: turso::Builder::new_local(path.to_str().unwrap())
            .experimental_index_method(true)
            .build()
            .await
            .unwrap(),
        write_gate: Arc::new(Mutex::new(())),
        fail_next_assistant_insert: Arc::new(AtomicBool::new(false)),
    }
}

// Hex captures the actual canonical TEXT bytes, including whitespace, unknown
// JSON fields and Unicode. Projection rows/dirty state/backfill cursors are also
// compared without relying on a JSON model's round trip.
async fn fts_fixture_snapshot(db: &Db) -> Vec<Vec<String>> {
    let mut snapshot = Vec::new();
    for sql in [
        "SELECT hex(CAST(json_array(id,tenant_id,hex(CAST(info_json AS BLOB)),updated) AS BLOB)) AS bytes FROM sessions ORDER BY id",
        "SELECT hex(CAST(json_array(id,tenant_id,session_id,hex(CAST(message_json AS BLOB)),created,position) AS BLOB)) AS bytes FROM messages ORDER BY id",
        "SELECT hex(CAST(json_array(key,session_id,message_id,role,created,title,body) AS BLOB)) AS bytes FROM conversation_search_docs ORDER BY key",
        "SELECT hex(CAST(json_array(key,session_id,message_id,revision,ready_at) AS BLOB)) AS bytes FROM conversation_search_dirty ORDER BY key",
        "SELECT hex(CAST(json_array(name,state,cursor_updated,cursor_id,rows_done,updated_at,error) AS BLOB)) AS bytes FROM data_migrations WHERE name IN ('conversation-search-sessions-v1','conversation-search-messages-v1') ORDER BY name",
        // Append events at index 5: canonical JSON [..2], docs [2], and the
        // unchanged-state [3..] slice assertions below keep their meaning.
        "SELECT hex(CAST(json_array(seq,event_id,tenant_id,kind,aggregate_id,aggregate_seq,owner_id,session_id,typeof(event_json),hex(CAST(event_json AS BLOB)),created) AS BLOB)) AS bytes FROM events ORDER BY seq",
    ] {
        snapshot.push(
            db.fetch_all(sql, vec![])
                .await
                .unwrap()
                .iter()
                .map(|row| row.get_str("bytes").unwrap())
                .collect(),
        );
    }
    fts_fixture_events(db).await;
    snapshot
}

async fn fts_fixture_events(db: &Db) {
    let rows = db
        .fetch_all(
            "SELECT seq,event_id,event_json FROM events ORDER BY seq",
            vec![],
        )
        .await
        .unwrap();
    assert_eq!(rows.len(), 2);
    for (row, compressed) in rows.iter().zip([false, true]) {
        let value = row.value("event_json").unwrap();
        assert!(if compressed {
            matches!(value, SqlValue::Blob(_))
        } else {
            matches!(value, SqlValue::Text(_))
        });
        let expected = fts_fixture_data::event_json(compressed);
        assert_eq!(event_storage::decoded_len(value).unwrap(), expected.len());
        assert_eq!(
            event_storage::decode_json_bytes(value).unwrap(),
            expected.as_bytes()
        );
        let event = event_storage::decode_event(value).unwrap();
        assert_eq!(event.id.to_string(), row.get_str("event_id").unwrap());
        assert_eq!(event.kind, "message.part.updated");
        assert_eq!(event.sequence, Some(row.get_i64("seq").unwrap() as u64));
        assert_eq!(event.properties["sessionID"], "ses_fixture");
        assert_eq!(event.properties["opaque"], "  unchanged  ");
    }
}

async fn fts_fixture_pages(db: &Db) {
    let hits = fts_fixture_hits_limit(db, "fixturepage", 512).await;
    assert_eq!(hits.len(), fts_fixture_data::PAGE_DOCUMENTS);
    for index in [0, fts_fixture_data::PAGE_DOCUMENTS - 1] {
        assert_eq!(
            fts_fixture_hits(db, &format!("pageprobe{index:04}")).await,
            [format!("message:msg_page_{index:04}")]
        );
    }
    assert_eq!(
        db.fetch_optional(
            "SELECT body FROM conversation_search_docs WHERE key='message:msg_page_0383'",
            vec![]
        )
        .await
        .unwrap()
        .unwrap()
        .get_str("body")
        .unwrap(),
        fts_fixture_data::page_body(383)
    );
}

async fn fts_fixture_hits(db: &Db, token: &str) -> Vec<String> {
    fts_fixture_hits_limit(db, token, 10).await
}

async fn fts_fixture_hits_limit(db: &Db, token: &str, limit: i64) -> Vec<String> {
    let plan = db
        .fetch_all(
            &format!("EXPLAIN QUERY PLAN {CANDIDATES}"),
            vec![text(token), int(limit)],
        )
        .await
        .unwrap();
    assert!(
        plan.iter()
            .any(|row| row.values.iter().any(|value| matches!(value,
        SqlValue::Text(detail) if detail.contains("QUERY INDEX METHOD fts")))),
        "scalar fallback cannot validate native index migration"
    );
    db.fetch_all(CANDIDATES, vec![text(token), int(limit)])
        .await
        .unwrap()
        .iter()
        .map(|row| row.get_str("key").unwrap())
        .collect()
}

async fn fts_fixture_marker_count(db: &Db) -> i64 {
    db.fetch_scalar_i64(
        "SELECT count(*) FROM data_migrations WHERE name = ?",
        vec![text(FTS_FORMAT_MIGRATION)],
    )
    .await
    .unwrap()
}

#[tokio::test]
async fn fts_format_migration_commit_failure_rolls_back_and_retries() {
    let _search_test = SEARCH_TEST.lock().await;
    let store = store().await;
    store._event_compactor.as_ref().unwrap().shutdown().await;
    let db = &store.db;
    session(
        db,
        "ses_format",
        "format fixture",
        "/fixture",
        None,
        "local",
    )
    .await;
    message(
        db,
        "msg_format",
        "ses_format",
        "formatrollbacktoken",
        "local",
    )
    .await;
    flush(db).await.unwrap();
    db.execute(
        "DELETE FROM data_migrations WHERE name=?",
        vec![text(FTS_FORMAT_MIGRATION)],
    )
    .await
    .unwrap();

    let connection = db.database.connect().unwrap();
    connection
        .execute("PRAGMA foreign_keys=ON", ())
        .await
        .unwrap();
    connection
        .execute(
            "CREATE TABLE fts_commit_parent (id INTEGER PRIMARY KEY)",
            (),
        )
        .await
        .unwrap();
    connection.execute("CREATE TABLE fts_commit_child (parent INTEGER REFERENCES fts_commit_parent(id) DEFERRABLE INITIALLY DEFERRED)", ()).await.unwrap();
    connection.execute("BEGIN IMMEDIATE", ()).await.unwrap();
    rebuild_fts(&connection).await.unwrap();
    connection
        .execute("INSERT INTO fts_commit_child VALUES (999)", ())
        .await
        .unwrap();
    // A real COMMIT error (not an injected statement failure) leaves the raw
    // transaction open. Use the same completion/rollback path as migrate_fts.
    let result = connection.execute("COMMIT", ()).await.map(|_| ());
    assert!(result.is_err(), "deferred FK must fail at commit");
    assert!(finish_fts_transaction(&connection, result).await.is_err());
    assert_eq!(fts_fixture_marker_count(db).await, 0);
    assert_eq!(
        db.fetch_scalar_i64("SELECT count(*) FROM fts_commit_child", vec![])
            .await
            .unwrap(),
        0
    );
    assert_eq!(
        fts_fixture_hits(db, "formatrollbacktoken").await,
        ["message:msg_format"]
    );
    // Keep the failed attempt's connection alive: a second connection must be
    // able to acquire the writer immediately, proving no deferred rollback.
    let probe = db.database.connect().unwrap();
    probe.execute("BEGIN IMMEDIATE", ()).await.unwrap();
    probe.execute("ROLLBACK", ()).await.unwrap();
    drop(probe);
    drop(connection);

    // Rollback failure must be terminal, even if its diagnostic were Busy:
    // the retry wrapper must receive Ok(Err(..)), not a retryable outer error.
    let connection = db.database.connect().unwrap();
    let error = connection
        .execute(
            "INSERT INTO deliberately_missing_fts_migration_table VALUES (1)",
            (),
        )
        .await
        .unwrap_err();
    assert!(
        finish_fts_transaction(&connection, Err(error))
            .await
            .unwrap()
            .is_err()
    );
    drop(connection);

    migrate(db).await.unwrap();
    assert_eq!(fts_fixture_marker_count(db).await, 1);
    assert_eq!(
        fts_fixture_hits(db, "formatrollbacktoken").await,
        ["message:msg_format"]
    );
    db.execute(
        "UPDATE data_migrations SET updated_at=7 WHERE name=?",
        vec![text(FTS_FORMAT_MIGRATION)],
    )
    .await
    .unwrap();
    migrate(db).await.unwrap();
    assert_eq!(
        db.fetch_scalar_i64(
            "SELECT updated_at FROM data_migrations WHERE name=?",
            vec![text(FTS_FORMAT_MIGRATION)]
        )
        .await
        .unwrap(),
        7
    );
    assert_eq!(
        fts_fixture_hits(db, "formatrollbacktoken").await,
        ["message:msg_format"]
    );
}

#[tokio::test]
#[ignore = "requires cold official 0.6.1 fixture; see tools/fts-v1-fixture/README.md"]
async fn official_061_fts_fixture_migrates_to_v2() {
    let _search_test = SEARCH_TEST.lock().await;
    let source = PathBuf::from(
        std::env::var_os("NEOISM_FTS_V1_FIXTURE")
            .expect("set NEOISM_FTS_V1_FIXTURE to the producer's cold database"),
    );
    let wal = PathBuf::from(format!("{}-wal", source.display()));
    assert!(
        !wal.exists() || std::fs::metadata(wal).unwrap().len() == 0,
        "fixture must be checkpointed and its producer must have exited"
    );
    let directory = std::env::temp_dir().join(format!(
        "neoism-fts-upgrade-{}",
        neoism_agent_core::new_session_id()
    ));
    std::fs::create_dir(&directory).unwrap();
    let path = directory.join("fixture.db");
    // Never open the source, and never let the consumer mutate the golden file.
    std::fs::copy(&source, &path).unwrap();
    let db = fts_fixture_db(&path).await;
    assert_eq!(
        db.fetch_optional("SELECT producer FROM fixture_provenance", vec![])
            .await
            .unwrap()
            .unwrap()
            .get_str("producer")
            .unwrap(),
        "official-crates.io-turso-0.6.1"
    );
    assert_eq!(fts_fixture_marker_count(&db).await, 0);
    assert_eq!(
        db.fetch_scalar_i64("SELECT count(*) FROM conversation_search_docs", vec![])
            .await
            .unwrap(),
        fts_fixture_data::PAGE_DOCUMENTS as i64 + 2
    );
    let backing_bytes = db.fetch_scalar_i64("SELECT sum(length(bytes)) FROM __turso_internal_fts_dir_conversation_search_fts", vec![]).await.unwrap();
    let page_size = db
        .fetch_scalar_i64("PRAGMA page_size", vec![])
        .await
        .unwrap();
    assert!(
        backing_bytes > 4 * page_size,
        "old FTS fixture must cross multiple backing pages"
    );
    assert_eq!(
        backing_bytes,
        db.fetch_scalar_i64(
            "SELECT value FROM fixture_metrics WHERE name='old_fts_backing_bytes'",
            vec![]
        )
        .await
        .unwrap()
    );
    let before = fts_fixture_snapshot(&db).await;

    // Exercise the exact production DROP/CREATE/marker sequence on an old index,
    // then force a SQL failure. Do not MATCH/OPTIMIZE the incompatible index.
    let mut connection = db.database.connect().unwrap();
    let tx = connection
        .transaction_with_behavior(turso::transaction::TransactionBehavior::Immediate)
        .await
        .unwrap();
    rebuild_fts(&tx).await.unwrap();
    assert!(
        tx.execute(
            "INSERT INTO deliberately_missing_fts_migration_table VALUES (1)",
            ()
        )
        .await
        .is_err()
    );
    tx.rollback().await.unwrap();
    drop(connection);
    assert_eq!(fts_fixture_marker_count(&db).await, 0);
    assert_eq!(fts_fixture_snapshot(&db).await, before);
    assert_eq!(db.fetch_scalar_i64("SELECT count(*) FROM sqlite_master WHERE type='index' AND name='conversation_search_fts'", vec![]).await.unwrap(), 1);
    drop(db);

    // Retry after a cold reopen; rollback must not poison the schema/native FTS.
    let db = fts_fixture_db(&path).await;
    migrate(&db).await.unwrap();
    assert_eq!(fts_fixture_marker_count(&db).await, 1);
    fts_fixture_pages(&db).await;
    assert_eq!(fts_fixture_snapshot(&db).await, before);
    assert_eq!(
        fts_fixture_hits(&db, "legacyquartz").await,
        ["message:msg_fixture"]
    );
    assert!(fts_fixture_hits(&db, "modernzircon").await.is_empty());

    // A failed real UPDATE must roll back both document text and native tokens.
    assert!(db.execute_transaction(vec![
        ("UPDATE conversation_search_docs SET body='modernzircon' WHERE key='message:msg_fixture'".into(), vec![]),
        ("INSERT INTO deliberately_missing_fts_migration_table VALUES (1)".into(), vec![]),
    ]).await.is_err());
    assert_eq!(fts_fixture_snapshot(&db).await, before);
    assert_eq!(
        fts_fixture_hits(&db, "legacyquartz").await,
        ["message:msg_fixture"]
    );
    assert!(fts_fixture_hits(&db, "modernzircon").await.is_empty());

    // Successful retry: genuine UPDATE, not delete+insert/upsert.
    db.execute("UPDATE conversation_search_docs SET body='modernzircon' WHERE key='message:msg_fixture'", vec![]).await.unwrap();
    assert!(fts_fixture_hits(&db, "legacyquartz").await.is_empty());
    assert_eq!(
        fts_fixture_hits(&db, "modernzircon").await,
        ["message:msg_fixture"]
    );
    db.execute(
        "UPDATE data_migrations SET updated_at=7 WHERE name=?",
        vec![text(FTS_FORMAT_MIGRATION)],
    )
    .await
    .unwrap();
    migrate(&db).await.unwrap();
    assert_eq!(
        db.fetch_scalar_i64(
            "SELECT updated_at FROM data_migrations WHERE name=?",
            vec![text(FTS_FORMAT_MIGRATION)]
        )
        .await
        .unwrap(),
        7
    );
    drop(db);

    // Also use the real startup path: all canonical migrations precede the FTS
    // gate, which must complete before worker writes become possible.
    let store = SessionStore::open(path.clone()).await.unwrap();
    store._search_worker.as_ref().unwrap().shutdown().await;
    store._event_compactor.as_ref().unwrap().shutdown().await;
    let db = &store.db;
    fts_fixture_pages(db).await;
    assert!(fts_fixture_hits(db, "legacyquartz").await.is_empty());
    assert_eq!(
        fts_fixture_hits(db, "modernzircon").await,
        ["message:msg_fixture"]
    );
    let after = fts_fixture_snapshot(db).await;
    assert_eq!(&after[..2], &before[..2], "canonical JSON bytes changed");
    assert_eq!(
        &after[3..],
        &before[3..],
        "dirty rows, backfill markers, or canonical event bytes/metadata changed"
    );
    db.execute("OPTIMIZE INDEX conversation_search_fts", vec![])
        .await
        .unwrap();
    assert_eq!(
        fts_fixture_hits(db, "modernzircon").await,
        ["message:msg_fixture"]
    );
    db.execute(
        "DELETE FROM conversation_search_docs WHERE key='message:msg_fixture'",
        vec![],
    )
    .await
    .unwrap();
    assert!(fts_fixture_hits(db, "modernzircon").await.is_empty());
    db.execute("OPTIMIZE INDEX conversation_search_fts", vec![])
        .await
        .unwrap();
    drop(store);

    let db = fts_fixture_db(&path).await;
    migrate(&db).await.unwrap();
    fts_fixture_pages(&db).await;
    assert!(fts_fixture_hits(&db, "legacyquartz").await.is_empty());
    assert!(fts_fixture_hits(&db, "modernzircon").await.is_empty());
    let after = fts_fixture_snapshot(&db).await;
    assert_eq!(&after[..2], &before[..2]);
    assert_eq!(&after[3..], &before[3..]);
    drop(db);
    std::fs::remove_dir_all(directory).unwrap();
}
