use std::collections::HashMap;
use std::path::{Component, Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Once};
use std::time::{SystemTime, UNIX_EPOCH};

use sqlx::sqlite::{SqliteConnectOptions, SqlitePoolOptions, SqliteRow};
use sqlx::SqlitePool;
use turso::Value as SqlValue;

use super::config::NeoismWorkspace;
use super::notes::{
    BlockEntry, HeadingEntry, LinkEntry, LinkKind, NoteEntry, PropertyEntry,
    WorkspaceNoteIndex,
};

const SCHEMA_VERSION: i64 = 4;

/// Which engine backs the note graph database. Chosen from
/// `NEOISM_NOTES_DB_BACKEND` (`turso` default, `sqlite` opt-out), mirroring
/// the agent server's `NEOISM_AGENT_DB_BACKEND`. Turso is the Rust rewrite
/// of SQLite; it has no FTS5 — see `migrate` for how search degrades there.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum DbBackend {
    Sqlite,
    Turso,
}

pub(crate) fn note_db_backend() -> DbBackend {
    match std::env::var("NEOISM_NOTES_DB_BACKEND") {
        Err(_) => DbBackend::Turso,
        Ok(value) => match value.trim().to_ascii_lowercase().as_str() {
            "" | "turso" => DbBackend::Turso,
            "sqlite" => DbBackend::Sqlite,
            other => {
                // `workspace_graph_db_path` cannot fail, so an unknown value
                // falls back to the default backend with a one-time warning
                // instead of erroring.
                static WARN: Once = Once::new();
                let other = other.to_string();
                WARN.call_once(|| {
                    eprintln!(
                        "[neoism-workspace-index] unknown NEOISM_NOTES_DB_BACKEND value `{other}` (expected \"sqlite\" or \"turso\"); defaulting to turso"
                    );
                });
                DbBackend::Turso
            }
        },
    }
}

/// Each backend gets its own database file: turso must never rewrite the
/// SQLite-managed database in place, so switching backends starts from an
/// empty note graph (it is rebuilt from the Markdown sources on next open).
pub fn workspace_graph_db_path(workspace: &NeoismWorkspace) -> PathBuf {
    let filename = match note_db_backend() {
        DbBackend::Sqlite => "notes.sqlite",
        DbBackend::Turso => "notes.turso.db",
    };
    workspace.cache_dir().join(filename)
}

pub fn rebuild_note_graph(
    workspace: &NeoismWorkspace,
    index: &WorkspaceNoteIndex,
) -> std::io::Result<()> {
    block_on_db(rebuild_note_graph_async(workspace, index))
}

pub fn replace_note_graph_file(
    workspace: &NeoismWorkspace,
    path: impl AsRef<Path>,
) -> std::io::Result<()> {
    block_on_db(replace_note_graph_file_async(workspace, path.as_ref()))
}

pub fn remove_note_graph_file(
    workspace: &NeoismWorkspace,
    path: impl AsRef<Path>,
) -> std::io::Result<()> {
    block_on_db(remove_note_graph_file_async(workspace, path.as_ref()))
}

async fn replace_note_graph_file_async(
    workspace: &NeoismWorkspace,
    path: &Path,
) -> std::io::Result<()> {
    let index = match WorkspaceNoteIndex::build_file(workspace, path)? {
        Some(index) => index,
        None => return remove_note_graph_file_async(workspace, path).await,
    };
    let Some(note) = index.notes.first() else {
        return Ok(());
    };
    let db = open_note_db(&workspace_graph_db_path(workspace)).await?;
    migrate(&db).await?;
    delete_note_rows(&db, &note.relative_path).await?;
    insert_note_index(&db, workspace, &index, now_unix_seconds(), false).await?;
    refresh_link_targets(&db).await?;
    db.close().await;
    Ok(())
}

async fn remove_note_graph_file_async(
    workspace: &NeoismWorkspace,
    path: &Path,
) -> std::io::Result<()> {
    let absolute = workspace.resolve_note_path(path);
    let relative_path = workspace.note_path_label(&absolute);
    let db = open_note_db(&workspace_graph_db_path(workspace)).await?;
    migrate(&db).await?;
    delete_note_rows(&db, &relative_path).await?;
    refresh_link_targets(&db).await?;
    db.close().await;
    Ok(())
}

async fn rebuild_note_graph_async(
    workspace: &NeoismWorkspace,
    index: &WorkspaceNoteIndex,
) -> std::io::Result<()> {
    let db = open_note_db(&workspace_graph_db_path(workspace)).await?;
    migrate(&db).await?;
    if db.fts_enabled() {
        db.execute("DELETE FROM blocks_fts", Vec::new()).await?;
    }
    for sql in [
        "DELETE FROM links",
        "DELETE FROM tags",
        "DELETE FROM tasks",
        "DELETE FROM note_properties",
        "DELETE FROM headings",
        "DELETE FROM blocks",
        "DELETE FROM notes",
    ] {
        db.execute(sql, Vec::new()).await?;
    }
    insert_note_index(&db, workspace, index, now_unix_seconds(), true).await?;
    db.close().await;
    Ok(())
}

