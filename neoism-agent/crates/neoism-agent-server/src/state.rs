use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::atomic::AtomicBool;
use std::sync::Arc;

use anyhow::Context;
use neoism_agent_core::{
    EventPayload, Id, IdKind, MessageInfo, MessageWithParts, PermissionRequestInfo,
    PermissionRule, PromptRequest, PtyInfo, QuestionRequestInfo, SessionInfo,
    SessionStatus, TodoInfo,
};
use serde::de::DeserializeOwned;
use serde_json::{json, Value};
use sqlx::sqlite::{SqliteConnectOptions, SqlitePoolOptions};
use sqlx::{Row, SqlitePool};
use tokio::sync::{broadcast, oneshot, Mutex, RwLock};

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

#[derive(Clone)]
pub(crate) struct SessionStore {
    pool: SqlitePool,
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
        let state_dir = PathBuf::from(crate::default_state_dir());
        std::fs::create_dir_all(&state_dir).with_context(|| {
            format!("failed to create state directory {}", state_dir.display())
        })?;
        Self::open(state_dir.join("agent.sqlite3")).await
    }

    pub(crate) async fn open(path: PathBuf) -> anyhow::Result<Self> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).with_context(|| {
                format!("failed to create database directory {}", parent.display())
            })?;
        }

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
        let store = Self { pool };
        store.migrate().await?;
        Ok(store)
    }

    async fn migrate(&self) -> anyhow::Result<()> {
        sqlx::query(
            r#"
            CREATE TABLE IF NOT EXISTS sessions (
                id TEXT PRIMARY KEY,
                info_json TEXT NOT NULL,
                updated INTEGER NOT NULL
            )
            "#,
        )
        .execute(&self.pool)
        .await?;
        sqlx::query(
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
        )
        .execute(&self.pool)
        .await?;
        sqlx::query(
            r#"
            CREATE TABLE IF NOT EXISTS permission_approvals (
                project_id TEXT PRIMARY KEY,
                rules_json TEXT NOT NULL,
                updated INTEGER NOT NULL
            )
            "#,
        )
        .execute(&self.pool)
        .await?;
        sqlx::query(
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
        )
        .execute(&self.pool)
        .await?;
        sqlx::query(
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
        )
        .execute(&self.pool)
        .await?;
        sqlx::query(
            r#"
            CREATE TABLE IF NOT EXISTS event_sequences (
                aggregate_id TEXT PRIMARY KEY,
                seq INTEGER NOT NULL,
                owner_id TEXT
            )
            "#,
        )
        .execute(&self.pool)
        .await?;
        sqlx::query(
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
        )
        .execute(&self.pool)
        .await?;
        self.ensure_event_columns().await?;
        sqlx::query(
            r#"
            CREATE INDEX IF NOT EXISTS idx_events_missing_aggregate
            ON events(seq)
            WHERE aggregate_id IS NULL OR aggregate_seq IS NULL
            "#,
        )
        .execute(&self.pool)
        .await?;
        self.backfill_event_sequences().await?;
        sqlx::query(
            "CREATE INDEX IF NOT EXISTS idx_sessions_updated ON sessions(updated)",
        )
        .execute(&self.pool)
        .await?;
        sqlx::query(
            "CREATE INDEX IF NOT EXISTS idx_messages_session_position ON messages(session_id, position)",
        )
        .execute(&self.pool)
        .await?;
        sqlx::query(
            "CREATE INDEX IF NOT EXISTS idx_prompt_queue_session_position ON prompt_queue(session_id, position)",
        )
        .execute(&self.pool)
        .await?;
        sqlx::query("CREATE INDEX IF NOT EXISTS idx_events_seq ON events(seq)")
            .execute(&self.pool)
            .await?;
        sqlx::query("CREATE INDEX IF NOT EXISTS idx_events_session_seq ON events(session_id, seq)")
            .execute(&self.pool)
            .await?;
        sqlx::query("CREATE INDEX IF NOT EXISTS idx_events_aggregate_seq ON events(aggregate_id, aggregate_seq)")
            .execute(&self.pool)
            .await?;
        sqlx::query(
            "CREATE INDEX IF NOT EXISTS idx_session_runs_session_updated ON session_runs(session_id, updated)",
        )
        .execute(&self.pool)
        .await?;
        self.migrate_fts().await?;
        Ok(())
    }

    /// Full-text search mirror of `messages`. FTS5 ships in the bundled
    /// SQLite (libsqlite3-sys sets SQLITE_ENABLE_FTS5), porter stemming
    /// makes "fixing"/"fixed" match. The mirror is best-effort: index
    /// failures must never break transcript persistence, so callers wrap
    /// fts_* errors in warnings instead of propagating them.
    async fn migrate_fts(&self) -> anyhow::Result<()> {
        sqlx::query(
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
        )
        .execute(&self.pool)
        .await?;
        self.backfill_fts().await?;
        Ok(())
    }

    /// One-time backfill of transcripts that predate the FTS mirror.
    async fn backfill_fts(&self) -> anyhow::Result<()> {
        let indexed: i64 = sqlx::query_scalar("SELECT count(*) FROM messages_fts")
            .fetch_one(&self.pool)
            .await?;
        if indexed > 0 {
            return Ok(());
        }
        let rows = sqlx::query("SELECT session_id, id, message_json FROM messages")
            .fetch_all(&self.pool)
            .await?;
        for row in rows {
            let session_id: String = row.get("session_id");
            let message_id: String = row.get("id");
            let json: String = row.get("message_json");
            let Ok(message) = serde_json::from_str::<MessageWithParts>(&json) else {
                continue;
            };
            let (role, created, content) = fts_document(&message);
            if content.is_empty() {
                continue;
            }
            sqlx::query(
                "INSERT INTO messages_fts (session_id, message_id, role, created, content) \
                 VALUES (?, ?, ?, ?, ?)",
            )
            .bind(&session_id)
            .bind(&message_id)
            .bind(role)
            .bind(sqlite_i64(created))
            .bind(&content)
            .execute(&self.pool)
            .await?;
        }
        Ok(())
    }

    async fn fts_insert_message(
        &self,
        session_id: &str,
        message: &MessageWithParts,
    ) -> anyhow::Result<()> {
        let (role, created, content) = fts_document(message);
        if content.is_empty() {
            return Ok(());
        }
        sqlx::query(
            "INSERT INTO messages_fts (session_id, message_id, role, created, content) \
             VALUES (?, ?, ?, ?, ?)",
        )
        .bind(session_id)
        .bind(message_id(message))
        .bind(role)
        .bind(sqlite_i64(created))
        .bind(&content)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    /// Full-text search across session transcripts. `query` uses FTS5
    /// syntax (bare words are AND-ed; porter stemming applies). Results
    /// rank by bm25 and carry a snippet with match markers.
    pub(crate) async fn search_messages(
        &self,
        query: &str,
        session_id: Option<&str>,
        limit: usize,
    ) -> anyhow::Result<Vec<MessageSearchHit>> {
        let limit = limit.clamp(1, 50) as i64;
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
        let mut q = sqlx::query(sql).bind(query);
        if let Some(session) = session_id {
            q = q.bind(session);
        }
        let rows = q.bind(limit).fetch_all(&self.pool).await?;
        Ok(rows
            .into_iter()
            .map(|row| MessageSearchHit {
                session_id: row.get("session_id"),
                message_id: row.get("message_id"),
                role: row.get("role"),
                created: row.get::<i64, _>("created").max(0) as u64,
                excerpt: row.get("excerpt"),
            })
            .collect())
    }

    async fn ensure_event_columns(&self) -> anyhow::Result<()> {
        for (column, definition) in [
            ("aggregate_id", "TEXT"),
            ("aggregate_seq", "INTEGER"),
            ("owner_id", "TEXT"),
        ] {
            if !self.table_has_column("events", column).await? {
                sqlx::query(&format!(
                    "ALTER TABLE events ADD COLUMN {column} {definition}"
                ))
                .execute(&self.pool)
                .await?;
            }
        }
        Ok(())
    }

    async fn table_has_column(&self, table: &str, column: &str) -> anyhow::Result<bool> {
        let rows = sqlx::query(&format!("PRAGMA table_info({table})"))
            .fetch_all(&self.pool)
            .await?;
        Ok(rows
            .iter()
            .any(|row| row.get::<String, _>("name") == column))
    }

    async fn backfill_event_sequences(&self) -> anyhow::Result<()> {
        let rows = sqlx::query(
            "SELECT seq, event_json FROM events INDEXED BY idx_events_missing_aggregate WHERE aggregate_id IS NULL OR aggregate_seq IS NULL ORDER BY seq ASC",
        )
        .fetch_all(&self.pool)
        .await?;
        for row in rows {
            let seq = row.get::<i64, _>("seq");
            let payload: EventPayload = decode_json(row.get::<String, _>("event_json"))?;
            let aggregate_id = crate::sync::aggregate_id(&payload);
            let aggregate_seq = self.next_aggregate_sequence(&aggregate_id, None).await?;
            sqlx::query(
                "UPDATE events SET aggregate_id = ?, aggregate_seq = ? WHERE seq = ?",
            )
            .bind(&aggregate_id)
            .bind(aggregate_seq)
            .bind(seq)
            .execute(&self.pool)
            .await?;
        }
        Ok(())
    }

    pub(crate) async fn list_sessions(&self) -> anyhow::Result<Vec<SessionInfo>> {
        let rows = sqlx::query("SELECT info_json FROM sessions ORDER BY updated DESC")
            .fetch_all(&self.pool)
            .await?;
        rows.into_iter()
            .map(|row| decode_json(row.get::<String, _>("info_json")))
            .collect()
    }

    pub(crate) async fn insert_session(&self, info: &SessionInfo) -> anyhow::Result<()> {
        sqlx::query("INSERT INTO sessions (id, info_json, updated) VALUES (?, ?, ?)")
            .bind(info.id.to_string())
            .bind(serde_json::to_string(info)?)
            .bind(sqlite_i64(info.time.updated))
            .execute(&self.pool)
            .await?;
        Ok(())
    }

    pub(crate) async fn update_session(&self, info: &SessionInfo) -> anyhow::Result<()> {
        sqlx::query("UPDATE sessions SET info_json = ?, updated = ? WHERE id = ?")
            .bind(serde_json::to_string(info)?)
            .bind(sqlite_i64(info.time.updated))
            .bind(info.id.to_string())
            .execute(&self.pool)
            .await?;
        Ok(())
    }

    pub(crate) async fn get_session(
        &self,
        session_id: &str,
    ) -> anyhow::Result<Option<SessionInfo>> {
        let row = sqlx::query("SELECT info_json FROM sessions WHERE id = ?")
            .bind(session_id)
            .fetch_optional(&self.pool)
            .await?;
        row.map(|row| decode_json(row.get::<String, _>("info_json")))
            .transpose()
    }

    pub(crate) async fn delete_session(&self, session_id: &str) -> anyhow::Result<bool> {
        let result = sqlx::query("DELETE FROM sessions WHERE id = ?")
            .bind(session_id)
            .execute(&self.pool)
            .await?;
        Ok(result.rows_affected() > 0)
    }

    pub(crate) async fn list_messages(
        &self,
        session_id: &str,
    ) -> anyhow::Result<Vec<MessageWithParts>> {
        let rows = sqlx::query(
            "SELECT message_json FROM messages WHERE session_id = ? ORDER BY position ASC, created ASC",
        )
        .bind(session_id)
        .fetch_all(&self.pool)
        .await?;
        rows.into_iter()
            .map(|row| decode_json(row.get::<String, _>("message_json")))
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
        let mut query = sqlx::query(&sql).bind(session_id);
        if let Some(position) = cursor_position {
            query = query.bind(position);
        }
        let rows = query.fetch_all(&self.pool).await?;
        rows.into_iter()
            .map(|row| decode_json(row.get::<String, _>("message_json")))
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
        let by_message_id: Option<i64> = sqlx::query_scalar(
            "SELECT position FROM messages WHERE session_id = ? AND id = ? LIMIT 1",
        )
        .bind(session_id)
        .bind(cursor)
        .fetch_optional(&self.pool)
        .await?;
        if by_message_id.is_some() {
            return Ok(by_message_id);
        }
        let pattern = format!("%\"id\":\"{}\"%", escape_like(cursor));
        let by_part_id: Option<i64> = sqlx::query_scalar(
            "SELECT position FROM messages WHERE session_id = ? AND message_json LIKE ? ESCAPE '\\' \
             ORDER BY position ASC LIMIT 1",
        )
        .bind(session_id)
        .bind(pattern)
        .fetch_optional(&self.pool)
        .await?;
        Ok(by_part_id)
    }

    pub(crate) async fn append_message(
        &self,
        session_id: &str,
        message: &MessageWithParts,
    ) -> anyhow::Result<()> {
        let position: i64 = sqlx::query_scalar(
            "SELECT COALESCE(MAX(position), -1) + 1 FROM messages WHERE session_id = ?",
        )
        .bind(session_id)
        .fetch_one(&self.pool)
        .await?;
        sqlx::query(
            "INSERT INTO messages (id, session_id, message_json, created, position) VALUES (?, ?, ?, ?, ?)",
        )
        .bind(message_id(message))
        .bind(session_id)
        .bind(serde_json::to_string(message)?)
        .bind(sqlite_i64(message_created(message)))
        .bind(position)
        .execute(&self.pool)
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
        let row = sqlx::query(
            "SELECT message_json FROM messages WHERE session_id = ? AND id = ?",
        )
        .bind(session_id)
        .bind(message_id)
        .fetch_optional(&self.pool)
        .await?;
        row.map(|row| decode_json(row.get::<String, _>("message_json")))
            .transpose()
    }

    pub(crate) async fn update_message(
        &self,
        session_id: &str,
        message: &MessageWithParts,
    ) -> anyhow::Result<bool> {
        let result = sqlx::query(
            "UPDATE messages SET message_json = ? WHERE session_id = ? AND id = ?",
        )
        .bind(serde_json::to_string(message)?)
        .bind(session_id)
        .bind(message_id(message))
        .execute(&self.pool)
        .await?;
        Ok(result.rows_affected() > 0)
    }

    pub(crate) async fn delete_message(
        &self,
        session_id: &str,
        message_id: &str,
    ) -> anyhow::Result<bool> {
        let result = sqlx::query("DELETE FROM messages WHERE session_id = ? AND id = ?")
            .bind(session_id)
            .bind(message_id)
            .execute(&self.pool)
            .await?;
        Ok(result.rows_affected() > 0)
    }

    /// Remove every transcript message for a session. Used by session import to
    /// make re-importing a bundle idempotent (the prior transcript is replaced
    /// rather than appended to).
    pub(crate) async fn delete_session_messages(
        &self,
        session_id: &str,
    ) -> anyhow::Result<usize> {
        let result = sqlx::query("DELETE FROM messages WHERE session_id = ?")
            .bind(session_id)
            .execute(&self.pool)
            .await?;
        Ok(result.rows_affected() as usize)
    }

    pub(crate) async fn list_permission_approvals(
        &self,
    ) -> anyhow::Result<HashMap<String, Vec<PermissionRule>>> {
        let rows = sqlx::query("SELECT project_id, rules_json FROM permission_approvals")
            .fetch_all(&self.pool)
            .await?;
        rows.into_iter()
            .map(|row| {
                Ok((
                    row.get::<String, _>("project_id"),
                    decode_json(row.get::<String, _>("rules_json"))?,
                ))
            })
            .collect()
    }

    pub(crate) async fn save_permission_approvals(
        &self,
        project_id: &str,
        rules: &[PermissionRule],
    ) -> anyhow::Result<()> {
        sqlx::query(
            r#"
            INSERT INTO permission_approvals (project_id, rules_json, updated)
            VALUES (?, ?, ?)
            ON CONFLICT(project_id) DO UPDATE SET
                rules_json = excluded.rules_json,
                updated = excluded.updated
            "#,
        )
        .bind(project_id)
        .bind(serde_json::to_string(rules)?)
        .bind(sqlite_i64(crate::now_millis()))
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    pub(crate) async fn enqueue_prompt(
        &self,
        session_id: &str,
        request: &PromptRequest,
    ) -> anyhow::Result<usize> {
        let position: i64 = sqlx::query_scalar(
            "SELECT COALESCE(MAX(position), -1) + 1 FROM prompt_queue WHERE session_id = ?",
        )
        .bind(session_id)
        .fetch_one(&self.pool)
        .await?;
        sqlx::query(
            "INSERT INTO prompt_queue (id, session_id, position, request_json, created) VALUES (?, ?, ?, ?, ?)",
        )
        .bind(Id::ascending(IdKind::Event).to_string())
        .bind(session_id)
        .bind(position)
        .bind(serde_json::to_string(request)?)
        .bind(sqlite_i64(crate::now_millis()))
        .execute(&self.pool)
        .await?;
        self.queued_prompt_count(session_id).await
    }

    pub(crate) async fn list_queued_prompts(
        &self,
        session_id: &str,
    ) -> anyhow::Result<Vec<PromptRequest>> {
        let rows = sqlx::query(
            "SELECT request_json FROM prompt_queue WHERE session_id = ? ORDER BY position ASC, created ASC",
        )
        .bind(session_id)
        .fetch_all(&self.pool)
        .await?;
        rows.into_iter()
            .map(|row| decode_json(row.get::<String, _>("request_json")))
            .collect()
    }

    pub(crate) async fn queued_prompt_count(
        &self,
        session_id: &str,
    ) -> anyhow::Result<usize> {
        let count: i64 =
            sqlx::query_scalar("SELECT COUNT(*) FROM prompt_queue WHERE session_id = ?")
                .bind(session_id)
                .fetch_one(&self.pool)
                .await?;
        Ok(count.max(0) as usize)
    }

    pub(crate) async fn pop_queued_prompt(
        &self,
        session_id: &str,
    ) -> anyhow::Result<Option<PromptRequest>> {
        let Some(row) = sqlx::query(
            "SELECT id, request_json FROM prompt_queue WHERE session_id = ? ORDER BY position ASC, created ASC LIMIT 1",
        )
        .bind(session_id)
        .fetch_optional(&self.pool)
        .await?
        else {
            return Ok(None);
        };
        let id = row.get::<String, _>("id");
        let request = decode_json(row.get::<String, _>("request_json"))?;
        sqlx::query("DELETE FROM prompt_queue WHERE id = ?")
            .bind(id)
            .execute(&self.pool)
            .await?;
        Ok(Some(request))
    }

    pub(crate) async fn clear_queued_prompts(
        &self,
        session_id: &str,
    ) -> anyhow::Result<usize> {
        let result = sqlx::query("DELETE FROM prompt_queue WHERE session_id = ?")
            .bind(session_id)
            .execute(&self.pool)
            .await?;
        Ok(result.rows_affected() as usize)
    }

    pub(crate) async fn queued_session_ids(&self) -> anyhow::Result<Vec<String>> {
        let rows = sqlx::query(
            "SELECT DISTINCT session_id FROM prompt_queue ORDER BY session_id",
        )
        .fetch_all(&self.pool)
        .await?;
        Ok(rows
            .into_iter()
            .map(|row| row.get::<String, _>("session_id"))
            .collect())
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
        sqlx::query(
            "INSERT INTO events (event_id, kind, aggregate_id, aggregate_seq, owner_id, session_id, event_json, created) VALUES (?, ?, ?, ?, ?, ?, ?, ?)",
        )
        .bind(event.id.to_string())
        .bind(&event.kind)
        .bind(&aggregate_id)
        .bind(aggregate_seq)
        .bind(owner_id.map(ToOwned::to_owned))
        .bind(session_id)
        .bind(serde_json::to_string(event)?)
        .bind(sqlite_i64(crate::now_millis()))
        .execute(&self.pool)
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
        sqlx::query(
            r#"
            INSERT INTO event_sequences (aggregate_id, seq, owner_id)
            VALUES (?, ?, ?)
            ON CONFLICT(aggregate_id) DO UPDATE SET
                seq = excluded.seq,
                owner_id = COALESCE(event_sequences.owner_id, excluded.owner_id)
            "#,
        )
        .bind(aggregate_id)
        .bind(next)
        .bind(owner)
        .execute(&self.pool)
        .await?;
        Ok(next)
    }

    pub(crate) async fn aggregate_sequence(
        &self,
        aggregate_id: &str,
    ) -> anyhow::Result<Option<AggregateSequence>> {
        let row = sqlx::query(
            "SELECT seq, owner_id FROM event_sequences WHERE aggregate_id = ?",
        )
        .bind(aggregate_id)
        .fetch_optional(&self.pool)
        .await?;
        Ok(row.map(|row| AggregateSequence {
            seq: row.get::<i64, _>("seq"),
            owner_id: row.get::<Option<String>, _>("owner_id"),
        }))
    }

    pub(crate) async fn claim_aggregate_owner(
        &self,
        aggregate_id: &str,
        owner_id: &str,
    ) -> anyhow::Result<()> {
        sqlx::query(
            r#"
            INSERT INTO event_sequences (aggregate_id, seq, owner_id)
            VALUES (?, -1, ?)
            ON CONFLICT(aggregate_id) DO UPDATE SET owner_id = excluded.owner_id
            "#,
        )
        .bind(aggregate_id)
        .bind(owner_id)
        .execute(&self.pool)
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
            sqlx::query(
                "SELECT seq, aggregate_id, aggregate_seq, owner_id, event_json FROM events WHERE seq > ? AND session_id = ? ORDER BY seq ASC LIMIT ?",
            )
            .bind(since)
            .bind(session_id)
            .bind(limit)
            .fetch_all(&self.pool)
            .await?
        } else {
            sqlx::query("SELECT seq, aggregate_id, aggregate_seq, owner_id, event_json FROM events WHERE seq > ? ORDER BY seq ASC LIMIT ?")
                .bind(since)
                .bind(limit)
                .fetch_all(&self.pool)
                .await?
        };
        rows.into_iter()
            .map(|row| {
                Ok(PersistedEvent {
                    seq: row.get::<i64, _>("seq"),
                    aggregate_id: row.get::<String, _>("aggregate_id"),
                    aggregate_seq: row.get::<i64, _>("aggregate_seq"),
                    owner_id: row.get::<Option<String>, _>("owner_id"),
                    payload: decode_json(row.get::<String, _>("event_json"))?,
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
        sqlx::query(
            r#"
            INSERT INTO session_runs (id, session_id, status, created, updated, error_json)
            VALUES (?, ?, 'running', ?, ?, NULL)
            ON CONFLICT(id) DO UPDATE SET
                status = 'running',
                updated = excluded.updated,
                error_json = NULL
            "#,
        )
        .bind(run_id)
        .bind(session_id)
        .bind(now)
        .bind(now)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    pub(crate) async fn finish_run(
        &self,
        run_id: &str,
        status: &str,
        error: Option<Value>,
    ) -> anyhow::Result<()> {
        sqlx::query(
            r#"
            UPDATE session_runs
            SET status = ?, updated = ?, error_json = ?
            WHERE id = ?
            "#,
        )
        .bind(status)
        .bind(sqlite_i64(crate::now_millis()))
        .bind(error.map(|value| value.to_string()))
        .bind(run_id)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    pub(crate) async fn interrupt_stale_runs(&self) -> anyhow::Result<u64> {
        let result = sqlx::query(
            r#"
            UPDATE session_runs
            SET status = 'interrupted',
                updated = ?,
                error_json = ?
            WHERE status IN ('running', 'retry')
            "#,
        )
        .bind(sqlite_i64(crate::now_millis()))
        .bind(json!({ "message": "Server restarted before run completed" }).to_string())
        .execute(&self.pool)
        .await?;
        Ok(result.rows_affected())
    }

    #[cfg(test)]
    pub(crate) async fn close(&self) {
        self.pool.close().await;
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

/// Flatten a message into `(role, created, searchable text)` for the FTS
/// index. Parts are inspected as JSON so new part variants degrade to
/// "not indexed" instead of breaking compilation or persistence.
fn fts_document(message: &MessageWithParts) -> (String, u64, String) {
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
