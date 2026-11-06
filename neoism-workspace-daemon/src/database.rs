//! SQLite work is executed by SQLx's dedicated SQLite worker, never the socket pump.
//! Explicit local Turso filenames use the native engine, strictly read-only.
//! No SQL supplied by the client is executed; names are validated then quoted.
use anyhow::{bail, ensure, Context, Result};
use base64::{engine::general_purpose::STANDARD, Engine};
use futures::TryStreamExt;
use neoism_protocol::database::*;
use sqlx::{sqlite::{SqliteConnectOptions, SqliteRow}, Connection, Row, SqliteConnection, TypeInfo, ValueRef};
use std::{path::Path, time::Duration};

#[path = "database_turso.rs"]
mod turso_readonly;

const MAX_BYTES: usize = 4 * 1024 * 1024;
const MAX_VALUES: usize = 200;
fn quote(name: &str) -> String { format!("\"{}\"", name.replace('"', "\"\"")) }
fn table_sql(name: &str) -> String { format!("main.{}", quote(name)) }
pub(crate) fn error_reply(message: impl AsRef<str>) -> DatabaseReply {
    DatabaseReply::Error { message: message.as_ref().chars().take(2000).collect() }
}

pub async fn handle(root: &Path, request: DatabaseRequest) -> DatabaseReply {
    match execute(root, request).await {
        Ok(reply) => reply,
        Err(error) => error_reply(format!("{error:#}")),
    }
}