async fn insert_note_index(
    db: &NoteDb,
    workspace: &NeoismWorkspace,
    index: &WorkspaceNoteIndex,
    indexed_at: i64,
    resolve_links_from_index: bool,
) -> std::io::Result<()> {
    let note_ids = note_ids(&workspace.config.id, &index.notes);
    let block_ids = block_ids(&workspace.config.id, &note_ids, &index.blocks);
    let source_block_by_line = source_block_by_line(&block_ids, &index.blocks);
    let heading_target_ids = heading_target_ids(
        &workspace.config.id,
        &note_ids,
        &source_block_by_line,
        &index.headings,
    );

    for note in &index.notes {
        let Some(note_id) = note_ids.get(&note.relative_path) else {
            continue;
        };
        db.execute(
            r#"
            INSERT INTO notes (
                id, workspace_id, path, title, modified, content_hash, indexed_at
            ) VALUES (?, ?, ?, ?, ?, ?, ?)
            "#,
            vec![
                text(note_id.as_str()),
                text(workspace.config.id.as_str()),
                text(note.relative_path.as_str()),
                text(note.title.as_str()),
                int(note.modified),
                text(stable_hash_hex(&format!(
                    "note-content:{}:{}",
                    note.relative_path, note.hash
                ))),
                int(indexed_at),
            ],
        )
        .await?;
    }

    for property in &index.properties {
        let Some(note_id) = note_ids.get(&property.note_path) else {
            continue;
        };
        insert_note_property(db, &workspace.config.id, note_id, property).await?;
    }

    for block in &index.blocks {
        let Some(note_id) = note_ids.get(&block.note_path) else {
            continue;
        };
        let Some(block_id) = block_ids.get(&block_key(block)) else {
            continue;
        };
        db.execute(
            r#"
            INSERT INTO blocks (
                id, note_id, path, kind, start_line, end_line, ordinal, anchor, text, text_hash
            ) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?)
            "#,
            vec![
                text(block_id.as_str()),
                text(note_id.as_str()),
                text(block.note_path.as_str()),
                text(block.kind.as_str()),
                int(block.start_line as i64),
                int(block.end_line as i64),
                int(block.ordinal as i64),
                opt_text(block.anchor.clone()),
                text(block.text.as_str()),
                text(stable_hash_hex(&block.text)),
            ],
        )
        .await?;
        insert_block_identity(db, block_id, block, indexed_at).await?;
        if db.fts_enabled() {
            db.execute(
                r#"
                INSERT INTO blocks_fts (
                    block_id, note_id, path, kind, text
                ) VALUES (?, ?, ?, ?, ?)
                "#,
                vec![
                    text(block_id.as_str()),
                    text(note_id.as_str()),
                    text(block.note_path.as_str()),
                    text(block.kind.as_str()),
                    text(block.text.as_str()),
                ],
            )
            .await?;
        }
    }

    for heading in &index.headings {
        let Some(note_id) = note_ids.get(&heading.note_path) else {
            continue;
        };
        let Some(block_id) =
            source_block_by_line.get(&(heading.note_path.clone(), heading.line))
        else {
            continue;
        };
        let heading_id = heading_entity_id(&workspace.config.id, block_id, heading);
        db.execute(
            r#"
            INSERT INTO headings (
                id, block_id, note_id, path, line, level, text, slug
            ) VALUES (?, ?, ?, ?, ?, ?, ?, ?)
            "#,
            vec![
                text(heading_id),
                text(block_id.as_str()),
                text(note_id.as_str()),
                text(heading.note_path.as_str()),
                int(heading.line as i64),
                int(heading.level as i64),
                text(heading.text.as_str()),
                text(heading.slug.as_str()),
            ],
        )
        .await?;
    }

    for link in &index.links {
        let Some(source_note_id) = note_ids.get(&link.source_path) else {
            continue;
        };
        let source_block_id = source_block_by_line
            .get(&(link.source_path.clone(), link.source_line))
            .cloned();
        let target_note = resolve_links_from_index
            .then(|| resolve_target_note(index, link))
            .flatten();
        let target_note_id = target_note
            .and_then(|note| note_ids.get(&note.relative_path))
            .cloned();
        let target_heading_id = target_note
            .and_then(|note| link.heading.as_deref().map(|heading| (note, heading)))
            .and_then(|(note, heading)| {
                let slug = heading_slug(heading);
                heading_target_ids
                    .get(&heading_key(&note.relative_path, &slug))
                    .cloned()
            });
        db.execute(
            r#"
            INSERT INTO links (
                id, source_block_id, source_note_id, source_path, source_line, raw,
                target, target_note_id, heading, target_heading_id, alias, kind
            ) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)
            "#,
            vec![
                text(stable_entity_id(
                    "link",
                    &workspace.config.id,
                    &format!(
                        "{}:{}:{}:{}",
                        link.source_path, link.source_line, link.raw, link.target
                    ),
                )),
                opt_text(source_block_id),
                text(source_note_id.as_str()),
                text(link.source_path.as_str()),
                int(link.source_line as i64),
                text(link.raw.as_str()),
                text(link.target.as_str()),
                opt_text(target_note_id),
                opt_text(link.heading.clone()),
                opt_text(target_heading_id),
                opt_text(link.alias.clone()),
                text(link_kind_name(&link.kind)),
            ],
        )
        .await?;
    }

    for tag in &index.tags {
        let Some(note_id) = note_ids.get(&tag.note_path) else {
            continue;
        };
        let block_id = source_block_by_line
            .get(&(tag.note_path.clone(), tag.line))
            .cloned();
        db.execute(
            r#"
            INSERT INTO tags (
                id, block_id, note_id, path, line, tag
            ) VALUES (?, ?, ?, ?, ?, ?)
            "#,
            vec![
                text(stable_entity_id(
                    "tag",
                    &workspace.config.id,
                    &format!("{}:{}:{}", tag.note_path, tag.line, tag.tag),
                )),
                opt_text(block_id),
                text(note_id.as_str()),
                text(tag.note_path.as_str()),
                int(tag.line as i64),
                text(tag.tag.as_str()),
            ],
        )
        .await?;
    }

    for task in &index.tasks {
        let Some(note_id) = note_ids.get(&task.note_path) else {
            continue;
        };
        let block_id = source_block_by_line
            .get(&(task.note_path.clone(), task.line))
            .cloned();
        db.execute(
            r#"
            INSERT INTO tasks (
                id, block_id, note_id, path, line, checked, text
            ) VALUES (?, ?, ?, ?, ?, ?, ?)
            "#,
            vec![
                text(stable_entity_id(
                    "task",
                    &workspace.config.id,
                    &format!("{}:{}:{}", task.note_path, task.line, task.text),
                )),
                opt_text(block_id),
                text(note_id.as_str()),
                text(task.note_path.as_str()),
                int(task.line as i64),
                int(if task.checked { 1 } else { 0 }),
                text(task.text.as_str()),
            ],
        )
        .await?;
    }

    Ok(())
}

