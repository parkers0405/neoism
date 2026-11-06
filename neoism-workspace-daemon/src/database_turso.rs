//! Native local Turso browsing. Query-only connections; no migrations or checkpoints.
use super::*;

pub(super) fn is_turso_path(path: &Path) -> bool {
    let name = path.file_name().unwrap_or_default().to_string_lossy().to_ascii_lowercase();
    name.ends_with(".turso.db") || name.ends_with(".turso") || name.ends_with(".tursodb")
}

fn value(value: turso::Value) -> Result<DatabaseValue> {
    Ok(match value {
        turso::Value::Null => DatabaseValue::Null,
        turso::Value::Integer(n) => DatabaseValue::Integer(n.to_string()),
        turso::Value::Real(n) => { ensure!(n.is_finite(), "Non-finite real"); DatabaseValue::Real(n) },
        turso::Value::Text(s) => DatabaseValue::Text(s),
        turso::Value::Blob(b) => DatabaseValue::Blob(STANDARD.encode(b)),
    })
}

fn params(values: &[DatabaseValue]) -> Result<Vec<turso::Value>> {
    values.iter().map(|v| Ok(match v {
        DatabaseValue::Null => turso::Value::Null,
        DatabaseValue::Integer(n) => turso::Value::Integer(n.parse()?),
        DatabaseValue::Real(n) => { ensure!(n.is_finite(), "Non-finite real"); turso::Value::Real(*n) },
        DatabaseValue::Text(s) => { ensure!(s.len() < MAX_BYTES, "Text too large"); turso::Value::Text(s.clone()) },
        DatabaseValue::Blob(b) => { ensure!(b.len() < MAX_BYTES, "Blob too large"); turso::Value::Blob(STANDARD.decode(b)?) },
    })).collect()
}

async fn metadata(conn: &turso::Connection, name: &str) -> Result<Metadata> {
    let mut entries = conn.query("SELECT type,coalesce(sql,'') FROM sqlite_schema WHERE name=? AND type IN ('table','view') AND name NOT GLOB 'sqlite_*'", [name]).await?;
    let entry = entries.next().await?.context("Unknown table/view")?;
    let kind: String = entry.get(0)?;
    let mut columns = Vec::new();
    let mut info = conn.query(format!("PRAGMA table_xinfo({})", quote(name)), ()).await?;
    while let Some(row) = info.next().await? {
        if row.get::<i64>(6)? == 1 { continue; }
        columns.push(DatabaseColumn {
            name: row.get(1)?, data_type: row.get(2)?, not_null: row.get::<i64>(3)? != 0,
            default_value: match row.get_value(4)? { turso::Value::Null => None, turso::Value::Text(s) => Some(s), _ => None },
            primary_key: row.get::<i64>(5)? as u32,
        });
    }
    ensure!(!columns.is_empty(), "Object has no visible columns");
    Ok(Metadata { object: DatabaseObject {
        name: name.into(), kind: if kind == "view" { DatabaseObjectKind::View } else { DatabaseObjectKind::Table },
        sql: entry.get(1)?, columns, indexes: vec![], triggers: vec![],
    }, identity: vec![], writable: vec![] })
}

async fn connect(path: &Path) -> Result<turso::Connection> {
    let database = turso::Builder::new_local(path.to_str().context("Database path must be UTF-8")?)
        .read_only(true).experimental_index_method(true).build().await?;
    let conn = database.connect()?;
    // Turso's process-wide registry may reuse an already-open writer Database
    // in an embedded daemon. Enforce read-only on OUR connection as well as
    // requesting read-only file handles; this changes no persistent setting.
    conn.execute("PRAGMA query_only=1", ()).await?;
    Ok(conn)
}