async fn execute(root: &Path, request: DatabaseRequest) -> Result<DatabaseReply> {
    let root = root.to_owned();
    let supplied = request.path().to_owned();
    let path = tokio::task::spawn_blocking(move || {
        let path = crate::files::resolve_existing_path(&root, &supplied).map_err(anyhow::Error::msg)?;
        ensure!(path.is_file(), "Database path must be an existing regular file");
        Ok::<_, anyhow::Error>(path)
    }).await??;
    // Explicit Turso filenames select the native engine, not a guess based on
    // the SQLite header (Turso can have native indexes and a live WAL).
    if turso_readonly::is_turso_path(&path) {
        return turso_readonly::execute(&path, request).await;
    }
    let options = SqliteConnectOptions::new().filename(&path).create_if_missing(false)
        .read_only(!request.is_mutation()).foreign_keys(true)
        .busy_timeout(Duration::from_millis(750))
        .pragma("trusted_schema", "OFF");
    let mut conn = SqliteConnection::connect_with(&options).await?;
    // Force schema validation even on a Schema request against a non-database file.
    sqlx::query("SELECT count(*) FROM main.sqlite_schema").fetch_one(&mut conn).await?;
    let reply = match request {
        DatabaseRequest::Schema { .. } => {
            let entries = sqlx::query("SELECT name FROM main.sqlite_schema WHERE type IN ('table','view') AND name NOT GLOB 'sqlite_*' ORDER BY name").fetch_all(&mut conn).await?;
            let mut objects = Vec::new();
            for entry in entries {
                let name: String = entry.try_get("name")?;
                let mut object = metadata(&mut conn, &name).await?.object;
                for index in sqlx::query("SELECT name,\"unique\" FROM pragma_index_list(?)").bind(&name).fetch_all(&mut conn).await? {
                    let index_name: String = index.try_get("name")?;
                    let columns = sqlx::query("SELECT name FROM pragma_index_info(?) ORDER BY seqno").bind(&index_name).fetch_all(&mut conn).await?.into_iter()
                        .filter_map(|r| r.try_get::<Option<String>, _>("name").ok().flatten()).collect();
                    object.indexes.push(DatabaseIndex { name: index_name, unique: index.try_get::<i64,_>("unique")? != 0, columns });
                }
                object.triggers = sqlx::query("SELECT name,coalesce(sql,'') AS sql FROM main.sqlite_schema WHERE type='trigger' AND tbl_name=? ORDER BY name").bind(&name).fetch_all(&mut conn).await?.into_iter()
                    .map(|r| Ok(DatabaseTrigger { name: r.try_get("name")?, sql: r.try_get("sql")? })).collect::<Result<_>>()?;
                objects.push(object);
                ensure!(serde_json::to_vec(&objects)?.len() < MAX_BYTES, "Schema exceeds database response limit");
            }
            DatabaseReply::Schema { objects }
        }
        DatabaseRequest::Browse { table, limit, offset, sort, filters, .. } => {
            let meta = metadata(&mut conn, &table).await?;
            let (predicate, params) = filters_sql(&meta, &filters)?;
            let total: i64 = bound_query(&format!("SELECT count(*) FROM {}{predicate}", table_sql(&table)), &params)?.fetch_one(&mut conn).await?.try_get(0)?;
            let columns: Vec<String> = meta.object.columns.iter().map(|c| c.name.clone()).collect();
            let mut selection: Vec<String> = columns.iter().map(|c| quote(c)).collect();
            // Synthetic rowid is returned separately, never mixed into user columns.
            let rowid = meta.identity.len() == 1 && !columns.contains(&meta.identity[0]);
            if rowid { selection.push(quote(&meta.identity[0])); }
            let mut ordering = Vec::new();
            if let Some(sort) = sort {
                meta.column(&sort.column)?;
                ordering.push(format!("{} {}", quote(&sort.column), if sort.descending { "DESC" } else { "ASC" }));
            }
            ordering.extend(meta.identity.iter().map(|c| quote(c)));
            let order = if ordering.is_empty() { String::new() } else { format!(" ORDER BY {}", ordering.join(",")) };
            let sql = format!("SELECT {} FROM {}{predicate}{order} LIMIT ? OFFSET ?", selection.join(","), table_sql(&table));
            let mut rows = Vec::new();
            let mut identities = Vec::new();
            let mut bytes = 0;
            let mut results = bound_query(&sql, &params)?.bind(i64::from(limit.min(200))).bind(i64::from(offset)).fetch(&mut conn);
            while let Some(row) = results.try_next().await? {
                let values = (0..columns.len()).map(|i| read_value(&row, i)).collect::<Result<Vec<_>>>()?;
                let identity = if meta.identity.is_empty() { None } else {
                    let mut identity = DatabaseRecord::new();
                    for key in &meta.identity {
                        let idx = columns.iter().position(|c| c == key).unwrap_or(columns.len());
                        identity.insert(key.clone(), read_value(&row, idx)?);
                    }
                    Some(identity)
                };
                bytes += serde_json::to_vec(&(&values, &identity))?.len();
                ensure!(bytes < MAX_BYTES / 2, "Rows exceed database response limit; select a smaller page");
                rows.push(values); identities.push(identity);
            }
            DatabaseReply::Rows { columns, rows, identities, total: total as u64, editable: !meta.identity.is_empty() }
        }
        DatabaseRequest::Values { table, column, filters, .. } => {
            let meta = metadata(&mut conn, &table).await?;
            meta.column(&column)?;
            let (predicate, params) = filters_sql(&meta, &filters)?;
            let sql = format!("SELECT {},count(*) FROM {}{predicate} GROUP BY {} ORDER BY count(*) DESC, {} LIMIT {}", quote(&column), table_sql(&table), quote(&column), quote(&column), MAX_VALUES + 1);
            let mut results = bound_query(&sql, &params)?.fetch(&mut conn);
            let mut values = Vec::new();
            let mut bytes = 0;
            let mut truncated = false;
            while let Some(row) = results.try_next().await? {
                if values.len() == MAX_VALUES { truncated = true; break; }
                let entry = DatabaseValueCount { value: read_value(&row, 0)?, count: row.try_get::<i64,_>(1)? as u64 };
                bytes += serde_json::to_vec(&entry)?.len();
                ensure!(bytes < MAX_BYTES / 2, "Distinct values exceed database response limit");
                values.push(entry);
            }
            DatabaseReply::Values { values, truncated }
        }
        DatabaseRequest::Apply { table, changes, .. } => {
            ensure!(changes.len() <= 200, "Apply is capped at 200 changes");
            // IMMEDIATE serializes identity/original checks with the ensuing write.
            let mut tx = conn.begin_with("BEGIN IMMEDIATE").await?;
            let meta = metadata(&mut tx, &table).await?;
            ensure!(!meta.identity.is_empty(), "Object has no safe editable identity");
            let mut affected = 0;
            for change in changes {
                for key in change.values.keys() {
                    meta.column(key)?;
                    ensure!(meta.writable.contains(key), "Generated/hidden column cannot be written: {key}");
                }
                let (sql, params) = match change.kind {
                    DatabaseChangeKind::Insert => {
                        ensure!(change.identity.is_none() && change.original.is_none(), "Insert cannot carry identity/original");
                        // Override schema-level REPLACE/IGNORE conflict policies:
                        // an editor insert must never silently delete another row.
                        if change.values.is_empty() { (format!("INSERT OR ABORT INTO {} DEFAULT VALUES", table_sql(&table)), vec![]) }
                        else { (format!("INSERT OR ABORT INTO {} ({}) VALUES ({})", table_sql(&table), change.values.keys().map(|k| quote(k)).collect::<Vec<_>>().join(","), vec!["?"; change.values.len()].join(",")), change.values.values().cloned().collect()) }
                    }
                    DatabaseChangeKind::Update | DatabaseChangeKind::Delete => {
                        let identity = change.identity.context("Identity is required")?;
                        ensure!(identity.len() == meta.identity.len() && meta.identity.iter().all(|k| identity.contains_key(k)), "Identity must contain exactly the safe key columns");
                        ensure!(identity.values().all(|v| !matches!(v, DatabaseValue::Null)), "Null identity is unsafe");
                        if meta.identity.iter().any(|k| !meta.object.columns.iter().any(|c| &c.name == k)) {
                            ensure!(identity.values().all(|v| matches!(v, DatabaseValue::Integer(_))), "Rowid must be an integer");
                        }
                        let original = change.original.context("Full original row is required for conflict detection")?;
                        ensure!(original.len() == meta.object.columns.len() && meta.object.columns.iter().all(|c| original.contains_key(&c.name)), "Original must contain every visible column exactly once");
                        let mut predicates = Vec::new(); let mut conditions = Vec::new();
                        for (key, value) in identity.iter().chain(original.iter()) {
                            // IS handles NULL, BINARY defeats a column's NOCASE
                            // collation, and typeof catches integer->real (or
                            // other storage-class) changes that SQLite's normal
                            // numeric equality would otherwise consider equal.
                            let storage = match value {
                                DatabaseValue::Null => "null", DatabaseValue::Integer(_) => "integer",
                                DatabaseValue::Real(_) => "real", DatabaseValue::Text(_) => "text",
                                DatabaseValue::Blob(_) => "blob",
                            };
                            predicates.push(format!("(typeof({0})=? AND {0} COLLATE BINARY IS ?)", quote(key)));
                            conditions.push(DatabaseValue::Text(storage.into()));
                            conditions.push(value.clone());
                        }
                        let prefix = if matches!(change.kind, DatabaseChangeKind::Update) {
                            ensure!(!change.values.is_empty(), "Update has no values");
                            format!("UPDATE OR ABORT {} SET {}", table_sql(&table), change.values.keys().map(|k| format!("{}=?", quote(k))).collect::<Vec<_>>().join(","))
                        } else { ensure!(change.values.is_empty(), "Delete values must be empty"); format!("DELETE FROM {}", table_sql(&table)) };
                        let mut params: Vec<_> = if matches!(change.kind, DatabaseChangeKind::Update) { change.values.values().cloned().collect() } else { vec![] };
                        params.extend(conditions);
                        (format!("{prefix} WHERE {}", predicates.join(" AND ")), params)
                    }
                };
                let count = bound_query(&sql, &params)?.execute(&mut *tx).await?.rows_affected();
                ensure!(count == 1, "Conflict: expected exactly one affected row, got {count}");
                affected += count;
            }
            tx.commit().await?;
            DatabaseReply::Applied { affected }
        }
    };
    ensure!(serde_json::to_vec(&reply)?.len() < MAX_BYTES, "Database response exceeds byte limit");
    Ok(reply)
}