async fn insert_note_property(
    db: &NoteDb,
    workspace_id: &str,
    note_id: &str,
    property: &PropertyEntry,
) -> std::io::Result<()> {
    db.execute(
        r#"
        INSERT INTO note_properties (
            id, note_id, path, key, value, value_type
        ) VALUES (?, ?, ?, ?, ?, ?)
        "#,
        vec![
            text(stable_entity_id(
                "property",
                workspace_id,
                &format!("{}:{}", property.note_path, property.key),
            )),
            text(note_id),
            text(property.note_path.as_str()),
            text(property.key.as_str()),
            text(property.value.as_str()),
            text(property.value_type.as_str()),
        ],
    )
    .await
}

async fn insert_block_identity(
    db: &NoteDb,
    block_id: &str,
    block: &BlockEntry,
    updated_at: i64,
) -> std::io::Result<()> {
    db.execute(
        r#"
        INSERT INTO block_identity (
            id, path, kind, ordinal, anchor, text_hash, updated_at
        ) VALUES (?, ?, ?, ?, ?, ?, ?)
        ON CONFLICT(id) DO UPDATE SET
            path = excluded.path,
            kind = excluded.kind,
            ordinal = excluded.ordinal,
            anchor = excluded.anchor,
            text_hash = excluded.text_hash,
            updated_at = excluded.updated_at
        "#,
        vec![
            text(block_id),
            text(block.note_path.as_str()),
            text(block.kind.as_str()),
            int(block.ordinal as i64),
            opt_text(block.anchor.clone()),
            text(stable_hash_hex(&block.text)),
            int(updated_at),
        ],
    )
    .await
}

async fn delete_note_rows(db: &NoteDb, relative_path: &str) -> std::io::Result<()> {
    if db.fts_enabled() {
        db.execute(
            "DELETE FROM blocks_fts WHERE path = ?",
            vec![text(relative_path)],
        )
        .await?;
    }
    // Delete child rows explicitly instead of leaning on ON DELETE CASCADE:
    // the sqlx backend enforces foreign keys, but turso's enforcement is not
    // guaranteed, and the explicit form keeps both backends identical.
    // (`block_identity` intentionally survives deletes — it has no FK and
    // carries stable identity across rebuilds. Dangling `target_note_id`
    // references are re-resolved by `refresh_link_targets`.)
    for sql in [
        "DELETE FROM headings WHERE path = ?",
        "DELETE FROM blocks WHERE path = ?",
        "DELETE FROM note_properties WHERE path = ?",
        "DELETE FROM tags WHERE path = ?",
        "DELETE FROM tasks WHERE path = ?",
        "DELETE FROM links WHERE source_path = ?",
        "DELETE FROM notes WHERE path = ?",
    ] {
        db.execute(sql, vec![text(relative_path)]).await?;
    }
    Ok(())
}

#[derive(Debug, Clone)]
struct DbNote {
    id: String,
    path: String,
    title: String,
}

#[derive(Debug, Clone)]
struct DbHeading {
    id: String,
    note_id: String,
    path: String,
    slug: String,
}

async fn refresh_link_targets(db: &NoteDb) -> std::io::Result<()> {
    let note_rows = db
        .fetch_all("SELECT id, path, title FROM notes", Vec::new())
        .await?;
    let notes = note_rows
        .iter()
        .map(|row| {
            Ok(DbNote {
                id: row.get_str("id")?,
                path: row.get_str("path")?,
                title: row.get_str("title")?,
            })
        })
        .collect::<std::io::Result<Vec<_>>>()?;
    let heading_rows = db
        .fetch_all("SELECT id, note_id, path, slug FROM headings", Vec::new())
        .await?;
    let headings = heading_rows
        .iter()
        .map(|row| {
            Ok(DbHeading {
                id: row.get_str("id")?,
                note_id: row.get_str("note_id")?,
                path: row.get_str("path")?,
                slug: row.get_str("slug")?,
            })
        })
        .collect::<std::io::Result<Vec<_>>>()?;
    let link_rows = db
        .fetch_all(
            "SELECT id, source_path, target, heading, kind FROM links ORDER BY source_path, source_line",
            Vec::new(),
        )
        .await?;

    for row in link_rows {
        let id = row.get_str("id")?;
        let source_path = row.get_str("source_path")?;
        let target = row.get_str("target")?;
        let heading = row.get_opt_str("heading")?;
        let kind = row.get_str("kind")?;
        let target_note = resolve_db_target_note(&notes, &source_path, &target, &kind);
        let target_note_id = target_note.map(|note| note.id.clone());
        let target_heading_id = target_note
            .and_then(|note| {
                heading.as_deref().map(heading_slug).and_then(|slug| {
                    resolve_db_heading(&headings, &note.id, &note.path, &slug)
                })
            })
            .map(|heading| heading.id.clone());
        db.execute(
            "UPDATE links SET target_note_id = ?, target_heading_id = ? WHERE id = ?",
            vec![
                opt_text(target_note_id),
                opt_text(target_heading_id),
                text(id),
            ],
        )
        .await?;
    }
    Ok(())
}

/// The two storage engines behind one set of SQL, mirroring the agent
/// server's `state.rs`. Every statement must stay inside the dialect both
/// engines support; the FTS5 virtual table is the one exception, handled
/// explicitly via the `fts_enabled` gate.
#[derive(Clone)]
enum Db {
    Sqlite(SqlitePool),
    Turso(turso::Database),
}