pub(super) async fn execute(path: &Path, request: DatabaseRequest) -> Result<DatabaseReply> {
    // Reject writes before even opening the file. This backend intentionally
    // never exposes an editable identity, including for ordinary native tables.
    ensure!(!request.is_mutation(), "Local Turso databases are read-only in the workbench");
    let conn = connect(path).await?;
    let reply = match request {
        DatabaseRequest::Schema { .. } => {
            let mut entries = conn.query("SELECT name FROM sqlite_schema WHERE type IN ('table','view') AND name NOT GLOB 'sqlite_*' ORDER BY name", ()).await?;
            let mut objects = Vec::new();
            while let Some(entry) = entries.next().await? {
                let name: String = entry.get(0)?;
                let mut object = metadata(&conn, &name).await?.object;
                let mut indexes = conn.query(format!("PRAGMA index_list({})", quote(&name)), ()).await?;
                while let Some(index) = indexes.next().await? {
                    let index_name: String = index.get(1)?;
                    let mut info = conn.query(format!("PRAGMA index_info({})", quote(&index_name)), ()).await?;
                    let mut columns = Vec::new();
                    while let Some(column) = info.next().await? {
                        if let turso::Value::Text(s) = column.get_value(2)? { columns.push(s); }
                    }
                    object.indexes.push(DatabaseIndex { name: index_name, unique: index.get::<i64>(2)? != 0, columns });
                }
                let mut triggers = conn.query("SELECT name,coalesce(sql,'') FROM sqlite_schema WHERE type='trigger' AND tbl_name=? ORDER BY name", [name]).await?;
                while let Some(trigger) = triggers.next().await? {
                    object.triggers.push(DatabaseTrigger { name: trigger.get(0)?, sql: trigger.get(1)? });
                }
                objects.push(object);
                ensure!(serde_json::to_vec(&objects)?.len() < MAX_BYTES, "Schema exceeds database response limit");
            }
            DatabaseReply::Schema { objects }
        }
        DatabaseRequest::Browse { table, limit, offset, sort, filters, .. } => {
            let meta = metadata(&conn, &table).await?;
            let (predicate, bound) = filters_sql(&meta, &filters)?;
            let mut count = conn.query(format!("SELECT count(*) FROM {}{predicate}", table_sql(&table)), params(&bound)?).await?;
            let total: i64 = count.next().await?.context("Missing count")?.get(0)?;
            let columns: Vec<_> = meta.object.columns.iter().map(|c| c.name.clone()).collect();
            let order = if let Some(sort) = sort {
                meta.column(&sort.column)?;
                format!(" ORDER BY {} {}", quote(&sort.column), if sort.descending { "DESC" } else { "ASC" })
            } else { String::new() };
            let sql = format!("SELECT {} FROM {}{predicate}{order} LIMIT {} OFFSET {}", columns.iter().map(|c| quote(c)).collect::<Vec<_>>().join(","), table_sql(&table), limit.min(200), offset);
            let mut results = conn.query(sql, params(&bound)?).await?;
            let mut rows = Vec::new();
            let mut bytes = 0;
            while let Some(row) = results.next().await? {
                let values = (0..columns.len()).map(|i| value(row.get_value(i)?)).collect::<Result<Vec<_>>>()?;
                bytes += serde_json::to_vec(&values)?.len();
                ensure!(bytes < MAX_BYTES / 2, "Rows exceed database response limit; select a smaller page");
                rows.push(values);
            }
            DatabaseReply::Rows { columns, identities: vec![None; rows.len()], rows, total: total as u64, editable: false }
        }
        DatabaseRequest::Values { table, column, filters, .. } => {
            let meta = metadata(&conn, &table).await?;
            meta.column(&column)?;
            let (predicate, bound) = filters_sql(&meta, &filters)?;
            let sql = format!("SELECT {},count(*) FROM {}{predicate} GROUP BY {} ORDER BY count(*) DESC, {} LIMIT {}", quote(&column), table_sql(&table), quote(&column), quote(&column), MAX_VALUES + 1);
            let mut rows = conn.query(sql, params(&bound)?).await?;
            let mut values = Vec::new();
            let mut bytes = 0;
            let mut truncated = false;
            while let Some(row) = rows.next().await? {
                if values.len() == MAX_VALUES { truncated = true; break; }
                let entry = DatabaseValueCount { value: value(row.get_value(0)?)?, count: row.get::<i64>(1)? as u64 };
                bytes += serde_json::to_vec(&entry)?.len();
                ensure!(bytes < MAX_BYTES / 2, "Distinct values exceed database response limit");
                values.push(entry);
            }
            DatabaseReply::Values { values, truncated }
        }
        DatabaseRequest::Apply { .. } => unreachable!(),
    };
    ensure!(serde_json::to_vec(&reply)?.len() < MAX_BYTES, "Database response exceeds byte limit");
    Ok(reply)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn native_schema_browse_filters_and_write_rejection() -> Result<()> {
        let dir = tempfile::tempdir()?;
        let path = dir.path().join("fixture.turso.db");
        {
            let db = turso::Builder::new_local(path.to_str().unwrap()).build().await?;
            let conn = db.connect()?;
            conn.execute("CREATE TABLE items(id INTEGER PRIMARY KEY, name TEXT, data BLOB)", ()).await?;
            conn.execute("CREATE INDEX by_name ON items(name)", ()).await?;
            conn.execute("INSERT INTO items VALUES(9007199254740993,'alpha',x'00ff'),(2,'beta',NULL)", ()).await?;
        }
        let supplied = path.to_string_lossy().into_owned();
        let schema = execute(&path, DatabaseRequest::Schema { path: supplied.clone() }).await?;
        let DatabaseReply::Schema { objects } = schema else { bail!("Expected schema") };
        assert_eq!(objects[0].columns.len(), 3);
        assert_eq!(objects[0].indexes[0].columns, ["name"]);
        let browse = execute(&path, DatabaseRequest::Browse { path: supplied.clone(), table: "items".into(), limit: 1, offset: 0,
            sort: Some(DatabaseSort { column: "id".into(), descending: true }), filters: vec![] }).await?;
        let DatabaseReply::Rows { rows, total, editable, identities, .. } = browse else { bail!("Expected rows") };
        assert_eq!(total, 2);
        assert!(!editable);
        assert_eq!(identities, [None]);
        assert_eq!(rows[0][0], DatabaseValue::Integer("9007199254740993".into()));
        assert_eq!(rows[0][2], DatabaseValue::Blob("AP8=".into()));
        let values = execute(&path, DatabaseRequest::Values { path: supplied.clone(), table: "items".into(), column: "name".into(),
            filters: vec![DatabaseFilter { column: "id".into(), values: vec![DatabaseValue::Integer("2".into())] }] }).await?;
        let DatabaseReply::Values { values, .. } = values else { bail!("Expected values") };
        assert_eq!(values[0].value, DatabaseValue::Text("beta".into()));
        assert!(execute(&path, DatabaseRequest::Apply { path: supplied, table: "items".into(), changes: vec![] }).await.unwrap_err().to_string().contains("read-only"));
        let conn = connect(&path).await?;
        assert!(conn.execute("DELETE FROM items", ()).await.is_err());
        drop(conn);
        // The embedded daemon can already own a writer for this same file.
        let writer = turso::Builder::new_local(path.to_str().unwrap()).build().await?;
        let writer_conn = writer.connect()?;
        let conn = connect(&path).await?;
        assert!(conn.execute("DELETE FROM items", ()).await.is_err());
        // query_only is connection-local, not a change to the agent's handle.
        writer_conn.execute("INSERT INTO items VALUES(3,'writer',NULL)", ()).await?;
        assert!(!is_turso_path(Path::new("ordinary.db")));
        Ok(())
    }

    /// Opt-in verification of the real file through the exact workbench code.
    /// Never emits schema SQL, names, counts, or user record values.
    #[tokio::test]
    #[ignore = "requires NEOISM_TURSO_READONLY_TEST_PATH; reads personal database"]
    async fn real_file_schema_and_limited_browse() -> Result<()> {
        let supplied = std::env::var("NEOISM_TURSO_READONLY_TEST_PATH")?;
        let path = Path::new(&supplied);
        let reply = super::super::handle(path.parent().context("Missing parent")?, DatabaseRequest::Schema { path: supplied.clone() }).await;
        let DatabaseReply::Schema { objects } = reply else { bail!("Schema failed: {reply:?}") };
        ensure!(!objects.is_empty(), "No schema objects");
        for object in objects {
            let reply = super::super::handle(path.parent().unwrap(), DatabaseRequest::Browse { path: supplied.clone(), table: object.name,
                limit: 1, offset: 0, sort: None, filters: vec![] }).await;
            ensure!(matches!(reply, DatabaseReply::Rows { editable: false, .. }), "Browse failed (record data suppressed)");
        }
        Ok(())
    }
}