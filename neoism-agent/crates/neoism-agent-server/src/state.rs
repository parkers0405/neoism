use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use anyhow::Context;
use neoism_agent_core::{
    EventPayload, Id, IdKind, MessageInfo, MessageWithParts, PermissionRequestInfo,
    PermissionRule, PromptRequest, PtyInfo, QuestionRequestInfo, SessionInfo,
    SessionStatus, TodoInfo,
};
use serde::de::DeserializeOwned;
use serde_json::{json, Value};
use sqlx::sqlite::{SqliteConnectOptions, SqlitePoolOptions, SqliteRow};
use sqlx::SqlitePool;
use tokio::sync::{broadcast, oneshot, Mutex, RwLock};
use turso::Value as SqlValue;

use crate::auth_store::AuthStore;
use crate::plugin::PluginRegistry;
use crate::provider::ProviderRegistry;
use crate::provider_catalog::ProviderCatalog;

#[derive(Clone)]
pub struct AppState {
    pub(crate) inner: Arc<InnerState>,
}

pub(crate) struct InnerState {
    pub(crate) store: SessionStore,
    pub(crate) auth_store: AuthStore,
    /// Embeddings provider for semantic transcript search; `None` when no
    /// API key is configured (semantic search reports unavailable).
    pub(crate) semantic: Option<crate::semantic::EmbeddingsClient>,
    pub(crate) providers: ProviderRegistry,
    pub(crate) provider_catalog: ProviderCatalog,
    pub(crate) provider_oauth: RwLock<HashMap<String, ProviderOAuthPending>>,
    pub(crate) plugins: PluginRegistry,
    pub(crate) statuses: RwLock<HashMap<String, SessionStatus>>,
    pub(crate) runs: RwLock<HashMap<String, SessionRun>>,
    pub(crate) prompt_queue_workers: RwLock<HashSet<String>>,
    pub(crate) background_jobs:
        RwLock<HashMap<String, crate::background_job::BackgroundJob>>,
    pub(crate) permissions: RwLock<HashMap<String, PermissionRequestInfo>>,
    pub(crate) permission_waiters: RwLock<HashMap<String, PermissionPending>>,
    pub(crate) permission_approvals: RwLock<HashMap<String, Vec<PermissionRule>>>,
    pub(crate) questions: RwLock<HashMap<String, QuestionRequestInfo>>,
    pub(crate) question_waiters: RwLock<HashMap<String, QuestionPending>>,
    pub(crate) todos: RwLock<HashMap<String, Vec<TodoInfo>>>,
    pub(crate) ptys: RwLock<HashMap<String, PtyInfo>>,
    pub(crate) pty_connect_tokens: RwLock<crate::pty::ConnectTokens>,
    events: broadcast::Sender<EventPayload>,
}

pub(crate) struct PermissionPending {
    pub(crate) request: PermissionRequestInfo,
    pub(crate) sender: oneshot::Sender<Result<Vec<PermissionRule>, String>>,
}

pub(crate) struct QuestionPending {
    pub(crate) sender: oneshot::Sender<Result<Vec<Vec<String>>, String>>,
}

#[derive(Clone)]
pub(crate) struct SessionRun {
    pub(crate) id: String,
    pub(crate) started_at: u64,
    pub(crate) cancel: Arc<AtomicBool>,
}

pub(crate) enum ProviderOAuthPending {
    OpenAiBrowser {
        issuer: String,
        redirect_uri: String,
        code_verifier: String,
        state: String,
        receiver: Arc<Mutex<Option<oneshot::Receiver<Result<String, String>>>>>,
    },
    OpenAiHeadless {
        issuer: String,
        device_auth_id: String,
        user_code: String,
        interval_ms: u64,
    },
    GithubCopilot {
        access_token_url: String,
        device_code: String,
        interval_ms: u64,
        enterprise_url: Option<String>,
    },
    /// xAI Grok "SuperGrok" OAuth: the browser redirects to
    /// `127.0.0.1:56121/callback?code=…`; the user copies that `code` and
    /// pastes it, and we exchange it with the stored PKCE verifier +
    /// redirect_uri.
    XaiLoopback {
        redirect_uri: String,
        code_verifier: String,
    },
    /// xAI Grok headless device-code OAuth: poll the token endpoint until the
    /// user finishes authorizing on another device.
    XaiDevice {
        device_code: String,
        interval_ms: u64,
    },
}

/// Which engine backs the agent database. Chosen once at startup from
/// `NEOISM_AGENT_DB_BACKEND` (`turso` default, `sqlite` opt-out): the store is
/// process-wide state opened before any per-directory config is loaded, so
/// this cannot live in project config. Turso is the Rust rewrite of SQLite
/// (MVCC concurrent writes); it has no FTS5 — see `migrate_fts` for how
/// search degrades there.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum DbBackend {
    Sqlite,
    Turso,
}

pub(crate) fn db_backend_from_env() -> anyhow::Result<DbBackend> {
    match std::env::var("NEOISM_AGENT_DB_BACKEND").ok() {
        None => Ok(DbBackend::Turso),
        Some(value) => match value.trim().to_ascii_lowercase().as_str() {
            "" | "turso" => Ok(DbBackend::Turso),
            "sqlite" => Ok(DbBackend::Sqlite),
            other => anyhow::bail!(
                "unknown NEOISM_AGENT_DB_BACKEND value `{other}` (expected \"sqlite\" or \"turso\")"
            ),
        },
    }
}

#[derive(Clone)]
pub(crate) struct SessionStore {
    db: Db,
    /// FTS5 only exists in the SQLite backend. When the mirror is
    /// unavailable, `fts_insert_message` no-ops and `search_messages` falls
    /// back to a LIKE scan.
    fts_enabled: Arc<AtomicBool>,
}

/// The two storage engines behind one set of SQL. Every statement the store
/// issues must stay inside the dialect both engines support (FTS5 is the one
/// exception, handled explicitly). Turso connections come from an internal
/// pool, so `connect()` per operation is cheap and lets MVCC run writes
/// concurrently.
#[derive(Clone)]
enum Db {
    Sqlite(SqlitePool),
    Turso(turso::Database),
}

/// An engine-agnostic row: column names plus SQLite-typed values.
struct DbRow {
    columns: Arc<Vec<String>>,
    values: Vec<SqlValue>,
}

impl DbRow {
    fn index(&self, name: &str) -> anyhow::Result<usize> {
        self.columns
            .iter()
            .position(|column| column == name)
            .with_context(|| format!("row has no column named `{name}`"))
    }

    fn value(&self, name: &str) -> anyhow::Result<&SqlValue> {
        let index = self.index(name)?;
        self.values
            .get(index)
            .with_context(|| format!("row has no value for column `{name}`"))
    }

    fn get_str(&self, name: &str) -> anyhow::Result<String> {
        match self.value(name)? {
            SqlValue::Text(value) => Ok(value.clone()),
            other => anyhow::bail!("column `{name}` is not text: {other:?}"),
        }
    }

    fn get_opt_str(&self, name: &str) -> anyhow::Result<Option<String>> {
        match self.value(name)? {
            SqlValue::Text(value) => Ok(Some(value.clone())),
            SqlValue::Null => Ok(None),
            other => anyhow::bail!("column `{name}` is not text or null: {other:?}"),
        }
    }

    fn get_i64(&self, name: &str) -> anyhow::Result<i64> {
        match self.value(name)? {
            SqlValue::Integer(value) => Ok(*value),
            other => anyhow::bail!("column `{name}` is not an integer: {other:?}"),
        }
    }