#[derive(Clone)]
pub(crate) struct NoteDb {
    db: Db,
    /// FTS5 only exists in the SQLite backend. When unavailable, FTS writes
    /// no-op and `search()` uses the LIKE fallback directly.
    fts_enabled: Arc<AtomicBool>,
}

/// An engine-agnostic row: column names plus SQLite-typed values.
pub(crate) struct DbRow {
    columns: Arc<Vec<String>>,
    values: Vec<SqlValue>,
}

impl DbRow {
    fn value(&self, name: &str) -> std::io::Result<&SqlValue> {
        let index = self
            .columns
            .iter()
            .position(|column| column == name)
            .ok_or_else(|| io_other(format!("row has no column named `{name}`")))?;
        self.values
            .get(index)
            .ok_or_else(|| io_other(format!("row has no value for column `{name}`")))
    }

    pub(crate) fn get_str(&self, name: &str) -> std::io::Result<String> {
        match self.value(name)? {
            SqlValue::Text(value) => Ok(value.clone()),
            other => Err(io_other(format!("column `{name}` is not text: {other:?}"))),
        }
    }

    pub(crate) fn get_opt_str(&self, name: &str) -> std::io::Result<Option<String>> {
        match self.value(name)? {
            SqlValue::Text(value) => Ok(Some(value.clone())),
            SqlValue::Null => Ok(None),
            other => Err(io_other(format!(
                "column `{name}` is not text or null: {other:?}"
            ))),
        }
    }

    pub(crate) fn get_i64(&self, name: &str) -> std::io::Result<i64> {
        match self.value(name)? {
            SqlValue::Integer(value) => Ok(*value),
            other => Err(io_other(format!(
                "column `{name}` is not an integer: {other:?}"
            ))),
        }
    }

    fn i64_at(&self, index: usize) -> std::io::Result<i64> {
        match self.values.get(index) {
            Some(SqlValue::Integer(value)) => Ok(*value),
            Some(other) => Err(io_other(format!(
                "column {index} is not an integer: {other:?}"
            ))),
            None => Err(io_other(format!("row has no column {index}"))),
        }
    }
}

pub(crate) fn text(value: impl Into<String>) -> SqlValue {
    SqlValue::Text(value.into())
}

pub(crate) fn opt_text(value: Option<String>) -> SqlValue {
    value.map(SqlValue::Text).unwrap_or(SqlValue::Null)
}

pub(crate) fn int(value: i64) -> SqlValue {
    SqlValue::Integer(value)
}

type SqliteQuery<'q> =
    sqlx::query::Query<'q, sqlx::Sqlite, sqlx::sqlite::SqliteArguments<'q>>;