struct Metadata { object: DatabaseObject, identity: Vec<String>, writable: Vec<String> }
impl Metadata {
    fn column(&self, name: &str) -> Result<()> {
        ensure!(self.object.columns.iter().any(|c| c.name == name), "Unknown column: {name}"); Ok(())
    }
}
async fn metadata(conn: &mut SqliteConnection, name: &str) -> Result<Metadata> {
    let entry = sqlx::query("SELECT type,coalesce(sql,'') AS sql FROM main.sqlite_schema WHERE name=? AND type IN ('table','view') AND name NOT GLOB 'sqlite_*'").bind(name).fetch_optional(&mut *conn).await?.context("Unknown table/view")?;
    let kind: String = entry.try_get("type")?;
    let mut columns = Vec::new(); let mut writable = Vec::new();
    for row in sqlx::query("SELECT name,type,\"notnull\",dflt_value,pk,hidden FROM pragma_table_xinfo(?) ORDER BY cid").bind(name).fetch_all(&mut *conn).await? {
        let hidden: i64 = row.try_get("hidden")?;
        if hidden == 1 { continue; }
        let column = DatabaseColumn { name: row.try_get("name")?, data_type: row.try_get("type")?, not_null: row.try_get::<i64,_>("notnull")? != 0, default_value: row.try_get("dflt_value")?, primary_key: row.try_get::<i64,_>("pk")? as u32 };
        if hidden == 0 { writable.push(column.name.clone()); }
        columns.push(column);
    }
    ensure!(!columns.is_empty(), "Object has no visible columns");
    let info = sqlx::query("SELECT type,wr FROM pragma_table_list() WHERE schema='main' AND name=?").bind(name).fetch_one(&mut *conn).await?;
    let ordinary = info.try_get::<String,_>("type")? == "table";
    let mut identity = Vec::new();
    if ordinary && kind == "table" {
        let mut pk: Vec<_> = columns.iter().filter(|c| c.primary_key > 0).collect();
        pk.sort_by_key(|c| c.primary_key);
        // INTEGER PRIMARY KEY is implicitly non-null even when table_xinfo
        // reports notnull=0. A separate pk index distinguishes the historical
        // `INTEGER PRIMARY KEY DESC` exception, which is nullable, not an alias.
        let integer_alias = pk.len() == 1 && pk[0].data_type.eq_ignore_ascii_case("INTEGER")
            && info.try_get::<i64,_>("wr")? == 0
            && sqlx::query("SELECT count(*) FROM pragma_index_list(?) WHERE origin='pk'").bind(name).fetch_one(&mut *conn).await?.try_get::<i64,_>(0)? == 0;
        if !pk.is_empty() && (pk.iter().all(|c| c.not_null) || integer_alias) { identity = pk.iter().map(|c| c.name.clone()).collect(); }
        else if info.try_get::<i64,_>("wr")? == 0 {
            if let Some(alias) = ["rowid", "_rowid_", "oid"].iter().find(|alias| !columns.iter().any(|c| c.name.eq_ignore_ascii_case(alias))) {
                identity.push((*alias).to_string());
            }
        }
    }
    Ok(Metadata { object: DatabaseObject { name: name.into(), kind: if kind == "view" { DatabaseObjectKind::View } else { DatabaseObjectKind::Table }, sql: entry.try_get("sql")?, columns, indexes: vec![], triggers: vec![] }, identity, writable })
}