    fn get_f64(&self, name: &str) -> anyhow::Result<f64> {
        match self.value(name)? {
            SqlValue::Real(value) => Ok(*value),
            SqlValue::Integer(value) => Ok(*value as f64),
            other => anyhow::bail!("column `{name}` is not numeric: {other:?}"),
        }
    }

    fn i64_at(&self, index: usize) -> anyhow::Result<i64> {
        match self.values.get(index) {
            Some(SqlValue::Integer(value)) => Ok(*value),
            Some(other) => anyhow::bail!("column {index} is not an integer: {other:?}"),
            None => anyhow::bail!("row has no column {index}"),
        }
    }
}

fn text(value: impl Into<String>) -> SqlValue {
    SqlValue::Text(value.into())
}

fn opt_text(value: Option<String>) -> SqlValue {
    value.map(SqlValue::Text).unwrap_or(SqlValue::Null)
}

fn int(value: i64) -> SqlValue {
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

fn sqlite_row_to_db_row(row: &SqliteRow) -> anyhow::Result<DbRow> {
    use sqlx::{Column as _, Row as _, TypeInfo as _, ValueRef as _};
    let mut columns = Vec::with_capacity(row.len());
    let mut values = Vec::with_capacity(row.len());
    for (index, column) in row.columns().iter().enumerate() {
        columns.push(column.name().to_string());
        let raw = row.try_get_raw(index)?;
        let value = if raw.is_null() {
            SqlValue::Null
        } else {
            // Decode by the value's actual storage class, not the column's
            // declared type: expression columns (COALESCE, snippet, …) only
            // carry the former.
            match raw.type_info().name() {
                "INTEGER" => SqlValue::Integer(row.try_get(index)?),
                "REAL" => SqlValue::Real(row.try_get(index)?),
                "BLOB" => SqlValue::Blob(row.try_get(index)?),
                _ => SqlValue::Text(row.try_get(index)?),
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
/// (its connections carry a 5s busy_timeout), so concurrent writers — e.g.
/// streamed event appends racing a new session insert — fail outright.
/// Emulate busy_timeout with a bounded retry so writers queue instead.
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

impl Db {
    async fn execute(&self, sql: &str, params: Vec<SqlValue>) -> anyhow::Result<u64> {
        match self {
            Db::Sqlite(pool) => {
                let mut query = sqlx::query(sql);
                for param in params {
                    query = bind_value(query, param);
                }
                Ok(query.execute(pool).await?.rows_affected())
            }
            Db::Turso(db) => Ok(turso_busy_retry(|| {
                let params = params.clone();
                async move {
                    let conn = db.connect()?;
                    conn.execute(sql, params).await
                }
            })
            .await?),
        }
    }

    async fn fetch_all(
        &self,
        sql: &str,
        params: Vec<SqlValue>,
    ) -> anyhow::Result<Vec<DbRow>> {
        match self {
            Db::Sqlite(pool) => {
                let mut query = sqlx::query(sql);
                for param in params {
                    query = bind_value(query, param);
                }
                let rows = query.fetch_all(pool).await?;
                rows.iter().map(sqlite_row_to_db_row).collect()
            }
            Db::Turso(db) => Ok(turso_busy_retry(|| {
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
            .await?),
        }
    }

    async fn fetch_optional(
        &self,
        sql: &str,
        params: Vec<SqlValue>,
    ) -> anyhow::Result<Option<DbRow>> {
        Ok(self.fetch_all(sql, params).await?.into_iter().next())
    }

    async fn fetch_scalar_i64(
        &self,
        sql: &str,
        params: Vec<SqlValue>,
    ) -> anyhow::Result<i64> {
        self.fetch_optional(sql, params)
            .await?
            .context("query returned no rows")?
            .i64_at(0)
    }
}

#[derive(Clone, Debug)]
pub(crate) struct PersistedEvent {
    pub(crate) seq: i64,
    pub(crate) aggregate_id: String,
    pub(crate) aggregate_seq: i64,
    pub(crate) owner_id: Option<String>,
    pub(crate) payload: EventPayload,
}

#[derive(Clone, Debug)]
pub(crate) struct AggregateSequence {
    pub(crate) seq: i64,
    pub(crate) owner_id: Option<String>,
}

impl AppState {
    pub async fn open_default() -> anyhow::Result<Self> {
        let store = SessionStore::open_default().await?;
        Self::from_store(store).await
    }

    pub async fn open_database(path: impl Into<PathBuf>) -> anyhow::Result<Self> {
        let store = SessionStore::open(path.into()).await?;
        Self::from_store(store).await
    }

    async fn from_store(store: SessionStore) -> anyhow::Result<Self> {
        let (events, _) = broadcast::channel(1024);
        let auth_store = AuthStore::from_env();
        let permission_approvals = store.list_permission_approvals().await?;
        store.interrupt_stale_runs().await?;
        let state = Self {
            inner: Arc::new(InnerState {
                store,
                providers: ProviderRegistry::from_env(auth_store.clone()),
                semantic: crate::semantic::EmbeddingsClient::from_env(&auth_store),
                auth_store,
                provider_catalog: ProviderCatalog::from_env(),
                provider_oauth: RwLock::new(HashMap::new()),
                plugins: PluginRegistry::default(),
                statuses: RwLock::new(HashMap::new()),
                runs: RwLock::new(HashMap::new()),
                prompt_queue_workers: RwLock::new(HashSet::new()),
                background_jobs: RwLock::new(HashMap::new()),
                permissions: RwLock::new(HashMap::new()),
                permission_waiters: RwLock::new(HashMap::new()),
                permission_approvals: RwLock::new(permission_approvals),
                questions: RwLock::new(HashMap::new()),
                question_waiters: RwLock::new(HashMap::new()),
                todos: RwLock::new(HashMap::new()),
                ptys: RwLock::new(HashMap::new()),
                pty_connect_tokens: RwLock::new(crate::pty::ConnectTokens::default()),
                events,
            }),
        };
        crate::session_queue::resume_prompt_queues(state.clone()).await?;
        Ok(state)
    }

    pub fn subscribe(&self) -> broadcast::Receiver<EventPayload> {
        self.inner.events.subscribe()
    }

    pub(crate) fn publish(&self, event: EventPayload) {
        self.inner.plugins.publish_event(&event);
        let store = self.inner.store.clone();
        let persisted = event.clone();
        if let Ok(handle) = tokio::runtime::Handle::try_current() {
            handle.spawn(async move {
                let _ = store.append_event(&persisted).await;
            });
        }
        let _ = self.inner.events.send(event);
    }

    #[cfg(test)]
    pub(crate) async fn publish_persisted(
        &self,
        event: EventPayload,
    ) -> anyhow::Result<()> {
        self.publish_persisted_with_owner(event, None).await
    }

    pub(crate) async fn publish_persisted_with_owner(
        &self,
        event: EventPayload,
        owner_id: Option<&str>,
    ) -> anyhow::Result<()> {
        self.inner.plugins.publish_event(&event);
        self.inner
            .store
            .append_event_with_owner(&event, owner_id)
            .await?;
        let _ = self.inner.events.send(event);
        Ok(())
    }
}

impl SessionStore {
    pub(crate) async fn open_default() -> anyhow::Result<Self> {
        let backend = db_backend_from_env()?;
        let state_dir = PathBuf::from(crate::default_state_dir());
        std::fs::create_dir_all(&state_dir).with_context(|| {
            format!("failed to create state directory {}", state_dir.display())
        })?;
        // Turso gets its own default file: it is beta, so it must never
        // rewrite the SQLite-managed database in place. Switching backends
        // therefore starts from an empty session history.
        let filename = match backend {
            DbBackend::Sqlite => "agent.sqlite3",
            DbBackend::Turso => "agent.turso.db",
        };
        Self::open_with_backend(state_dir.join(filename), backend).await
    }

    pub(crate) async fn open(path: PathBuf) -> anyhow::Result<Self> {
        Self::open_with_backend(path, db_backend_from_env()?).await
    }

    pub(crate) async fn open_with_backend(
        path: PathBuf,
        backend: DbBackend,
    ) -> anyhow::Result<Self> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).with_context(|| {
                format!("failed to create database directory {}", parent.display())
            })?;
        }

        let db = match backend {
            DbBackend::Sqlite => {
                let options = SqliteConnectOptions::new()
                    .filename(&path)
                    .create_if_missing(true)
                    .foreign_keys(true)
                    .pragma("journal_mode", "WAL")
                    .pragma("synchronous", "NORMAL");
                let pool = SqlitePoolOptions::new()
                    .max_connections(5)
                    .connect_with(options)
                    .await
                    .with_context(|| {
                        format!("failed to open SQLite database {}", path.display())
                    })?;
                Db::Sqlite(pool)
            }
            DbBackend::Turso => {
                let path = path
                    .to_str()
                    .context("turso database path is not valid UTF-8")?;
                let database = turso::Builder::new_local(path)
                    .build()
                    .await
                    .with_context(|| format!("failed to open turso database {path}"))?;
                Db::Turso(database)
            }
        };
        let store = Self {
            db,
            fts_enabled: Arc::new(AtomicBool::new(false)),
        };
        store.migrate().await?;
        Ok(store)
    }

    async fn migrate(&self) -> anyhow::Result<()> {
        self.db
            .execute(
                r#"
            CREATE TABLE IF NOT EXISTS sessions (
                id TEXT PRIMARY KEY,
                info_json TEXT NOT NULL,
                updated INTEGER NOT NULL
            )
            "#,
                Vec::new(),
            )
            .await?;
        self.db
            .execute(
                r#"
            CREATE TABLE IF NOT EXISTS messages (
                id TEXT PRIMARY KEY,
                session_id TEXT NOT NULL,
                message_json TEXT NOT NULL,
                created INTEGER NOT NULL,
                position INTEGER NOT NULL,
                FOREIGN KEY(session_id) REFERENCES sessions(id) ON DELETE CASCADE
            )
            "#,
                Vec::new(),
            )
            .await?;
        self.db
            .execute(
                r#"
            CREATE TABLE IF NOT EXISTS permission_approvals (
                project_id TEXT PRIMARY KEY,
                rules_json TEXT NOT NULL,
                updated INTEGER NOT NULL
            )
            "#,
                Vec::new(),
            )
            .await?;
        self.db
            .execute(
                r#"
            CREATE TABLE IF NOT EXISTS prompt_queue (
                id TEXT PRIMARY KEY,
                session_id TEXT NOT NULL,
                position INTEGER NOT NULL,
                request_json TEXT NOT NULL,
                created INTEGER NOT NULL,
                FOREIGN KEY(session_id) REFERENCES sessions(id) ON DELETE CASCADE
            )
            "#,
                Vec::new(),
            )
            .await?;
        self.db
            .execute(
                r#"
            CREATE TABLE IF NOT EXISTS events (
                seq INTEGER PRIMARY KEY AUTOINCREMENT,
                event_id TEXT NOT NULL,
                kind TEXT NOT NULL,
                aggregate_id TEXT,
                aggregate_seq INTEGER,
                owner_id TEXT,
                session_id TEXT,
                event_json TEXT NOT NULL,
                created INTEGER NOT NULL
            )
            "#,
                Vec::new(),
            )
            .await?;
        self.db
            .execute(
                r#"
            CREATE TABLE IF NOT EXISTS event_sequences (
                aggregate_id TEXT PRIMARY KEY,
                seq INTEGER NOT NULL,
                owner_id TEXT
            )
            "#,
                Vec::new(),
            )
            .await?;
        self.db
            .execute(
                r#"
            CREATE TABLE IF NOT EXISTS session_runs (
                id TEXT PRIMARY KEY,
                session_id TEXT NOT NULL,
                status TEXT NOT NULL,
                created INTEGER NOT NULL,
                updated INTEGER NOT NULL,
                error_json TEXT,
                FOREIGN KEY(session_id) REFERENCES sessions(id) ON DELETE CASCADE
            )
            "#,
                Vec::new(),
            )
            .await?;
        self.ensure_event_columns().await?;
        self.db
            .execute(
                r#"
            CREATE INDEX IF NOT EXISTS idx_events_missing_aggregate
            ON events(seq)
            WHERE aggregate_id IS NULL OR aggregate_seq IS NULL
            "#,
                Vec::new(),
            )
            .await?;
        self.backfill_event_sequences().await?;
        self.db
            .execute(
                "CREATE INDEX IF NOT EXISTS idx_sessions_updated ON sessions(updated)",
                Vec::new(),
            )
            .await?;
        self.db
            .execute(
                "CREATE INDEX IF NOT EXISTS idx_messages_session_position ON messages(session_id, position)",
                Vec::new(),
            )
            .await?;
        self.db
            .execute(
                "CREATE INDEX IF NOT EXISTS idx_prompt_queue_session_position ON prompt_queue(session_id, position)",
                Vec::new(),
            )
            .await?;
        self.db
            .execute(
                "CREATE INDEX IF NOT EXISTS idx_events_seq ON events(seq)",
                Vec::new(),
            )
            .await?;
        self.db
            .execute(
                "CREATE INDEX IF NOT EXISTS idx_events_session_seq ON events(session_id, seq)",
                Vec::new(),
            )
            .await?;
        self.db
            .execute(
                "CREATE INDEX IF NOT EXISTS idx_events_aggregate_seq ON events(aggregate_id, aggregate_seq)",
                Vec::new(),
            )
            .await?;
        self.db
            .execute(
                "CREATE INDEX IF NOT EXISTS idx_session_runs_session_updated ON session_runs(session_id, updated)",
                Vec::new(),
            )
            .await?;
        self.migrate_fts().await?;
        self.migrate_semantic().await?;
        Ok(())
    }

    /// Vector-embedding mirror of `messages` for semantic search. The table
    /// exists on both backends (so cleanup DELETEs are portable), but only
    /// turso has the `vector32`/`vector_distance_cos` functions that write
    /// and query it — `semantic_search_supported` gates all of that. Rows
    /// with a NULL embedding are tombstones for messages with no searchable
    /// text, so the indexer doesn't retry them forever.
    async fn migrate_semantic(&self) -> anyhow::Result<()> {
        self.db
            .execute(
                r#"
            CREATE TABLE IF NOT EXISTS message_embeddings (
                message_id TEXT PRIMARY KEY,
                session_id TEXT NOT NULL,
                created INTEGER NOT NULL,
                model TEXT NOT NULL,
                embedding BLOB
            )
            "#,
                Vec::new(),
            )
            .await?;
        self.db
            .execute(
                "CREATE INDEX IF NOT EXISTS idx_message_embeddings_session ON message_embeddings(session_id)",
                Vec::new(),
            )
            .await?;
        Ok(())
    }

    pub(crate) fn semantic_search_supported(&self) -> bool {
        matches!(self.db, Db::Turso(_))
    }

    /// Messages that still need an embedding for `model` — new messages plus
    /// anything indexed under a different model (a model switch re-indexes).
    /// Sessions with an active run are skipped so streamed messages are only
    /// embedded once they've stopped changing.
    pub(crate) async fn messages_missing_embeddings(
        &self,
        model: &str,
        limit: usize,
    ) -> anyhow::Result<Vec<PendingEmbedding>> {
        let rows = self
            .db
            .fetch_all(
                "SELECT m.session_id, m.id, m.created, m.message_json \
                 FROM messages m \
                 LEFT JOIN message_embeddings e \
                   ON e.message_id = m.id AND (e.model = ? OR e.model = 'none') \
                 WHERE e.message_id IS NULL \
                   AND m.session_id NOT IN (SELECT session_id FROM session_runs WHERE status IN ('running', 'retry')) \
                 ORDER BY m.created DESC LIMIT ?",
                vec![text(model), int(limit.clamp(1, 256) as i64)],
            )
            .await?;
        rows.into_iter()
            .map(|row| {
                Ok(PendingEmbedding {
                    session_id: row.get_str("session_id")?,
                    message_id: row.get_str("id")?,
                    created: row.get_i64("created")?,
                    message_json: row.get_str("message_json")?,
                })
            })
            .collect()
    }

    pub(crate) async fn upsert_message_embedding(
        &self,
        message_id: &str,
        session_id: &str,
        created: i64,
        model: &str,
        vector_json: &str,
    ) -> anyhow::Result<()> {
        self.db
            .execute(
                r#"
            INSERT INTO message_embeddings (message_id, session_id, created, model, embedding)
            VALUES (?, ?, ?, ?, vector32(?))
            ON CONFLICT(message_id) DO UPDATE SET
                model = excluded.model,
                embedding = excluded.embedding
            "#,
                vec![
                    text(message_id),
                    text(session_id),
                    int(created),
                    text(model),
                    text(vector_json),
                ],
            )
            .await?;
        Ok(())
    }

    /// Mark a message as having nothing to embed (no searchable text or
    /// undecodable JSON) so the indexer stops picking it up.
    pub(crate) async fn tombstone_message_embedding(
        &self,
        message_id: &str,
        session_id: &str,
        created: i64,
    ) -> anyhow::Result<()> {
        self.db
            .execute(
                r#"
            INSERT INTO message_embeddings (message_id, session_id, created, model, embedding)
            VALUES (?, ?, ?, 'none', NULL)
            ON CONFLICT(message_id) DO UPDATE SET model = 'none', embedding = NULL
            "#,
                vec![text(message_id), text(session_id), int(created)],
            )
            .await?;
        Ok(())
    }

    /// Rank indexed messages by cosine distance to an embedded query.
    /// Exact scan — fine at chat-history scale.
    pub(crate) async fn semantic_search(
        &self,
        query_vector_json: &str,
        model: &str,
        session_id: Option<&str>,
        limit: usize,
    ) -> anyhow::Result<Vec<SemanticSearchHit>> {
        let limit = limit.clamp(1, 50) as i64;
        let (sql, params) = match session_id {
            Some(session) => (
                "SELECT e.session_id, e.message_id, m.message_json, \
                 vector_distance_cos(e.embedding, vector32(?)) AS distance \
                 FROM message_embeddings e JOIN messages m ON m.id = e.message_id \
                 WHERE e.model = ? AND e.embedding IS NOT NULL AND e.session_id = ? \
                 ORDER BY distance ASC LIMIT ?",
                vec![
                    text(query_vector_json),
                    text(model),
                    text(session),
                    int(limit),
                ],
            ),
            None => (
                "SELECT e.session_id, e.message_id, m.message_json, \
                 vector_distance_cos(e.embedding, vector32(?)) AS distance \
                 FROM message_embeddings e JOIN messages m ON m.id = e.message_id \
                 WHERE e.model = ? AND e.embedding IS NOT NULL \
                 ORDER BY distance ASC LIMIT ?",
                vec![text(query_vector_json), text(model), int(limit)],
            ),
        };
        let rows = self.db.fetch_all(sql, params).await?;
        let mut hits = Vec::new();
        for row in rows {
            let json = row.get_str("message_json")?;
            let Ok(message) = serde_json::from_str::<MessageWithParts>(&json) else {
                continue;
            };
            let (role, created, content) = fts_document(&message);
            let excerpt: String = content.replace('\n', " ").chars().take(200).collect();
            hits.push(SemanticSearchHit {
                session_id: row.get_str("session_id")?,
                message_id: row.get_str("message_id")?,
                role,
                created,
                excerpt,
                distance: row.get_f64("distance")?,
            });
        }
        Ok(hits)
    }

    /// Full-text search mirror of `messages`. FTS5 ships in the bundled
    /// SQLite (libsqlite3-sys sets SQLITE_ENABLE_FTS5), porter stemming
    /// makes "fixing"/"fixed" match. The mirror is best-effort: index
    /// failures must never break transcript persistence, so callers wrap
    /// fts_* errors in warnings instead of propagating them. Turso has no
    /// FTS5 at all, so there the mirror is disabled up front and
    /// `search_messages` uses the LIKE fallback.
    async fn migrate_fts(&self) -> anyhow::Result<()> {
        let created = self
            .db
            .execute(
                r#"
            CREATE VIRTUAL TABLE IF NOT EXISTS messages_fts USING fts5(
                session_id UNINDEXED,
                message_id UNINDEXED,
                role UNINDEXED,
                created UNINDEXED,
                content,
                tokenize = 'porter unicode61'
            )
            "#,
                Vec::new(),
            )
            .await;
        match created {
            Ok(_) => {
                self.fts_enabled.store(true, Ordering::Relaxed);
                self.backfill_fts().await
            }
            Err(error) => match &self.db {
                Db::Sqlite(_) => Err(error),
                Db::Turso(_) => {
                    tracing::warn!(
                        %error,
                        "FTS5 is unavailable on the turso backend; message search will use a LIKE scan"
                    );
                    Ok(())
                }
            },
        }
    }

    /// One-time backfill of transcripts that predate the FTS mirror.
    async fn backfill_fts(&self) -> anyhow::Result<()> {
        let indexed = self
            .db
            .fetch_scalar_i64("SELECT count(*) FROM messages_fts", Vec::new())
            .await?;
        if indexed > 0 {
            return Ok(());
        }
        let rows = self
            .db
            .fetch_all(
                "SELECT session_id, id, message_json FROM messages",
                Vec::new(),
            )
            .await?;
        for row in rows {
            let session_id = row.get_str("session_id")?;
            let message_id = row.get_str("id")?;
            let json = row.get_str("message_json")?;
            let Ok(message) = serde_json::from_str::<MessageWithParts>(&json) else {
                continue;
            };
            let (role, created, content) = fts_document(&message);
            if content.is_empty() {
                continue;
            }
            self.db
                .execute(
                    "INSERT INTO messages_fts (session_id, message_id, role, created, content) \
                     VALUES (?, ?, ?, ?, ?)",
                    vec![
                        text(session_id),
                        text(message_id),
                        text(role),
                        int(sqlite_i64(created)),
                        text(content),
                    ],
                )
                .await?;
        }
        Ok(())
    }

    async fn fts_insert_message(
        &self,
        session_id: &str,
        message: &MessageWithParts,
    ) -> anyhow::Result<()> {
        if !self.fts_enabled.load(Ordering::Relaxed) {
            return Ok(());
        }
        let (role, created, content) = fts_document(message);
        if content.is_empty() {
            return Ok(());
        }
        self.db
            .execute(
                "INSERT INTO messages_fts (session_id, message_id, role, created, content) \
                 VALUES (?, ?, ?, ?, ?)",
                vec![
                    text(session_id),
                    text(message_id(message)),
                    text(role),
                    int(sqlite_i64(created)),
                    text(content),
                ],
            )
            .await?;
        Ok(())
    }

    /// Full-text search across session transcripts. `query` uses FTS5
    /// syntax (bare words are AND-ed; porter stemming applies). Results
    /// rank by bm25 and carry a snippet with match markers. Without FTS
    /// (turso backend) the fallback AND-matches whole words case-insensitively
    /// over recent messages — no stemming, recency order instead of bm25.
    pub(crate) async fn search_messages(
        &self,
        query: &str,
        session_id: Option<&str>,
        limit: usize,
    ) -> anyhow::Result<Vec<MessageSearchHit>> {
        let limit = limit.clamp(1, 50);
        if !self.fts_enabled.load(Ordering::Relaxed) {
            return self.search_messages_like(query, session_id, limit).await;
        }
        let sql = if session_id.is_some() {
            "SELECT session_id, message_id, role, created, \
             snippet(messages_fts, 4, '>>', '<<', ' ... ', 24) AS excerpt \
             FROM messages_fts WHERE messages_fts MATCH ? AND session_id = ? \
             ORDER BY bm25(messages_fts) LIMIT ?"
        } else {
            "SELECT session_id, message_id, role, created, \
             snippet(messages_fts, 4, '>>', '<<', ' ... ', 24) AS excerpt \
             FROM messages_fts WHERE messages_fts MATCH ? \
             ORDER BY bm25(messages_fts) LIMIT ?"
        };
        let mut params = vec![text(query)];
        if let Some(session) = session_id {
            params.push(text(session));
        }
        params.push(int(limit as i64));
        let rows = self.db.fetch_all(sql, params).await?;
        rows.into_iter()
            .map(|row| {
                Ok(MessageSearchHit {
                    session_id: row.get_str("session_id")?,
                    message_id: row.get_str("message_id")?,
                    role: row.get_str("role")?,
                    created: row.get_i64("created")?.max(0) as u64,
                    excerpt: row.get_str("excerpt")?,
                })
            })
            .collect()
    }

    /// LIKE-scan fallback for backends without FTS5. Prefilters in SQL on the
    /// longest term (LIKE is ASCII-case-insensitive in both engines), then
    /// AND-matches every term against the same flattened document the FTS
    /// mirror indexes. Bounded to the most recent candidates, so it trades
    /// recall on huge histories for predictable work.
    async fn search_messages_like(
        &self,
        query: &str,
        session_id: Option<&str>,
        limit: usize,
    ) -> anyhow::Result<Vec<MessageSearchHit>> {
        const SCAN_CAP: i64 = 500;
        let terms: Vec<&str> = query.split_whitespace().collect();
        let Some(seed) = terms.iter().max_by_key(|term| term.len()) else {
            return Ok(Vec::new());
        };
        let pattern = format!("%{}%", escape_like(seed));
        let (sql, params) = match session_id {
            Some(session) => (
                "SELECT session_id, id, message_json FROM messages \
                 WHERE message_json LIKE ? ESCAPE '\\' AND session_id = ? \
                 ORDER BY created DESC LIMIT ?",
                vec![text(pattern), text(session), int(SCAN_CAP)],
            ),
            None => (
                "SELECT session_id, id, message_json FROM messages \
                 WHERE message_json LIKE ? ESCAPE '\\' \
                 ORDER BY created DESC LIMIT ?",
                vec![text(pattern), int(SCAN_CAP)],
            ),
        };
        let rows = self.db.fetch_all(sql, params).await?;
        let mut hits = Vec::new();
        for row in rows {
            let json = row.get_str("message_json")?;
            let Ok(message) = serde_json::from_str::<MessageWithParts>(&json) else {
                continue;
            };
            let (role, created, content) = fts_document(&message);
            let matches: Vec<usize> = terms
                .iter()
                .map(|term| find_ignore_ascii_case(&content, term))
                .collect::<Option<Vec<_>>>()
                .unwrap_or_default();
            let Some(&first) = matches.iter().min() else {
                continue;
            };
            let matched_term = terms[matches
                .iter()
                .position(|&start| start == first)
                .unwrap_or(0)];
            hits.push(MessageSearchHit {
                session_id: row.get_str("session_id")?,
                message_id: row.get_str("id")?,
                role,
                created,
                excerpt: like_excerpt(&content, first, matched_term.len()),
            });
            if hits.len() >= limit {
                break;
            }
        }
        Ok(hits)
    }

    async fn ensure_event_columns(&self) -> anyhow::Result<()> {
        for (column, definition) in [
            ("aggregate_id", "TEXT"),
            ("aggregate_seq", "INTEGER"),
            ("owner_id", "TEXT"),
        ] {
            if !self.table_has_column("events", column).await? {
                self.db
                    .execute(
                        &format!("ALTER TABLE events ADD COLUMN {column} {definition}"),
                        Vec::new(),
                    )
                    .await?;
            }
        }
        Ok(())
    }

    async fn table_has_column(&self, table: &str, column: &str) -> anyhow::Result<bool> {
        let rows = self
            .db
            .fetch_all(&format!("PRAGMA table_info({table})"), Vec::new())
            .await?;
        for row in rows {
            if row.get_str("name")? == column {
                return Ok(true);
            }
        }
        Ok(false)
    }

    async fn backfill_event_sequences(&self) -> anyhow::Result<()> {
        let rows = self
            .db
            .fetch_all(
                "SELECT seq, event_json FROM events INDEXED BY idx_events_missing_aggregate WHERE aggregate_id IS NULL OR aggregate_seq IS NULL ORDER BY seq ASC",
                Vec::new(),
            )
            .await?;
        for row in rows {
            let seq = row.get_i64("seq")?;
            let payload: EventPayload = decode_json(row.get_str("event_json")?)?;
            let aggregate_id = crate::sync::aggregate_id(&payload);
            let aggregate_seq = self.next_aggregate_sequence(&aggregate_id, None).await?;
            self.db
                .execute(
                    "UPDATE events SET aggregate_id = ?, aggregate_seq = ? WHERE seq = ?",
                    vec![text(aggregate_id), int(aggregate_seq), int(seq)],
                )
                .await?;
        }
        Ok(())
    }

    pub(crate) async fn list_sessions(&self) -> anyhow::Result<Vec<SessionInfo>> {
        let rows = self
            .db
            .fetch_all(
                "SELECT info_json FROM sessions ORDER BY updated DESC",
                Vec::new(),
            )
            .await?;
        rows.into_iter()
            .map(|row| decode_json(row.get_str("info_json")?))
            .collect()
    }

    pub(crate) async fn insert_session(&self, info: &SessionInfo) -> anyhow::Result<()> {
        self.db
            .execute(
                "INSERT INTO sessions (id, info_json, updated) VALUES (?, ?, ?)",
                vec![
                    text(info.id.to_string()),
                    text(serde_json::to_string(info)?),
                    int(sqlite_i64(info.time.updated)),
                ],
            )
            .await?;
        Ok(())
    }

    pub(crate) async fn update_session(&self, info: &SessionInfo) -> anyhow::Result<()> {
        self.db
            .execute(
                "UPDATE sessions SET info_json = ?, updated = ? WHERE id = ?",
                vec![
                    text(serde_json::to_string(info)?),
                    int(sqlite_i64(info.time.updated)),
                    text(info.id.to_string()),
                ],
            )
            .await?;
        Ok(())
    }

    pub(crate) async fn get_session(
        &self,
        session_id: &str,
    ) -> anyhow::Result<Option<SessionInfo>> {
        let row = self
            .db
            .fetch_optional(
                "SELECT info_json FROM sessions WHERE id = ?",
                vec![text(session_id)],
            )
            .await?;
        row.map(|row| decode_json(row.get_str("info_json")?))
            .transpose()
    }

    pub(crate) async fn delete_session(&self, session_id: &str) -> anyhow::Result<bool> {
        // Delete children explicitly instead of leaning on the FK cascade:
        // the cascade only fires where the foreign_keys pragma is on, and
        // turso connections don't enable it. Same end state on both engines.
        for sql in [
            "DELETE FROM messages WHERE session_id = ?",
            "DELETE FROM prompt_queue WHERE session_id = ?",
            "DELETE FROM session_runs WHERE session_id = ?",
            "DELETE FROM message_embeddings WHERE session_id = ?",
        ] {
            self.db.execute(sql, vec![text(session_id)]).await?;
        }
        let affected = self
            .db
            .execute("DELETE FROM sessions WHERE id = ?", vec![text(session_id)])
            .await?;
        Ok(affected > 0)
    }

    pub(crate) async fn list_messages(
        &self,
        session_id: &str,
    ) -> anyhow::Result<Vec<MessageWithParts>> {
        let rows = self
            .db
            .fetch_all(
                "SELECT message_json FROM messages WHERE session_id = ? ORDER BY position ASC, created ASC",
                vec![text(session_id)],
            )
            .await?;
        rows.into_iter()
            .map(|row| decode_json(row.get_str("message_json")?))
            .collect()
    }

    /// Page through a session's messages without decoding the whole
    /// transcript. `cursor` is the id of a message (its info id) or of any
    /// part inside one; results are the `limit` messages immediately older
    /// than it (`order_desc`) or newer than it (ascending). The cursor and
    /// limit are pushed into SQL so "load older" reads only the page it
    /// needs instead of the entire session every time.
    pub(crate) async fn list_messages_page(
        &self,
        session_id: &str,
        cursor: Option<&str>,
        limit: Option<usize>,
        order_desc: bool,
    ) -> anyhow::Result<Vec<MessageWithParts>> {
        let cursor_position = match cursor.filter(|cursor| !cursor.is_empty()) {
            Some(cursor) => self.message_position(session_id, cursor).await?,
            None => None,
        };
        let direction = if order_desc { "DESC" } else { "ASC" };
        // For desc we want messages older than the cursor (lower position);
        // for asc, newer (higher position). A cursor we couldn't resolve
        // behaves as no cursor — same as the old in-memory path.
        let cursor_clause = match (cursor_position, order_desc) {
            (Some(_), true) => " AND position < ?",
            (Some(_), false) => " AND position > ?",
            (None, _) => "",
        };
        let limit_clause = match limit.filter(|limit| *limit > 0) {
            Some(limit) => format!(" LIMIT {}", limit as i64),
            None => String::new(),
        };
        let sql = format!(
            "SELECT message_json FROM messages WHERE session_id = ?{cursor_clause} \
             ORDER BY position {direction}, created {direction}{limit_clause}"
        );
        let mut params = vec![text(session_id)];
        if let Some(position) = cursor_position {
            params.push(int(position));
        }
        let rows = self.db.fetch_all(&sql, params).await?;
        rows.into_iter()
            .map(|row| decode_json(row.get_str("message_json")?))
            .collect()
    }

    /// Resolve a history cursor to its row `position`. Tries the message id
    /// (primary key) first, then falls back to matching a part id embedded in
    /// the message JSON — without decoding rows into structs.
    async fn message_position(
        &self,
        session_id: &str,
        cursor: &str,
    ) -> anyhow::Result<Option<i64>> {
        let by_message_id = self
            .db
            .fetch_optional(
                "SELECT position FROM messages WHERE session_id = ? AND id = ? LIMIT 1",
                vec![text(session_id), text(cursor)],
            )
            .await?;
        if let Some(row) = by_message_id {
            return Ok(Some(row.get_i64("position")?));
        }
        let pattern = format!("%\"id\":\"{}\"%", escape_like(cursor));
        let by_part_id = self
            .db
            .fetch_optional(
                "SELECT position FROM messages WHERE session_id = ? AND message_json LIKE ? ESCAPE '\\' \
                 ORDER BY position ASC LIMIT 1",
                vec![text(session_id), text(pattern)],
            )
            .await?;
        by_part_id.map(|row| row.get_i64("position")).transpose()
    }

    pub(crate) async fn append_message(
        &self,
        session_id: &str,
        message: &MessageWithParts,
    ) -> anyhow::Result<()> {
        let position = self
            .db
            .fetch_scalar_i64(
                "SELECT COALESCE(MAX(position), -1) + 1 FROM messages WHERE session_id = ?",
                vec![text(session_id)],
            )
            .await?;
        self.db
            .execute(
                "INSERT INTO messages (id, session_id, message_json, created, position) VALUES (?, ?, ?, ?, ?)",
                vec![
                    text(message_id(message)),
                    text(session_id),
                    text(serde_json::to_string(message)?),
                    int(sqlite_i64(message_created(message))),
                    int(position),
                ],
            )
            .await?;
        // FTS mirror failures must never break transcript persistence.
        if let Err(error) = self.fts_insert_message(session_id, message).await {
            tracing::warn!(%error, "failed to index message for full-text search");
        }
        Ok(())
    }

    pub(crate) async fn get_message(
        &self,
        session_id: &str,
        message_id: &str,
    ) -> anyhow::Result<Option<MessageWithParts>> {
        let row = self
            .db
            .fetch_optional(
                "SELECT message_json FROM messages WHERE session_id = ? AND id = ?",
                vec![text(session_id), text(message_id)],
            )
            .await?;
        row.map(|row| decode_json(row.get_str("message_json")?))
            .transpose()
    }

    pub(crate) async fn update_message(
        &self,
        session_id: &str,
        message: &MessageWithParts,
    ) -> anyhow::Result<bool> {
        let affected = self
            .db
            .execute(
                "UPDATE messages SET message_json = ? WHERE session_id = ? AND id = ?",
                vec![
                    text(serde_json::to_string(message)?),
                    text(session_id),
                    text(message_id(message)),
                ],
            )
            .await?;
        if affected > 0 {
            // Drop the stale embedding so the semantic indexer re-embeds the
            // edited content once the session goes quiet.
            self.db
                .execute(
                    "DELETE FROM message_embeddings WHERE message_id = ?",
                    vec![text(message_id(message))],
                )
                .await?;
        }
        Ok(affected > 0)
    }

    pub(crate) async fn delete_message(
        &self,
        session_id: &str,
        message_id: &str,
    ) -> anyhow::Result<bool> {
        let affected = self
            .db
            .execute(
                "DELETE FROM messages WHERE session_id = ? AND id = ?",
                vec![text(session_id), text(message_id)],
            )
            .await?;
        self.db
            .execute(
                "DELETE FROM message_embeddings WHERE message_id = ?",
                vec![text(message_id)],
            )
            .await?;
        Ok(affected > 0)
    }

    /// Remove every transcript message for a session. Used by session import to
    /// make re-importing a bundle idempotent (the prior transcript is replaced
    /// rather than appended to).
    pub(crate) async fn delete_session_messages(
        &self,
        session_id: &str,
    ) -> anyhow::Result<usize> {
        let affected = self
            .db
            .execute(
                "DELETE FROM messages WHERE session_id = ?",
                vec![text(session_id)],
            )
            .await?;
        self.db
            .execute(
                "DELETE FROM message_embeddings WHERE session_id = ?",
                vec![text(session_id)],
            )
            .await?;
        Ok(affected as usize)
    }

    pub(crate) async fn list_permission_approvals(
        &self,
    ) -> anyhow::Result<HashMap<String, Vec<PermissionRule>>> {
        let rows = self
            .db
            .fetch_all(
                "SELECT project_id, rules_json FROM permission_approvals",
                Vec::new(),
            )
            .await?;
        rows.into_iter()
            .map(|row| {
                Ok((
                    row.get_str("project_id")?,
                    decode_json(row.get_str("rules_json")?)?,
                ))
            })
            .collect()
    }

    pub(crate) async fn save_permission_approvals(
        &self,
        project_id: &str,
        rules: &[PermissionRule],
    ) -> anyhow::Result<()> {
        self.db
            .execute(
                r#"
            INSERT INTO permission_approvals (project_id, rules_json, updated)
            VALUES (?, ?, ?)
            ON CONFLICT(project_id) DO UPDATE SET
                rules_json = excluded.rules_json,
                updated = excluded.updated
            "#,
                vec![
                    text(project_id),
                    text(serde_json::to_string(rules)?),
                    int(sqlite_i64(crate::now_millis())),
                ],
            )
            .await?;
        Ok(())
    }

    pub(crate) async fn enqueue_prompt(
        &self,
        session_id: &str,
        request: &PromptRequest,
    ) -> anyhow::Result<usize> {
        let position = self
            .db
            .fetch_scalar_i64(
                "SELECT COALESCE(MAX(position), -1) + 1 FROM prompt_queue WHERE session_id = ?",
                vec![text(session_id)],
            )
            .await?;
        self.db
            .execute(
                "INSERT INTO prompt_queue (id, session_id, position, request_json, created) VALUES (?, ?, ?, ?, ?)",
                vec![
                    text(Id::ascending(IdKind::Event).to_string()),
                    text(session_id),
                    int(position),
                    text(serde_json::to_string(request)?),
                    int(sqlite_i64(crate::now_millis())),
                ],
            )
            .await?;
        self.queued_prompt_count(session_id).await
    }

    pub(crate) async fn list_queued_prompts(
        &self,
        session_id: &str,
    ) -> anyhow::Result<Vec<PromptRequest>> {
        let rows = self
            .db
            .fetch_all(
                "SELECT request_json FROM prompt_queue WHERE session_id = ? ORDER BY position ASC, created ASC",
                vec![text(session_id)],
            )
            .await?;
        rows.into_iter()
            .map(|row| decode_json(row.get_str("request_json")?))
            .collect()
    }

    pub(crate) async fn queued_prompt_count(
        &self,
        session_id: &str,
    ) -> anyhow::Result<usize> {
        let count = self
            .db
            .fetch_scalar_i64(
                "SELECT COUNT(*) FROM prompt_queue WHERE session_id = ?",
                vec![text(session_id)],
            )
            .await?;
        Ok(count.max(0) as usize)
    }

    pub(crate) async fn pop_queued_prompt(
        &self,
        session_id: &str,
    ) -> anyhow::Result<Option<PromptRequest>> {
        let Some(row) = self
            .db
            .fetch_optional(
                "SELECT id, request_json FROM prompt_queue WHERE session_id = ? ORDER BY position ASC, created ASC LIMIT 1",
                vec![text(session_id)],
            )
            .await?
        else {
            return Ok(None);
        };
        let id = row.get_str("id")?;
        let request = decode_json(row.get_str("request_json")?)?;
        self.db
            .execute("DELETE FROM prompt_queue WHERE id = ?", vec![text(id)])
            .await?;
        Ok(Some(request))
    }

    pub(crate) async fn clear_queued_prompts(
        &self,
        session_id: &str,
    ) -> anyhow::Result<usize> {
        let affected = self
            .db
            .execute(
                "DELETE FROM prompt_queue WHERE session_id = ?",
                vec![text(session_id)],
            )
            .await?;
        Ok(affected as usize)
    }

    pub(crate) async fn queued_session_ids(&self) -> anyhow::Result<Vec<String>> {
        let rows = self
            .db
            .fetch_all(
                "SELECT DISTINCT session_id FROM prompt_queue ORDER BY session_id",
                Vec::new(),
            )
            .await?;
        rows.into_iter()
            .map(|row| row.get_str("session_id"))
            .collect()
    }

    pub(crate) async fn append_event(&self, event: &EventPayload) -> anyhow::Result<()> {
        self.append_event_with_owner(event, None).await
    }

    pub(crate) async fn append_event_with_owner(
        &self,
        event: &EventPayload,
        owner_id: Option<&str>,
    ) -> anyhow::Result<()> {
        let aggregate_id = crate::sync::aggregate_id(event);
        let aggregate_seq = self
            .next_aggregate_sequence(&aggregate_id, owner_id)
            .await?;
        let session_id = event
            .properties
            .get("sessionID")
            .and_then(Value::as_str)
            .map(ToString::to_string);
        self.db
            .execute(
                "INSERT INTO events (event_id, kind, aggregate_id, aggregate_seq, owner_id, session_id, event_json, created) VALUES (?, ?, ?, ?, ?, ?, ?, ?)",
                vec![
                    text(event.id.to_string()),
                    text(event.kind.clone()),
                    text(aggregate_id),
                    int(aggregate_seq),
                    opt_text(owner_id.map(ToOwned::to_owned)),
                    opt_text(session_id),
                    text(serde_json::to_string(event)?),
                    int(sqlite_i64(crate::now_millis())),
                ],
            )
            .await?;
        Ok(())
    }

    async fn next_aggregate_sequence(
        &self,
        aggregate_id: &str,
        owner_id: Option<&str>,
    ) -> anyhow::Result<i64> {
        let latest = self.aggregate_sequence(aggregate_id).await?;
        let next = latest.as_ref().map(|row| row.seq + 1).unwrap_or(0);
        let owner = latest
            .and_then(|row| row.owner_id)
            .or_else(|| owner_id.map(ToOwned::to_owned));
        self.db
            .execute(
                r#"
            INSERT INTO event_sequences (aggregate_id, seq, owner_id)
            VALUES (?, ?, ?)
            ON CONFLICT(aggregate_id) DO UPDATE SET
                seq = excluded.seq,
                owner_id = COALESCE(event_sequences.owner_id, excluded.owner_id)
            "#,
                vec![text(aggregate_id), int(next), opt_text(owner)],
            )
            .await?;
        Ok(next)
    }

    pub(crate) async fn aggregate_sequence(
        &self,
        aggregate_id: &str,
    ) -> anyhow::Result<Option<AggregateSequence>> {
        let row = self
            .db
            .fetch_optional(
                "SELECT seq, owner_id FROM event_sequences WHERE aggregate_id = ?",
                vec![text(aggregate_id)],
            )
            .await?;
        row.map(|row| {
            Ok(AggregateSequence {
                seq: row.get_i64("seq")?,
                owner_id: row.get_opt_str("owner_id")?,
            })
        })
        .transpose()
    }

    pub(crate) async fn claim_aggregate_owner(
        &self,
        aggregate_id: &str,
        owner_id: &str,
    ) -> anyhow::Result<()> {
        self.db
            .execute(
                r#"
            INSERT INTO event_sequences (aggregate_id, seq, owner_id)
            VALUES (?, -1, ?)
            ON CONFLICT(aggregate_id) DO UPDATE SET owner_id = excluded.owner_id
            "#,
                vec![text(aggregate_id), text(owner_id)],
            )
            .await?;
        Ok(())
    }

    pub(crate) async fn list_events_after(
        &self,
        since: i64,
        limit: usize,
        session_id: Option<&str>,
    ) -> anyhow::Result<Vec<PersistedEvent>> {
        let limit = limit.clamp(1, 5_000) as i64;
        let rows = if let Some(session_id) = session_id {
            self.db
                .fetch_all(
                    "SELECT seq, aggregate_id, aggregate_seq, owner_id, event_json FROM events WHERE seq > ? AND session_id = ? ORDER BY seq ASC LIMIT ?",
                    vec![int(since), text(session_id), int(limit)],
                )
                .await?
        } else {
            self.db
                .fetch_all(
                    "SELECT seq, aggregate_id, aggregate_seq, owner_id, event_json FROM events WHERE seq > ? ORDER BY seq ASC LIMIT ?",
                    vec![int(since), int(limit)],
                )
                .await?
        };
        rows.into_iter()
            .map(|row| {
                Ok(PersistedEvent {
                    seq: row.get_i64("seq")?,
                    aggregate_id: row.get_str("aggregate_id")?,
                    aggregate_seq: row.get_i64("aggregate_seq")?,
                    owner_id: row.get_opt_str("owner_id")?,
                    payload: decode_json(row.get_str("event_json")?)?,
                })
            })
            .collect()
    }

    pub(crate) async fn start_run(
        &self,
        run_id: &str,
        session_id: &str,
    ) -> anyhow::Result<()> {
        let now = sqlite_i64(crate::now_millis());
        self.db
            .execute(
                r#"
            INSERT INTO session_runs (id, session_id, status, created, updated, error_json)
            VALUES (?, ?, 'running', ?, ?, NULL)
            ON CONFLICT(id) DO UPDATE SET
                status = 'running',
                updated = excluded.updated,
                error_json = NULL
            "#,
                vec![text(run_id), text(session_id), int(now), int(now)],
            )
            .await?;
        Ok(())
    }

    pub(crate) async fn finish_run(
        &self,
        run_id: &str,
        status: &str,
        error: Option<Value>,
    ) -> anyhow::Result<()> {
        self.db
            .execute(
                r#"
            UPDATE session_runs
            SET status = ?, updated = ?, error_json = ?
            WHERE id = ?
            "#,
                vec![
                    text(status),
                    int(sqlite_i64(crate::now_millis())),
                    opt_text(error.map(|value| value.to_string())),
                    text(run_id),
                ],
            )
            .await?;
        Ok(())
    }

    pub(crate) async fn interrupt_stale_runs(&self) -> anyhow::Result<u64> {
        let affected = self
            .db
            .execute(
                r#"
            UPDATE session_runs
            SET status = 'interrupted',
                updated = ?,
                error_json = ?
            WHERE status IN ('running', 'retry')
            "#,
                vec![
                    int(sqlite_i64(crate::now_millis())),
                    text(
                        json!({ "message": "Server restarted before run completed" })
                            .to_string(),
                    ),
                ],
            )
            .await?;
        Ok(affected)
    }

    #[cfg(test)]
    pub(crate) async fn close(&self) {
        match &self.db {
            Db::Sqlite(pool) => pool.close().await,
            // Turso has no explicit close; dropping the handle releases it.
            Db::Turso(_) => {}
        }
    }
}

fn decode_json<T: DeserializeOwned>(raw: String) -> anyhow::Result<T> {
    serde_json::from_str(&raw).context("failed to decode persisted JSON")
}

fn message_id(message: &MessageWithParts) -> String {
    match &message.info {
        MessageInfo::User(message) => message.id.to_string(),
        MessageInfo::Assistant(message) => message.id.to_string(),
    }
}

fn message_created(message: &MessageWithParts) -> u64 {
    match &message.info {
        MessageInfo::User(message) => message.time.created,
        MessageInfo::Assistant(message) => message.time.created,
    }
}

fn sqlite_i64(value: u64) -> i64 {
    value.min(i64::MAX as u64) as i64
}

/// One row of `search_messages` output: where the match lives plus an
/// FTS5 snippet with `>>`/`<<` markers around matched terms.
#[derive(Debug, Clone, serde::Serialize)]
pub(crate) struct MessageSearchHit {
    pub(crate) session_id: String,
    pub(crate) message_id: String,
    pub(crate) role: String,
    pub(crate) created: u64,
    pub(crate) excerpt: String,
}

/// A message the semantic indexer still needs to embed.
#[derive(Debug, Clone)]
pub(crate) struct PendingEmbedding {
    pub(crate) session_id: String,
    pub(crate) message_id: String,
    pub(crate) created: i64,
    pub(crate) message_json: String,
}

/// One semantic search result; `distance` is cosine distance (lower = closer).
#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct SemanticSearchHit {
    pub(crate) session_id: String,
    pub(crate) message_id: String,
    pub(crate) role: String,
    pub(crate) created: u64,
    pub(crate) excerpt: String,
    pub(crate) distance: f64,
}

/// Flatten a message into `(role, created, searchable text)` for the FTS
/// index and the semantic embedding indexer. Parts are inspected as JSON so
/// new part variants degrade to "not indexed" instead of breaking
/// compilation or persistence.
pub(crate) fn fts_document(message: &MessageWithParts) -> (String, u64, String) {
    let role = match &message.info {
        MessageInfo::User(_) => "user",
        MessageInfo::Assistant(_) => "assistant",
    };
    let created = message_created(message);
    let mut chunks: Vec<String> = Vec::new();
    for part in &message.parts {
        let Ok(value) = serde_json::to_value(part) else {
            continue;
        };
        if let Some(text) = value.get("text").and_then(|text| text.as_str()) {
            let trimmed = text.trim();
            if !trimmed.is_empty() {
                chunks.push(trimmed.to_string());
            }
        }
        // Tool outputs live under state.output on completed tool parts.
        // Cap their contribution so one huge output can't bloat the index.
        if let Some(output) = value
            .get("state")
            .and_then(|state| state.get("output"))
            .and_then(|output| output.as_str())
        {
            let trimmed = output.trim();
            if !trimmed.is_empty() {
                chunks.push(trimmed.chars().take(2000).collect());
            }
        }
    }
    (role.to_string(), created, chunks.join("\n"))
}

/// Escape a value for use inside a SQL `LIKE` pattern (with `ESCAPE '\'`).
/// Ids contain `_`, which is a `LIKE` wildcard, so it must be escaped or
/// `prt_abc` would also match `prtXabc`.
fn escape_like(input: &str) -> String {
    let mut out = String::with_capacity(input.len());
    for ch in input.chars() {
        if matches!(ch, '\\' | '%' | '_') {
            out.push('\\');
        }
        out.push(ch);
    }
    out
}

/// ASCII-case-insensitive substring search. A match can only start on a
/// UTF-8 char boundary (a valid needle never begins with a continuation
/// byte), so the returned byte offset is safe to slice with.
fn find_ignore_ascii_case(haystack: &str, needle: &str) -> Option<usize> {
    if needle.is_empty() || needle.len() > haystack.len() {
        return None;
    }
    haystack
        .as_bytes()
        .windows(needle.len())
        .position(|window| window.eq_ignore_ascii_case(needle.as_bytes()))
}

/// Build a `>>match<<` excerpt around a match, mirroring the FTS5 snippet
/// markers so both search paths render the same way.
fn like_excerpt(content: &str, start: usize, len: usize) -> String {
    const CONTEXT: usize = 90;
    let end = (start + len).min(content.len());
    let mut window_start = start.saturating_sub(CONTEXT);
    while !content.is_char_boundary(window_start) {
        window_start -= 1;
    }
    let mut window_end = (end + CONTEXT).min(content.len());
    while !content.is_char_boundary(window_end) {
        window_end += 1;
    }
    let mut excerpt = String::new();
    if window_start > 0 {
        excerpt.push_str(" ... ");
    }
    excerpt.push_str(&content[window_start..start]);
    excerpt.push_str(">>");
    excerpt.push_str(&content[start..end]);
    excerpt.push_str("<<");
    excerpt.push_str(&content[end..window_end]);
    if window_end < content.len() {
        excerpt.push_str(" ... ");
    }
    excerpt.replace('\n', " ")
}