fn bind_value(query: SqliteQuery<'_>, value: SqlValue) -> SqliteQuery<'_> {
    match value {
        SqlValue::Null => query.bind(None::<String>),
        SqlValue::Integer(value) => query.bind(value),
        SqlValue::Real(value) => query.bind(value),
        SqlValue::Text(value) => query.bind(value),
        SqlValue::Blob(value) => query.bind(value),
    }
}

fn sqlite_row_to_db_row(row: &SqliteRow) -> std::io::Result<DbRow> {
    use sqlx::{Column as _, Row as _, TypeInfo as _, ValueRef as _};
    let mut columns = Vec::with_capacity(row.len());
    let mut values = Vec::with_capacity(row.len());
    for (index, column) in row.columns().iter().enumerate() {
        columns.push(column.name().to_string());
        let raw = row.try_get_raw(index).map_err(io_other)?;
        let value = if raw.is_null() {
            SqlValue::Null
        } else {
            // Decode by the value's actual storage class, not the column's
            // declared type: expression columns (COALESCE, snippet, …) only
            // carry the former.
            match raw.type_info().name() {
                "INTEGER" => SqlValue::Integer(row.try_get(index).map_err(io_other)?),
                "REAL" => SqlValue::Real(row.try_get(index).map_err(io_other)?),
                "BLOB" => SqlValue::Blob(row.try_get(index).map_err(io_other)?),
                _ => SqlValue::Text(row.try_get(index).map_err(io_other)?),
            }
        };
        values.push(value);
    }
    Ok(DbRow {
        columns: Arc::new(columns),
        values,
    })
}

/// Turso returns `Busy` immediately where the sqlx SQLite pool would wait
/// (its connections carry a busy_timeout), so concurrent writers — e.g. a
/// watcher-driven replace racing a full reindex — fail outright. Emulate
/// busy_timeout with a bounded retry so writers queue instead.
async fn turso_busy_retry<T, F, Fut>(mut op: F) -> Result<T, turso::Error>
where
    F: FnMut() -> Fut,
    Fut: std::future::Future<Output = Result<T, turso::Error>>,
{
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
    let mut delay = std::time::Duration::from_millis(2);
    loop {
        match op().await {
            Err(error)
                if turso_error_is_busy(&error)
                    && std::time::Instant::now() < deadline =>
            {
                tokio::time::sleep(delay).await;
                delay = (delay * 2).min(std::time::Duration::from_millis(100));
            }
            other => return other,
        }
    }
}

fn turso_error_is_busy(error: &turso::Error) -> bool {
    matches!(error, turso::Error::Busy(_) | turso::Error::BusySnapshot(_)) || {
        let message = error.to_string().to_ascii_lowercase();
        message.contains("locked") || message.contains("busy")
    }
}

impl NoteDb {
    fn backend(&self) -> DbBackend {
        match &self.db {
            Db::Sqlite(_) => DbBackend::Sqlite,
            Db::Turso(_) => DbBackend::Turso,
        }
    }

    pub(crate) fn fts_enabled(&self) -> bool {
        self.fts_enabled.load(Ordering::Relaxed)
    }

    fn set_fts_enabled(&self, enabled: bool) {
        self.fts_enabled.store(enabled, Ordering::Relaxed);
    }

    pub(crate) async fn execute(
        &self,
        sql: &str,
        params: Vec<SqlValue>,
    ) -> std::io::Result<()> {
        match &self.db {
            Db::Sqlite(pool) => {
                let mut query = sqlx::query(sql);
                for param in params {
                    query = bind_value(query, param);
                }
                query.execute(pool).await.map_err(io_other)?;
                Ok(())
            }
            Db::Turso(db) => {
                turso_busy_retry(|| {
                    let params = params.clone();
                    async move {
                        let conn = db.connect()?;
                        conn.execute(sql, params).await
                    }
                })
                .await
                .map_err(io_other)?;
                Ok(())
            }
        }
    }

    pub(crate) async fn fetch_all(
        &self,
        sql: &str,
        params: Vec<SqlValue>,
    ) -> std::io::Result<Vec<DbRow>> {
        match &self.db {
            Db::Sqlite(pool) => {
                let mut query = sqlx::query(sql);
                for param in params {
                    query = bind_value(query, param);
                }
                let rows = query.fetch_all(pool).await.map_err(io_other)?;
                rows.iter().map(sqlite_row_to_db_row).collect()
            }
            Db::Turso(db) => turso_busy_retry(|| {
                let params = params.clone();
                async move {
                    let conn = db.connect()?;
                    let mut rows = conn.query(sql, params).await?;
                    let columns = Arc::new(rows.column_names());
                    let mut out = Vec::new();
                    while let Some(row) = rows.next().await? {
                        let values = (0..row.column_count())
                            .map(|index| row.get_value(index))
                            .collect::<Result<Vec<_>, _>>()?;
                        out.push(DbRow {
                            columns: columns.clone(),
                            values,
                        });
                    }
                    Ok(out)
                }
            })
            .await
            .map_err(io_other),
        }
    }

    pub(crate) async fn fetch_scalar_i64(
        &self,
        sql: &str,
        params: Vec<SqlValue>,
    ) -> std::io::Result<i64> {
        self.fetch_all(sql, params)
            .await?
            .into_iter()
            .next()
            .ok_or_else(|| io_other("query returned no rows"))?
            .i64_at(0)
    }

    pub(crate) async fn close(&self) {
        if let Db::Sqlite(pool) = &self.db {
            pool.close().await;
        }
    }
}

pub(crate) async fn open_note_db(path: &Path) -> std::io::Result<NoteDb> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let db = match note_db_backend() {
        DbBackend::Sqlite => {
            let options = SqliteConnectOptions::new()
                .filename(path)
                .create_if_missing(true)
                .foreign_keys(true)
                .pragma("journal_mode", "WAL")
                .pragma("synchronous", "NORMAL");
            let pool = SqlitePoolOptions::new()
                .max_connections(1)
                .connect_with(options)
                .await
                .map_err(io_other)?;
            Db::Sqlite(pool)
        }
        DbBackend::Turso => {
            let path = path
                .to_str()
                .ok_or_else(|| io_other("turso database path is not valid UTF-8"))?;
            let database = turso::Builder::new_local(path)
                .build()
                .await
                .map_err(io_other)?;
            Db::Turso(database)
        }
    };
    Ok(NoteDb {
        db,
        fts_enabled: Arc::new(AtomicBool::new(false)),
    })
}

pub(crate) async fn migrate(db: &NoteDb) -> std::io::Result<()> {
    for query in SCHEMA {
        db.execute(query, Vec::new()).await?;
    }
    // FTS5 ships in the bundled SQLite; turso has no FTS5 module, which
    // is the DESIGNED state of the default backend — search runs on the
    // tokenized LIKE scan there (query.rs), not a degraded mode worth
    // warning about. Hard-fail only on the SQLite backend, where FTS5
    // going missing would be a real build regression.
    match db.execute(FTS_SCHEMA, Vec::new()).await {
        Ok(()) => db.set_fts_enabled(true),
        Err(error) => match db.backend() {
            DbBackend::Sqlite => return Err(error),
            DbBackend::Turso => db.set_fts_enabled(false),
        },
    }
    apply_schema_upgrades(db).await?;
    let current = db
        .fetch_scalar_i64(
            "SELECT COALESCE(MAX(version), 0) FROM schema_migrations",
            Vec::new(),
        )
        .await?;
    if current < SCHEMA_VERSION {
        db.execute(
            "INSERT OR IGNORE INTO schema_migrations (version, applied_at) VALUES (?, ?)",
            vec![int(SCHEMA_VERSION), int(now_unix_seconds())],
        )
        .await?;
    }
    Ok(())
}

async fn apply_schema_upgrades(db: &NoteDb) -> std::io::Result<()> {
    let _ = db
        .execute("ALTER TABLE blocks ADD COLUMN anchor TEXT", Vec::new())
        .await;
    Ok(())
}