fn filters_sql(meta: &Metadata, filters: &[DatabaseFilter]) -> Result<(String, Vec<DatabaseValue>)> {
    ensure!(filters.len() <= 100, "Too many filters");
    let mut predicates = Vec::new(); let mut params = Vec::new();
    for filter in filters {
        meta.column(&filter.column)?;
        ensure!(filter.values.len() <= 200, "Too many filter values");
        if filter.values.is_empty() { predicates.push("0".into()); continue; }
        predicates.push(format!("({})", filter.values.iter().map(|_| format!("{} IS ?", quote(&filter.column))).collect::<Vec<_>>().join(" OR ")));
        params.extend(filter.values.clone());
    }
    ensure!(params.len() <= 900, "Too many bound filter values");
    Ok((if predicates.is_empty() { String::new() } else { format!(" WHERE {}", predicates.join(" AND ")) }, params))
}
fn bound_query<'a>(sql: &'a str, values: &[DatabaseValue]) -> Result<sqlx::query::Query<'a, sqlx::Sqlite, sqlx::sqlite::SqliteArguments<'a>>> {
    let mut query = sqlx::query(sql);
    for value in values {
        query = match value {
            DatabaseValue::Null => query.bind(None::<String>),
            DatabaseValue::Integer(v) => query.bind(v.parse::<i64>().context("Invalid signed 64-bit integer")?),
            DatabaseValue::Real(v) => { ensure!(v.is_finite(), "Non-finite real"); query.bind(*v) },
            DatabaseValue::Text(v) => { ensure!(v.len() < MAX_BYTES, "Text too large"); query.bind(v.clone()) },
            DatabaseValue::Blob(v) => { ensure!(v.len() < MAX_BYTES, "Blob too large"); query.bind(STANDARD.decode(v).context("Invalid base64 blob")?) },
        };
    }
    Ok(query)
}
fn read_value(row: &SqliteRow, index: usize) -> Result<DatabaseValue> {
    let raw = row.try_get_raw(index)?;
    if raw.is_null() { return Ok(DatabaseValue::Null); }
    Ok(match raw.type_info().name() {
        "INTEGER" | "INT" | "BOOLEAN" => DatabaseValue::Integer(row.try_get::<i64,_>(index)?.to_string()),
        "REAL" => { let value: f64 = row.try_get(index)?; ensure!(value.is_finite(), "Non-finite SQLite real cannot be represented"); DatabaseValue::Real(value) },
        "TEXT" => { let value: String = row.try_get(index)?; ensure!(value.len() < MAX_BYTES / 2, "Cell exceeds response limit"); DatabaseValue::Text(value) },
        "BLOB" => { let value: Vec<u8> = row.try_get(index)?; ensure!(value.len() < MAX_BYTES / 2, "Cell exceeds response limit"); DatabaseValue::Blob(STANDARD.encode(value)) },
        other => bail!("Unsupported SQLite storage type: {other}"),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::{json, Value};
    use tempfile::TempDir;

    async fn fixture() -> TempDir {
        let dir = tempfile::tempdir().unwrap();
        let options = SqliteConnectOptions::new().filename(dir.path().join("test.sqlite")).create_if_missing(true);
        let mut conn = SqliteConnection::connect_with(&options).await.unwrap();
        sqlx::raw_sql("CREATE TABLE items (id INTEGER PRIMARY KEY, name TEXT NOT NULL DEFAULT 'default', n INTEGER, data BLOB); CREATE INDEX by_name ON items(name); CREATE TABLE audit (id INTEGER); CREATE TRIGGER log_insert AFTER INSERT ON items BEGIN INSERT INTO audit VALUES(new.id); END; CREATE VIEW names AS SELECT name FROM items; INSERT INTO items VALUES(9007199254740993,'alpha',NULL,x'00ff'),(2,'beta',7,NULL),(3,'beta',8,NULL); CREATE TABLE unsafe (rowid TEXT, _rowid_ TEXT, oid TEXT, k TEXT PRIMARY KEY); CREATE TABLE safe (a TEXT NOT NULL, b INTEGER NOT NULL, v TEXT, PRIMARY KEY(a,b)) WITHOUT ROWID; INSERT INTO safe VALUES('a',1,'old'); CREATE TABLE parent (id INTEGER PRIMARY KEY); CREATE TABLE child (id INTEGER PRIMARY KEY, parent INTEGER REFERENCES parent(id)); CREATE TABLE generated (id INTEGER PRIMARY KEY, n INTEGER, twice INTEGER GENERATED ALWAYS AS (n*2));").execute(&mut conn).await.unwrap();
        conn.close().await.unwrap();
        dir
    }
    async fn call(dir: &TempDir, request: Value) -> Value {
        serde_json::to_value(handle(dir.path(), serde_json::from_value(request).unwrap()).await).unwrap()
    }
    fn browse(table: &str) -> Value { json!({"Browse":{"path":"test.sqlite","table":table,"limit":200,"offset":0,"filters":[]}}) }
    fn integer(value: &str) -> Value { json!({"kind":"integer","value":value}) }
    fn text(value: &str) -> Value { json!({"kind":"text","value":value}) }
    async fn apply(dir: &TempDir, table: &str, changes: Value) -> Value { call(dir,json!({"Apply":{"path":"test.sqlite","table":table,"changes":changes}})).await }

    #[tokio::test]
    async fn schema_precision_blobs_and_paging() {
        let dir = fixture().await;
        let schema = call(&dir,json!({"Schema":{"path":"test.sqlite"}})).await;
        let items = schema["Schema"]["objects"].as_array().unwrap().iter().find(|o| o["name"] == "items").unwrap();
        assert_eq!(items["kind"],"table");
        assert_eq!(items["columns"][1]["default_value"],"'default'");
        assert_eq!(items["indexes"][0]["columns"],json!(["name"]));
        assert_eq!(items["triggers"][0]["name"],"log_insert");
        let reply = call(&dir,json!({"Browse":{"path":"test.sqlite","table":"items","limit":1,"offset":2,"sort":{"column":"id","descending":false},"filters":[]}})).await;
        assert_eq!(reply["Rows"]["total"],3);
        assert_eq!(reply["Rows"]["rows"][0][0],integer("9007199254740993"));
        assert_eq!(reply["Rows"]["rows"][0][3],json!({"kind":"blob","value":"AP8="}));
        assert_eq!(reply["Rows"]["identities"][0]["id"],integer("9007199254740993"));
    }
    #[tokio::test]
    async fn parameterized_filters_distinct_and_identifier_validation() {
        let dir = fixture().await;
        let reply = call(&dir,json!({"Browse":{"path":"test.sqlite","table":"items","limit":999,"offset":1,"filters":[{"column":"name","values":[text("beta")]}]}})).await;
        assert_eq!(reply["Rows"]["total"],2);
        assert_eq!(reply["Rows"]["rows"].as_array().unwrap().len(),1);
        let values = call(&dir,json!({"Values":{"path":"test.sqlite","table":"items","column":"name","filters":[]}})).await;
        assert_eq!(values["Values"]["values"][0],json!({"value":text("beta"),"count":2}));
        let null = call(&dir,json!({"Values":{"path":"test.sqlite","table":"items","column":"id","filters":[{"column":"n","values":[{"kind":"null"}]}]}})).await;
        assert_eq!(null["Values"]["values"][0]["value"],integer("9007199254740993"));
        let injection = call(&dir,json!({"Values":{"path":"test.sqlite","table":"items","column":"name","filters":[{"column":"name","values":[text("' OR 1=1 --")]}]}})).await;
        assert_eq!(injection["Values"]["values"],json!([]));
        let invalid = call(&dir,json!({"Values":{"path":"test.sqlite","table":"items","column":"name); DROP TABLE items;--","filters":[]}})).await;
        assert!(invalid.get("Error").is_some());
        assert!(call(&dir,browse("items; DROP TABLE items")).await.get("Error").is_some());
    }
    #[tokio::test]
    async fn crud_defaults_conflicts_and_transaction_rollback() {
        let dir = fixture().await;
        assert_eq!(apply(&dir,"items",json!([{"kind":"insert","values":{"id":integer("4")}}])).await,json!({"Applied":{"affected":1}}));
        let all = call(&dir,browse("items")).await;
        let rows = all["Rows"]["rows"].as_array().unwrap();
        let index = rows.iter().position(|r| r[0] == integer("4")).unwrap();
        assert_eq!(rows[index][1],text("default"));
        let original = json!({"id":rows[index][0],"name":rows[index][1],"n":rows[index][2],"data":rows[index][3]});
        let identity = all["Rows"]["identities"][index].clone();
        let update = json!({"kind":"update","values":{"name":text("changed")},"identity":identity,"original":original});
        assert_eq!(apply(&dir,"items",json!([update])).await,json!({"Applied":{"affected":1}}));
        assert!(apply(&dir,"items",json!([update])).await.get("Error").is_some());
        // First insert is rolled back when the second change conflicts.
        assert!(apply(&dir,"items",json!([{"kind":"insert","values":{"id":integer("5")}},update])).await.get("Error").is_some());
        assert_eq!(call(&dir,browse("items")).await["Rows"]["total"],4);
        let mut changed_original = original.clone(); changed_original["name"] = text("changed");
        assert_eq!(apply(&dir,"items",json!([{"kind":"delete","values":{},"identity":identity,"original":changed_original}])).await,json!({"Applied":{"affected":1}}));
        assert_eq!(call(&dir,browse("items")).await["Rows"]["total"],3);
        assert!(apply(&dir,"child",json!([{"kind":"insert","values":{"id":integer("1"),"parent":integer("999")}}])).await.get("Error").is_some());
    }
    #[tokio::test]
    async fn unsafe_edits_and_full_original_required() {
        let dir = fixture().await;
        for table in ["names","unsafe"] {
            assert_eq!(call(&dir,browse(table)).await["Rows"]["editable"],false);
            assert!(apply(&dir,table,json!([{"kind":"insert","values":{}}])).await.get("Error").is_some());
        }
        let safe = call(&dir,browse("safe")).await;
        assert_eq!(safe["Rows"]["identities"][0],json!({"a":text("a"),"b":integer("1")}));
        for identity in [json!({}),json!({"a":text("a")}),json!({"a":{"kind":"null"},"b":integer("1")}),json!({"a":text("a"),"b":integer("1"),"v":text("old")})] {
            assert!(apply(&dir,"safe",json!([{"kind":"delete","values":{},"identity":identity,"original":{"a":text("a"),"b":integer("1"),"v":text("old")}}])).await.get("Error").is_some());
        }
        assert!(apply(&dir,"safe",json!([{"kind":"delete","values":{},"identity":{"a":text("a"),"b":integer("1")}}])).await.get("Error").is_some());
        assert!(apply(&dir,"generated",json!([{"kind":"insert","values":{"twice":integer("2")}}])).await.get("Error").is_some());
    }
    #[tokio::test]
    async fn bounds_and_invalid_values() {
        let dir = fixture().await;
        let mut conn = SqliteConnection::connect_with(&SqliteConnectOptions::new().filename(dir.path().join("test.sqlite"))).await.unwrap();
        sqlx::raw_sql("WITH RECURSIVE n(x) AS (VALUES(10) UNION ALL SELECT x+1 FROM n WHERE x<250) INSERT INTO items(id,name) SELECT x, printf('%d',x) FROM n").execute(&mut conn).await.unwrap();
        let reply = call(&dir,browse("items")).await;
        assert_eq!(reply["Rows"]["rows"].as_array().unwrap().len(),200);
        let reply = call(&dir,json!({"Values":{"path":"test.sqlite","table":"items","column":"name","filters":[]}})).await;
        assert_eq!(reply["Values"]["truncated"],true);
        assert_eq!(reply["Values"]["values"].as_array().unwrap().len(),200);
        for value in [integer("9223372036854775808"),json!({"kind":"blob","value":"!invalid!"})] {
            assert!(apply(&dir,"items",json!([{"kind":"insert","values":{"id":value}}])).await.get("Error").is_some());
        }
        sqlx::query("INSERT INTO items(id,name) VALUES(9223372036854775807,?)").bind("x".repeat(MAX_BYTES)).execute(&mut conn).await.unwrap();
        assert!(call(&dir,json!({"Browse":{"path":"test.sqlite","table":"items","limit":1,"offset":0,"sort":{"column":"id","descending":true},"filters":[]}})).await.get("Error").is_some());
    }
    #[tokio::test]
    async fn existing_paths_only_and_escape_rejected() {
        let dir = fixture().await;
        for path in ["missing.sqlite","../test.sqlite","."] {
            assert!(call(&dir,json!({"Schema":{"path":path}})).await.get("Error").is_some());
        }
        assert!(!dir.path().join("missing.sqlite").exists());
        let external = fixture().await;
        assert!(call(&dir,json!({"Schema":{"path":external.path().join("test.sqlite")}})).await.get("Error").is_some());
        #[cfg(unix)] {
            std::os::unix::fs::symlink(external.path().join("test.sqlite"), dir.path().join("escape.sqlite")).unwrap();
            assert!(call(&dir,json!({"Schema":{"path":"escape.sqlite"}})).await.get("Error").is_some());
        }
    }

    #[tokio::test]
    async fn quoted_names_composite_keys_and_null_vs_default() {
        let dir = fixture().await;
        let mut conn = SqliteConnection::connect_with(&SqliteConnectOptions::new().filename(dir.path().join("test.sqlite"))).await.unwrap();
        sqlx::raw_sql("CREATE TABLE \"odd\"\"name\" (\"a\"\"b\" TEXT DEFAULT 'fallback', n REAL); INSERT INTO \"odd\"\"name\" DEFAULT VALUES; INSERT INTO \"odd\"\"name\" VALUES(NULL,1.25)").execute(&mut conn).await.unwrap();
        let rows = call(&dir,browse("odd\"name")).await;
        assert_eq!(rows["Rows"]["rows"][0][0],text("fallback"));
        assert_eq!(rows["Rows"]["rows"][1][0],json!({"kind":"null"}));
        assert_eq!(rows["Rows"]["rows"][1][1],json!({"kind":"real","value":1.25}));
        let filtered = call(&dir,json!({"Values":{"path":"test.sqlite","table":"odd\"name","column":"a\"b","filters":[{"column":"a\"b","values":[text("fallback")]}]}})).await;
        assert_eq!(filtered["Values"]["values"][0]["count"],1);
        let updated = apply(&dir,"safe",json!([{"kind":"update","values":{"v":text("new")},"identity":{"a":text("a"),"b":integer("1")},"original":{"a":text("a"),"b":integer("1"),"v":text("old")}}])).await;
        assert_eq!(updated,json!({"Applied":{"affected":1}}));
        assert_eq!(call(&dir,browse("safe")).await["Rows"]["rows"][0][2],text("new"));
        // Explicit NULL is passed through, never silently replaced by DEFAULT.
        assert!(apply(&dir,"items",json!([{"kind":"insert","values":{"id":integer("44"),"name":{"kind":"null"}}}])).await.get("Error").is_some());
    }

    #[tokio::test]
    async fn writer_lock_has_bounded_busy_wait() {
        let dir = fixture().await;
        let mut conn = SqliteConnection::connect_with(&SqliteConnectOptions::new().filename(dir.path().join("test.sqlite"))).await.unwrap();
        let _lock = conn.begin_with("BEGIN IMMEDIATE").await.unwrap();
        let reply = tokio::time::timeout(Duration::from_secs(4), apply(&dir,"items",json!([{"kind":"insert","values":{"id":integer("44")}}]))).await.expect("SQLite busy wait was not bounded");
        assert!(reply.get("Error").is_some());
    }

    #[tokio::test]
    async fn rowid_aliases_must_be_real_and_unshadowed() {
        let dir = fixture().await;
        let mut conn = SqliteConnection::connect_with(&SqliteConnectOptions::new().filename(dir.path().join("test.sqlite"))).await.unwrap();
        sqlx::raw_sql("CREATE TABLE heap (ROWID TEXT, value TEXT); INSERT INTO heap VALUES('shadow','old'); CREATE TABLE descending (rowid TEXT, _rowid_ TEXT, oid TEXT, id INTEGER PRIMARY KEY DESC); INSERT INTO descending VALUES('a','b','c',NULL); CREATE TABLE alias (rowid TEXT, _rowid_ TEXT, oid TEXT, id INTEGER PRIMARY KEY); INSERT INTO alias VALUES('a','b','c',1)").execute(&mut conn).await.unwrap();
        let heap = call(&dir,browse("heap")).await;
        assert_eq!(heap["Rows"]["identities"][0],json!({"_rowid_":integer("1")}));
        assert_eq!(apply(&dir,"heap",json!([{"kind":"update","values":{"value":text("new")},"identity":{"_rowid_":integer("1")},"original":{"ROWID":text("shadow"),"value":text("old")}}])).await,json!({"Applied":{"affected":1}}));
        assert_eq!(call(&dir,browse("descending")).await["Rows"]["editable"],false);
        assert_eq!(call(&dir,browse("alias")).await["Rows"]["identities"][0],json!({"id":integer("1")}));
    }

    #[tokio::test]
    async fn schema_replace_policy_cannot_silently_delete_other_rows() {
        let dir = fixture().await;
        let mut conn = SqliteConnection::connect_with(&SqliteConnectOptions::new().filename(dir.path().join("test.sqlite"))).await.unwrap();
        sqlx::raw_sql("CREATE TABLE replace_policy (id INTEGER PRIMARY KEY ON CONFLICT REPLACE, name TEXT UNIQUE ON CONFLICT REPLACE); INSERT INTO replace_policy VALUES(1,'one'),(2,'two')").execute(&mut conn).await.unwrap();
        assert!(apply(&dir,"replace_policy",json!([{"kind":"insert","values":{"id":integer("1"),"name":text("replacement")}}])).await.get("Error").is_some());
        assert!(apply(&dir,"replace_policy",json!([{"kind":"update","values":{"name":text("two")},"identity":{"id":integer("1")},"original":{"id":integer("1"),"name":text("one")}}])).await.get("Error").is_some());
        let result = call(&dir,browse("replace_policy")).await;
        assert_eq!(result["Rows"]["total"],2);
        assert_eq!(result["Rows"]["rows"][0][1],text("one"));
    }

    #[tokio::test]
    async fn conflicts_detect_storage_class_and_case_changes() {
        let dir = fixture().await;
        let mut conn = SqliteConnection::connect_with(&SqliteConnectOptions::new().filename(dir.path().join("test.sqlite"))).await.unwrap();
        sqlx::raw_sql("CREATE TABLE storage_type (id INTEGER PRIMARY KEY, value, name TEXT COLLATE NOCASE); INSERT INTO storage_type VALUES(1,1,'A'); UPDATE storage_type SET value=1.0, name='a'").execute(&mut conn).await.unwrap();
        for original in [
            json!({"id":integer("1"),"value":integer("1"),"name":text("a")}),
            json!({"id":integer("1"),"value":{"kind":"real","value":1.0},"name":text("A")})
        ] {
            assert!(apply(&dir,"storage_type",json!([{"kind":"delete","values":{},"identity":{"id":integer("1")},"original":original}])).await.get("Error").is_some());
        }
        assert_eq!(call(&dir,browse("storage_type")).await["Rows"]["total"],1);
    }
}