const SCHEMA: &[&str] = &[
    r#"
    CREATE TABLE IF NOT EXISTS schema_migrations (
        version INTEGER PRIMARY KEY,
        applied_at INTEGER NOT NULL
    )
    "#,
    r#"
    CREATE TABLE IF NOT EXISTS notes (
        id TEXT PRIMARY KEY,
        workspace_id TEXT NOT NULL,
        path TEXT NOT NULL UNIQUE,
        title TEXT NOT NULL,
        modified INTEGER NOT NULL,
        content_hash TEXT NOT NULL,
        indexed_at INTEGER NOT NULL
    )
    "#,
    r#"
    CREATE TABLE IF NOT EXISTS blocks (
        id TEXT PRIMARY KEY,
        note_id TEXT NOT NULL,
        path TEXT NOT NULL,
        kind TEXT NOT NULL,
        start_line INTEGER NOT NULL,
        end_line INTEGER NOT NULL,
        ordinal INTEGER NOT NULL,
        anchor TEXT,
        text TEXT NOT NULL,
        text_hash TEXT NOT NULL,
        FOREIGN KEY(note_id) REFERENCES notes(id) ON DELETE CASCADE
    )
    "#,
    r#"
    CREATE TABLE IF NOT EXISTS headings (
        id TEXT PRIMARY KEY,
        block_id TEXT NOT NULL,
        note_id TEXT NOT NULL,
        path TEXT NOT NULL,
        line INTEGER NOT NULL,
        level INTEGER NOT NULL,
        text TEXT NOT NULL,
        slug TEXT NOT NULL,
        FOREIGN KEY(block_id) REFERENCES blocks(id) ON DELETE CASCADE,
        FOREIGN KEY(note_id) REFERENCES notes(id) ON DELETE CASCADE
    )
    "#,
    r#"
    CREATE TABLE IF NOT EXISTS block_identity (
        id TEXT PRIMARY KEY,
        path TEXT NOT NULL,
        kind TEXT NOT NULL,
        ordinal INTEGER NOT NULL,
        anchor TEXT,
        text_hash TEXT NOT NULL,
        updated_at INTEGER NOT NULL
    )
    "#,
    r#"
    CREATE TABLE IF NOT EXISTS note_properties (
        id TEXT PRIMARY KEY,
        note_id TEXT NOT NULL,
        path TEXT NOT NULL,
        key TEXT NOT NULL,
        value TEXT NOT NULL,
        value_type TEXT NOT NULL,
        FOREIGN KEY(note_id) REFERENCES notes(id) ON DELETE CASCADE
    )
    "#,
    r#"
    CREATE TABLE IF NOT EXISTS links (
        id TEXT PRIMARY KEY,
        source_block_id TEXT,
        source_note_id TEXT NOT NULL,
        source_path TEXT NOT NULL,
        source_line INTEGER NOT NULL,
        raw TEXT NOT NULL,
        target TEXT NOT NULL,
        target_note_id TEXT,
        heading TEXT,
        target_heading_id TEXT,
        alias TEXT,
        kind TEXT NOT NULL,
        FOREIGN KEY(source_block_id) REFERENCES blocks(id) ON DELETE SET NULL,
        FOREIGN KEY(source_note_id) REFERENCES notes(id) ON DELETE CASCADE,
        FOREIGN KEY(target_note_id) REFERENCES notes(id) ON DELETE SET NULL,
        FOREIGN KEY(target_heading_id) REFERENCES headings(id) ON DELETE SET NULL
    )
    "#,
    r#"
    CREATE TABLE IF NOT EXISTS tags (
        id TEXT PRIMARY KEY,
        block_id TEXT,
        note_id TEXT NOT NULL,
        path TEXT NOT NULL,
        line INTEGER NOT NULL,
        tag TEXT NOT NULL,
        FOREIGN KEY(block_id) REFERENCES blocks(id) ON DELETE SET NULL,
        FOREIGN KEY(note_id) REFERENCES notes(id) ON DELETE CASCADE
    )
    "#,
    r#"
    CREATE TABLE IF NOT EXISTS tasks (
        id TEXT PRIMARY KEY,
        block_id TEXT,
        note_id TEXT NOT NULL,
        path TEXT NOT NULL,
        line INTEGER NOT NULL,
        checked INTEGER NOT NULL,
        text TEXT NOT NULL,
        FOREIGN KEY(block_id) REFERENCES blocks(id) ON DELETE SET NULL,
        FOREIGN KEY(note_id) REFERENCES notes(id) ON DELETE CASCADE
    )
    "#,
    "CREATE INDEX IF NOT EXISTS idx_blocks_note_line ON blocks(note_id, start_line, end_line)",
    "CREATE INDEX IF NOT EXISTS idx_headings_note_slug ON headings(note_id, slug)",
    "CREATE INDEX IF NOT EXISTS idx_links_source_note ON links(source_note_id)",
    "CREATE INDEX IF NOT EXISTS idx_links_target_note ON links(target_note_id)",
    "CREATE INDEX IF NOT EXISTS idx_links_unresolved ON links(target_note_id, kind)",
    "CREATE INDEX IF NOT EXISTS idx_tags_tag ON tags(tag)",
    "CREATE INDEX IF NOT EXISTS idx_tasks_checked ON tasks(checked)",
    "CREATE INDEX IF NOT EXISTS idx_block_identity_path ON block_identity(path, ordinal)",
    "CREATE INDEX IF NOT EXISTS idx_block_identity_anchor ON block_identity(path, anchor)",
    "CREATE INDEX IF NOT EXISTS idx_note_properties_key ON note_properties(key)",
];

/// FTS5 virtual table, kept out of `SCHEMA` because only the SQLite backend
/// supports it. Maintained manually (no triggers).
const FTS_SCHEMA: &str = r#"
    CREATE VIRTUAL TABLE IF NOT EXISTS blocks_fts USING fts5(
        block_id UNINDEXED,
        note_id UNINDEXED,
        path UNINDEXED,
        kind UNINDEXED,
        text,
        tokenize = 'unicode61'
    )
"#;

fn note_ids(workspace_id: &str, notes: &[NoteEntry]) -> HashMap<String, String> {
    notes
        .iter()
        .map(|note| {
            (
                note.relative_path.clone(),
                stable_entity_id("note", workspace_id, &note.relative_path),
            )
        })
        .collect()
}

fn block_ids(
    workspace_id: &str,
    note_ids: &HashMap<String, String>,
    blocks: &[BlockEntry],
) -> HashMap<String, String> {
    blocks
        .iter()
        .filter_map(|block| {
            let note_id = note_ids.get(&block.note_path)?;
            let key = block_key(block);
            Some((
                key.clone(),
                stable_entity_id(
                    "block",
                    workspace_id,
                    &block_identity_key(note_id, block),
                ),
            ))
        })
        .collect()
}

fn block_identity_key(note_id: &str, block: &BlockEntry) -> String {
    if let Some(anchor) = &block.anchor {
        format!("{note_id}:anchor:{anchor}")
    } else {
        format!("{note_id}:{}:{}", block.kind.as_str(), block.ordinal)
    }
}

fn heading_target_ids(
    workspace_id: &str,
    note_ids: &HashMap<String, String>,
    source_block_by_line: &HashMap<(String, usize), String>,
    headings: &[HeadingEntry],
) -> HashMap<String, String> {
    let mut out = HashMap::new();
    for heading in headings {
        if !note_ids.contains_key(&heading.note_path) {
            continue;
        }
        let Some(block_id) =
            source_block_by_line.get(&(heading.note_path.clone(), heading.line))
        else {
            continue;
        };
        let key = heading_key(&heading.note_path, &heading.slug);
        out.entry(key)
            .or_insert_with(|| heading_entity_id(workspace_id, block_id, heading));
    }
    out
}

fn heading_entity_id(
    workspace_id: &str,
    block_id: &str,
    heading: &HeadingEntry,
) -> String {
    stable_entity_id(
        "heading",
        workspace_id,
        &format!("{block_id}:{}", heading.slug),
    )
}

fn source_block_by_line(
    block_ids: &HashMap<String, String>,
    blocks: &[BlockEntry],
) -> HashMap<(String, usize), String> {
    let mut out = HashMap::new();
    for block in blocks {
        let Some(block_id) = block_ids.get(&block_key(block)) else {
            continue;
        };
        for line in block.start_line..=block.end_line {
            out.insert((block.note_path.clone(), line), block_id.clone());
        }
    }
    out
}

fn block_key(block: &BlockEntry) -> String {
    format!(
        "{}:{}:{}:{}",
        block.note_path, block.ordinal, block.start_line, block.end_line
    )
}

fn heading_key(note_path: &str, slug: &str) -> String {
    format!("{note_path}#{slug}")
}

fn resolve_target_note<'a>(
    index: &'a WorkspaceNoteIndex,
    link: &LinkEntry,
) -> Option<&'a NoteEntry> {
    if matches!(link.kind, LinkKind::CodeRef) {
        return None;
    }
    let source_dir = Path::new(&link.source_path)
        .parent()
        .unwrap_or_else(|| Path::new(""));
    let target = Path::new(&link.target);
    let mut candidates = Vec::new();
    if target.is_absolute() {
        candidates.push(relative_components(target));
    } else {
        candidates.push(normalize_relative_components(&source_dir.join(target)));
        candidates.push(relative_components(target));
    }
    if target.extension().is_none() {
        let base_candidates = candidates.clone();
        for candidate in base_candidates {
            for ext in ["md", "markdown", "mdx"] {
                candidates.push(format!("{candidate}.{ext}"));
            }
        }
    }
    index.notes.iter().find(|note| {
        candidates.iter().any(|candidate| {
            candidate == &note.relative_path
                || strip_markdown_extension(candidate)
                    == strip_markdown_extension(&note.relative_path)
        }) || note.title.eq_ignore_ascii_case(&link.target)
    })
}

fn resolve_db_target_note<'a>(
    notes: &'a [DbNote],
    source_path: &str,
    target: &str,
    kind: &str,
) -> Option<&'a DbNote> {
    if kind == "code_ref" {
        return None;
    }
    let source_dir = Path::new(source_path)
        .parent()
        .unwrap_or_else(|| Path::new(""));
    let target_path = Path::new(target);
    let mut candidates = Vec::new();
    if target_path.is_absolute() {
        candidates.push(relative_components(target_path));
    } else {
        candidates.push(normalize_relative_components(&source_dir.join(target_path)));
        candidates.push(relative_components(target_path));
    }
    if target_path.extension().is_none() {
        let base_candidates = candidates.clone();
        for candidate in base_candidates {
            for ext in ["md", "markdown", "mdx"] {
                candidates.push(format!("{candidate}.{ext}"));
            }
        }
    }
    notes.iter().find(|note| {
        candidates.iter().any(|candidate| {
            candidate == &note.path
                || strip_markdown_extension(candidate)
                    == strip_markdown_extension(&note.path)
        }) || note.title.eq_ignore_ascii_case(target)
    })
}

fn resolve_db_heading<'a>(
    headings: &'a [DbHeading],
    note_id: &str,
    note_path: &str,
    slug: &str,
) -> Option<&'a DbHeading> {
    headings
        .iter()
        .find(|heading| {
            heading.note_id == note_id
                && heading.path == note_path
                && heading.slug == slug
        })
        .or_else(|| {
            headings
                .iter()
                .find(|heading| heading.note_id == note_id && heading.slug == slug)
        })
}

fn link_kind_name(kind: &LinkKind) -> &'static str {
    match kind {
        LinkKind::Note => "note",
        LinkKind::CodeRef => "code_ref",
        LinkKind::Embed => "embed",
    }
}

fn heading_slug(text: &str) -> String {
    let mut slug = String::new();
    let mut last_dash = false;
    for ch in text.chars() {
        if ch.is_ascii_alphanumeric() {
            slug.push(ch.to_ascii_lowercase());
            last_dash = false;
        } else if !last_dash {
            slug.push('-');
            last_dash = true;
        }
    }
    slug.trim_matches('-').to_string()
}

fn stable_entity_id(kind: &str, workspace_id: &str, value: &str) -> String {
    format!(
        "{kind}:{}",
        stable_hash_hex(&format!("{workspace_id}:{value}"))
    )
}

fn stable_hash_hex(value: &str) -> String {
    let mut hash = 0xcbf29ce484222325_u64;
    for byte in value.as_bytes() {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(0x100000001b3);
    }
    format!("{hash:016x}")
}

fn relative_components(path: &Path) -> String {
    path.components()
        .filter_map(|component| match component {
            Component::Normal(part) => Some(part.to_string_lossy().into_owned()),
            Component::ParentDir => Some("..".to_string()),
            _ => None,
        })
        .collect::<Vec<_>>()
        .join("/")
}

fn normalize_relative_components(path: &Path) -> String {
    let mut parts = Vec::new();
    for component in path.components() {
        match component {
            Component::Normal(part) => parts.push(part.to_string_lossy().into_owned()),
            Component::ParentDir => {
                parts.pop();
            }
            _ => {}
        }
    }
    parts.join("/")
}

fn strip_markdown_extension(path: &str) -> &str {
    for suffix in [".markdown", ".mdx", ".md"] {
        if let Some(stripped) = path.strip_suffix(suffix) {
            return stripped;
        }
    }
    path
}

fn now_unix_seconds() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_secs() as i64)
        .unwrap_or(0)
}

pub(crate) fn block_on_db<F, T>(future: F) -> std::io::Result<T>
where
    F: std::future::Future<Output = std::io::Result<T>> + Send,
    T: Send,
{
    if tokio::runtime::Handle::try_current().is_ok() {
        return std::thread::scope(|scope| {
            scope
                .spawn(move || block_on_db_runtime(future))
                .join()
                .map_err(|_| {
                    std::io::Error::other("note graph runtime thread panicked")
                })?
        });
    }
    block_on_db_runtime(future)
}

fn block_on_db_runtime<F, T>(future: F) -> std::io::Result<T>
where
    F: std::future::Future<Output = std::io::Result<T>>,
{
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .map_err(io_other)?
        .block_on(future)
}

pub(crate) fn io_other(err: impl std::fmt::Display) -> std::io::Error {
    std::io::Error::other(err.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{NeoismWorkspace, NotesConfig, WorkspaceConfig};

    fn test_workspace(name: &str) -> NeoismWorkspace {
        let root = std::env::temp_dir()
            .join(format!("neoism-graph-db-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(root.join(".neoism/cache")).unwrap();
        let notes_root = root.join("Neoism/Vaults/Personal");
        NeoismWorkspace {
            root,
            config: WorkspaceConfig {
                version: 1,
                id: format!("test-{name}"),
                name: name.to_string(),
                notes: NotesConfig {
                    enabled: true,
                    workspace: notes_root.display().to_string(),
                    ignore: Vec::new(),
                },
            },
        }
    }

    async fn count(db: &NoteDb, sql: &str) -> std::io::Result<i64> {
        db.fetch_scalar_i64(sql, Vec::new()).await
    }

    #[test]
    fn rebuild_graph_persists_notes_blocks_links_tags_and_tasks() {
        let workspace = test_workspace("persist");
        std::fs::create_dir_all(workspace.notes_workspace_dir()).unwrap();
        std::fs::write(
            workspace.notes_workspace_dir().join("Roadmap.md"),
            "# Roadmap\n\n- [ ] ship #neoism\n\nSee [[Roadmap#Roadmap]]\n",
        )
        .unwrap();
        let index = WorkspaceNoteIndex::build(&workspace).unwrap();

        rebuild_note_graph(&workspace, &index).unwrap();
        let db_path = workspace_graph_db_path(&workspace);
        assert!(db_path.is_file());

        let counts = block_on_db(async {
            let db = open_note_db(&db_path).await?;
            migrate(&db).await?;
            let notes = count(&db, "SELECT COUNT(*) FROM notes").await?;
            let blocks = count(&db, "SELECT COUNT(*) FROM blocks").await?;
            let links = count(&db, "SELECT COUNT(*) FROM links").await?;
            let tags = count(&db, "SELECT COUNT(*) FROM tags").await?;
            let tasks = count(&db, "SELECT COUNT(*) FROM tasks").await?;
            let migration = count(
                &db,
                "SELECT COALESCE(MAX(version), 0) FROM schema_migrations",
            )
            .await?;
            db.close().await;
            Ok((notes, blocks, links, tags, tasks, migration))
        })
        .unwrap();

        assert_eq!(counts.0, 1);
        assert!(counts.1 >= 3);
        assert_eq!(counts.2, 1);
        assert_eq!(counts.3, 1);
        assert_eq!(counts.4, 1);
        assert_eq!(counts.5, SCHEMA_VERSION);

        let _ = std::fs::remove_dir_all(&workspace.root);
    }

    #[test]
    fn rebuild_graph_allows_duplicate_heading_slugs() {
        let workspace = test_workspace("duplicate-headings");
        std::fs::create_dir_all(workspace.notes_workspace_dir()).unwrap();
        std::fs::write(
            workspace.notes_workspace_dir().join("Roadmap.md"),
            "# Roadmap\n\n## Tasks\n\nFirst section.\n\n## Tasks\n\nSecond section.\n\nSee [[Roadmap#Tasks]].\n",
        )
        .unwrap();
        let index = WorkspaceNoteIndex::build(&workspace).unwrap();

        rebuild_note_graph(&workspace, &index).unwrap();
        let db_path = workspace_graph_db_path(&workspace);

        let counts = block_on_db(async {
            let db = open_note_db(&db_path).await?;
            migrate(&db).await?;
            let headings = count(&db, "SELECT COUNT(*) FROM headings").await?;
            let distinct_heading_ids =
                count(&db, "SELECT COUNT(DISTINCT id) FROM headings").await?;
            let resolved_heading_links = count(
                &db,
                "SELECT COUNT(*) FROM links WHERE target_heading_id IS NOT NULL",
            )
            .await?;
            db.close().await;
            Ok((headings, distinct_heading_ids, resolved_heading_links))
        })
        .unwrap();

        assert_eq!(counts.0, 3);
        assert_eq!(counts.1, 3);
        assert_eq!(counts.2, 1);

        let _ = std::fs::remove_dir_all(&workspace.root);
    }
}
