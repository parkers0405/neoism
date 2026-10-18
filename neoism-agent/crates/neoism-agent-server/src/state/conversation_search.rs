//! Durable, eventually consistent lexical search. This module owns only a disposable
//! projection: canonical writes must append the dirty/delete statements to their transaction.
//! No triggers, embeddings, startup history scan, or SessionStore ownership cycle.
use super::*;
use std::collections::{HashMap, HashSet};
use std::time::Duration;

const BATCH: usize = 16;
// Raw canonical JSON summed across prepared documents. Guard parameters retain
// two copies; Db's bounded retry may transiently clone those once more.
const BATCH_SOURCE_BYTES: usize = 2 * 1024 * 1024;
const BODY_BYTES: usize = 64 * 1024;
const SETTLE_MS: u64 = 500;
const PREFIX: usize = 64;
const MAX_BOOST: f64 = 1.25;
const MAX_RECENCY_BOOST: f64 = 0.05;
const RECENCY_WINDOW_MS: u64 = 30 * 24 * 60 * 60 * 1000;
const MAX_CANDIDATES: usize = 16_384;
const MAX_SESSIONS: usize = 16_384;
const MAX_ANCESTORS: usize = 128;
const SEARCH_TIMEOUT: Duration = Duration::from_secs(2);
const TOO_BROAD: &str = "Conversation search is too broad; refine the query or select a conversation/directory and retry";

#[derive(Default)]
struct SearchBudget {
    candidates: usize,
    payloads: usize,
    max_candidates: Option<usize>, // tests can exercise exhaustion cheaply
}

// Process-global is intentionally stricter than a per-store limit: two searches
// total, with immediate backpressure rather than queued foreground work.
fn search_slots(_db: &Db) -> Arc<tokio::sync::Semaphore> {
    static SLOTS: std::sync::OnceLock<Arc<tokio::sync::Semaphore>> =
        std::sync::OnceLock::new();
    SLOTS
        .get_or_init(|| Arc::new(tokio::sync::Semaphore::new(2)))
        .clone()
}
const MIGRATIONS: [(&str, &str); 2] = [
    ("conversation-search-sessions-v1", "sessions"),
    ("conversation-search-messages-v1", "messages"),
];

pub(super) struct SearchWorker {
    cancel: tokio::sync::watch::Sender<bool>,
    task: tokio::sync::Mutex<Option<tokio::task::JoinHandle<()>>>,
}

impl SearchWorker {
    /// Idempotent, including concurrent callers. Cancellation is observed at
    /// transaction boundaries; never abort a Turso commit/rollback future.
    /// The remaining work is one bounded projection batch (16 documents / 2 MiB
    /// source JSON, or one oversized document) or a bounded backfill unit.
    /// Db bounds busy retries; shutdown never drains the rest of history.
    pub(super) async fn shutdown(&self) {
        let _ = self.cancel.send(true);
        let mut task = self.task.lock().await;
        if let Some(task) = task.as_mut() {
            if let Err(error) = task.await {
                tracing::warn!(%error, "conversation search worker stopped unexpectedly");
            }
        }
        *task = None;
    }
}

impl Drop for SearchWorker {
    fn drop(&mut self) {
        // A detached task finishes its current transaction and observes this
        // signal before its next document. It owns only Db, never SessionStore.
        let _ = self.cancel.send(true);
    }
}

pub(super) fn spawn(db: Db) -> SearchWorker {
    let (cancel, mut receiver) = tokio::sync::watch::channel(false);
    let task = tokio::spawn(async move {
        let mut delay = Duration::from_millis(100);
        loop {
            tokio::select! {
                _ = tokio::time::sleep(delay) => {},
                _ = receiver.changed() => break,
            }
            if *receiver.borrow() {
                break;
            }
            delay = match tick_inner(&db, false, Some(&receiver)).await {
                Ok(true) => Duration::from_millis(100),
                Ok(false) => Duration::from_secs(1),
                Err(error) => {
                    tracing::warn!(%error, "conversation search projection retrying");
                    Duration::from_secs(1)
                }
            };
            tokio::task::yield_now().await;
        }
    });
    SearchWorker {
        cancel,
        task: tokio::sync::Mutex::new(Some(task)),
    }
}

pub(super) async fn migrate(db: &Db) -> anyhow::Result<()> {
    db.execute_transaction(vec![
        ("CREATE TABLE IF NOT EXISTS conversation_search_docs (key TEXT PRIMARY KEY, session_id TEXT NOT NULL, message_id TEXT NOT NULL, role TEXT NOT NULL, created INTEGER NOT NULL, title TEXT NOT NULL, body TEXT NOT NULL)".into(), vec![]),
        ("CREATE INDEX IF NOT EXISTS conversation_search_session ON conversation_search_docs(session_id)".into(), vec![]),
        ("CREATE TABLE IF NOT EXISTS conversation_search_dirty (key TEXT PRIMARY KEY, session_id TEXT NOT NULL, message_id TEXT NOT NULL, revision TEXT NOT NULL, ready_at INTEGER NOT NULL)".into(), vec![]),
        ("CREATE INDEX IF NOT EXISTS conversation_search_ready ON conversation_search_dirty(ready_at, key)".into(), vec![]),
        ("CREATE INDEX IF NOT EXISTS conversation_search_dirty_session ON conversation_search_dirty(session_id)".into(), vec![]),
        // See turso_core 0.6.1 index_method/fts.rs: patterns 0..6 and field_weights.
        ("CREATE INDEX IF NOT EXISTS conversation_search_fts ON conversation_search_docs USING fts (title, body) WITH (weights='title=3.0,body=1.0')".into(), vec![]),
    ]).await?;
    for (name, _) in MIGRATIONS {
        db.execute("INSERT OR IGNORE INTO data_migrations(name, state, cursor_id, rows_done, updated_at) VALUES (?, 'pending', '', 0, ?)", vec![text(name), int(store_i64(crate::now_millis()))]).await?;
    }
    Ok(())
}

/// A new random revision on *every* edit prevents ABA (including delete/recreate).
/// Enqueue after the canonical write, in the same transaction, not before it.
pub(super) fn dirty_message(id: &str) -> (String, Vec<SqlValue>) {
    (
        "INSERT INTO conversation_search_dirty(key, session_id, message_id, revision, ready_at) SELECT 'message:' || id, session_id, id, lower(hex(randomblob(16))), ? FROM messages WHERE id = ? ON CONFLICT(key) DO UPDATE SET session_id=excluded.session_id, message_id=excluded.message_id, revision=excluded.revision, ready_at=excluded.ready_at".into(),
        vec![int(store_i64(crate::now_millis().saturating_add(SETTLE_MS))), text(id)],
    )
}

/// Renames touch one title document, never the conversation's entire transcript.
pub(super) fn dirty_session(id: &str) -> (String, Vec<SqlValue>) {
    (
        "INSERT INTO conversation_search_dirty(key, session_id, message_id, revision, ready_at) SELECT 'session:' || id, id, '', lower(hex(randomblob(16))), ? FROM sessions WHERE id = ? ON CONFLICT(key) DO UPDATE SET revision=excluded.revision, ready_at=excluded.ready_at".into(),
        vec![int(store_i64(crate::now_millis().saturating_add(SETTLE_MS))), text(id)],
    )
}

pub(super) fn delete_message(id: &str) -> Vec<(String, Vec<SqlValue>)> {
    vec![
        (
            "DELETE FROM conversation_search_docs WHERE key = ?".into(),
            vec![text(format!("message:{id}"))],
        ),
        (
            "DELETE FROM conversation_search_dirty WHERE key = ?".into(),
            vec![text(format!("message:{id}"))],
        ),
    ]
}

/// Call for every canonical session deleted (including descendant cascades).
/// This works before OR after removing canonical messages.
pub(super) fn delete_session(id: &str) -> Vec<(String, Vec<SqlValue>)> {
    vec![
        (
            "DELETE FROM conversation_search_docs WHERE session_id = ?".into(),
            vec![text(id)],
        ),
        (
            "DELETE FROM conversation_search_dirty WHERE session_id = ?".into(),
            vec![text(id)],
        ),
    ]
}

/// At most BATCH primary-key seeks/rows, with cursor advancement and enqueues
/// committed together. INSERT OR IGNORE never supersedes a live dirty revision.
async fn backfill(db: &Db) -> anyhow::Result<bool> {
    for (name, table) in MIGRATIONS {
        let row = db
            .fetch_optional(
                "SELECT state, cursor_id FROM data_migrations WHERE name = ?",
                vec![text(name)],
            )
            .await?
            .context("missing conversation search migration")?;
        if row.get_str("state")? == "complete" {
            continue;
        }
        let cursor = row.get_opt_str("cursor_id")?.unwrap_or_default();
        let rows = db
            .fetch_all(
                &format!("SELECT id FROM {table} WHERE id > ? ORDER BY id LIMIT ?"),
                vec![text(&cursor), int(BATCH as i64)],
            )
            .await?;
        let mut statements = Vec::with_capacity(rows.len() + 1);
        for row in &rows {
            let id = row.get_str("id")?;
            let (source, key, message) = if table == "messages" {
                ("session_id", "message:", "id")
            } else {
                ("id", "session:", "''")
            };
            // Live writes may already have been projected while backfill was
            // waiting. Do not commit the same FTS document a second time.
            statements.push((format!("INSERT OR IGNORE INTO conversation_search_dirty(key, session_id, message_id, revision, ready_at) SELECT '{key}' || id, {source}, {message}, lower(hex(randomblob(16))), 0 FROM {table} WHERE id = ? AND NOT EXISTS (SELECT 1 FROM conversation_search_docs d WHERE d.key = '{key}' || {table}.id)"), vec![text(id)]));
        }
        let last = rows
            .last()
            .map(|row| row.get_str("id"))
            .transpose()?
            .unwrap_or_else(|| cursor.clone());
        statements.push(("UPDATE data_migrations SET state=?, cursor_id=?, rows_done=rows_done+?, updated_at=?, error=NULL WHERE name=? AND COALESCE(cursor_id, '')=?".into(), vec![text(if rows.len() < BATCH { "complete" } else { "running" }), text(last), int(rows.len() as i64), int(store_i64(crate::now_millis())), text(name), text(cursor)]));
        db.execute_transaction(statements).await?;
        return Ok(true);
    }
    Ok(false)
}

// Same text/reasoning/completed-tool semantics as search_document, but cap
// *during* flattening: a large output must not create another unbounded copy.
fn bounded_document(message: &MessageWithParts) -> (String, u64, String) {
    use neoism_agent_core::{Part, ToolState};
    let role = match &message.info {
        MessageInfo::User(_) => "user",
        MessageInfo::Assistant(_) => "assistant",
    };
    let mut body = String::with_capacity(BODY_BYTES);
    for part in &message.parts {
        let (chunk, tool) = match part {
            Part::Text(part) => (part.text.as_str(), false),
            Part::Reasoning(part) => (part.text.as_str(), false),
            Part::Tool(part) => match &part.state {
                ToolState::Completed { output, .. } => (output.as_str(), true),
                _ => continue,
            },
            _ => continue,
        };
        let chunk = chunk.trim();
        if chunk.is_empty() {
            continue;
        }
        if !body.is_empty() && body.len() < BODY_BYTES {
            body.push('\n');
        }
        for ch in chunk.chars().take(if tool { 2000 } else { usize::MAX }) {
            if body.len() + ch.len_utf8() > BODY_BYTES {
                return (role.into(), message_created(message), body);
            }
            body.push(ch);
        }
        if body.len() >= BODY_BYTES {
            break;
        }
    }
    (role.into(), message_created(message), body)
}

fn truncate_body(mut body: String) -> String {
    if body.len() > BODY_BYTES {
        let mut end = BODY_BYTES;
        while !body.is_char_boundary(end) {
            end -= 1;
        }
        body.truncate(end);
    }
    body
}

const PROJECTION_INSERT: &str = "INSERT INTO conversation_search_docs(key, session_id, message_id, role, created, title, body) ";
const PROJECTION_UPDATE: &str = " ON CONFLICT(key) DO UPDATE SET session_id=excluded.session_id, message_id=excluded.message_id, role=excluded.role, created=excluded.created, title=excluded.title, body=excluded.body";
const PROJECTION_DELETE: &str = "DELETE FROM conversation_search_docs WHERE ";

/// Process one observed snapshot. Both the revision and canonical JSON are
/// revalidated by SQL *inside* the writer transaction, never via a prior read.
/// An old task cannot overwrite a newer edit or resurrect a synchronous delete.
fn projection_statements(row: DbRow) -> anyhow::Result<Vec<(String, Vec<SqlValue>)>> {
    let key = row.get_str("key")?;
    let revision = row.get_str("revision")?;
    let session = row.get_str("session_id")?;
    let message = row.get_str("message_id")?;
    let source = row.get_opt_str("source")?;
    let (table, column, id) = if message.is_empty() {
        ("sessions", "info_json", session.as_str())
    } else {
        ("messages", "message_json", message.as_str())
    };
    // Invalid legacy payloads are quarantined by dropping their projection;
    // another canonical edit queues a fresh attempt instead of poisoning a batch.
    let document = source.as_deref().and_then(|source| {
        if message.is_empty() {
            serde_json::from_str::<SessionInfo>(source)
                .map(|info| {
                    (
                        "title".to_string(),
                        info.time.created,
                        truncate_body(info.title),
                        String::new(),
                    )
                })
                .ok()
        } else {
            serde_json::from_str::<MessageWithParts>(source)
                .map(|message| {
                    let (role, created, body) = bounded_document(&message);
                    (role, created, String::new(), body)
                })
                .ok()
        }
    });
    let mut statements = Vec::with_capacity(2);
    let valid = format!("EXISTS (SELECT 1 FROM conversation_search_dirty WHERE key=? AND revision=?) AND EXISTS (SELECT 1 FROM {table} WHERE id=? AND {column}=?)");
    if let Some((role, created, title, body)) = document {
        statements.push((format!("{PROJECTION_INSERT}SELECT ?, ?, ?, ?, ?, ?, ? WHERE {valid}{PROJECTION_UPDATE}"), vec![text(&key), text(&session), text(&message), text(role), int(store_i64(created)), text(title), text(body), text(&key), text(&revision), text(id), text(source.clone().unwrap_or_default())]));
    } else {
        if source.is_some() {
            tracing::warn!(%key, "skipping invalid canonical conversation search document");
        }
        statements.push((format!("{PROJECTION_DELETE}key=? AND EXISTS (SELECT 1 FROM conversation_search_dirty WHERE key=? AND revision=?)"), vec![text(&key), text(&key), text(&revision)]));
    }
    // If the canonical JSON changed without changing the queue token, retry it.
    // Usually the token predicate alone handles this; the extra guard also makes
    // backfill and accidental out-of-band writers safe.
    statements.push((format!("DELETE FROM conversation_search_dirty WHERE key=? AND revision=? AND (NOT EXISTS (SELECT 1 FROM {table} WHERE id=?) OR EXISTS (SELECT 1 FROM {table} WHERE id=? AND {column}=?))"), vec![text(key), text(revision), text(id), text(id), source.map(text).unwrap_or(SqlValue::Null)]));
    Ok(statements)
}

// FtsCursor::drop flushes Tantivy when Connection::execute drops its statement,
// even within a larger SQL transaction. One multi-row UPSERT is essential: a
// transaction containing sixteen separate UPSERTs still churns native segments.
// Preserve each SELECT's nonce/canonical predicates and parameter order verbatim.
fn combine_projection_statements(
    statements: Vec<(String, Vec<SqlValue>)>,
) -> Vec<(String, Vec<SqlValue>)> {
    let mut selects = Vec::new();
    let mut insert_params = Vec::new();
    let mut deletes = Vec::new();
    let mut delete_params = Vec::new();
    let mut cleanup = Vec::new();
    for (sql, params) in statements {
        if let Some(select) = sql
            .strip_prefix(PROJECTION_INSERT)
            .and_then(|sql| sql.strip_suffix(PROJECTION_UPDATE))
        {
            selects.push(select.to_string());
            insert_params.extend(params);
        } else if let Some(predicate) = sql.strip_prefix(PROJECTION_DELETE) {
            deletes.push(format!("({predicate})"));
            delete_params.extend(params);
        } else {
            cleanup.push((sql, params));
        }
    }
    let mut batch = Vec::with_capacity(cleanup.len() + 2);
    if !selects.is_empty() {
        batch.push((
            format!(
                "{PROJECTION_INSERT}{}{PROJECTION_UPDATE}",
                selects.join(" UNION ALL ")
            ),
            insert_params,
        ));
    }
    if !deletes.is_empty() {
        batch.push((
            format!("{PROJECTION_DELETE}{}", deletes.join(" OR ")),
            delete_params,
        ));
    }
    batch.extend(cleanup);
    batch
}

#[cfg(test)]
async fn project(db: &Db, row: DbRow) -> anyhow::Result<()> {
    db.execute_transaction(projection_statements(row)?).await
}

fn projection_source_bytes(row: &DbRow) -> usize {
    row.columns
        .iter()
        .position(|column| column == "source")
        .and_then(|index| row.values.get(index))
        .map(|value| match value {
            SqlValue::Text(source) => source.len(),
            _ => 0,
        })
        .unwrap_or(0)
}

#[cfg(test)]
async fn tick(db: &Db, force: bool) -> anyhow::Result<bool> {
    tick_inner(db, force, None).await
}

async fn tick_inner(
    db: &Db,
    force: bool,
    cancel: Option<&tokio::sync::watch::Receiver<bool>>,
) -> anyhow::Result<bool> {
    let cancelled = || cancel.is_some_and(|cancel| *cancel.borrow());
    if cancelled() {
        return Ok(false);
    }
    let rows = db.fetch_all("SELECT key, revision FROM conversation_search_dirty WHERE ready_at <= ? ORDER BY ready_at, key LIMIT ?", vec![int(if force { i64::MAX } else { store_i64(crate::now_millis()) }), int(BATCH as i64)]).await?;
    let count = rows.len();
    let mut statements = Vec::with_capacity(BATCH * 2);
    let mut source_bytes = 0usize;
    for row in rows {
        if cancelled() {
            return Ok(false);
        }
        // Load one canonical payload at a time, not sixteen potentially huge
        // histories in fetch_all. An intervening edit simply skips this token.
        if let Some(snapshot) = db.fetch_optional("SELECT q.key, q.session_id, q.message_id, q.revision, CASE WHEN q.message_id='' THEN s.info_json ELSE m.message_json END AS source FROM conversation_search_dirty q LEFT JOIN sessions s ON s.id=q.session_id LEFT JOIN messages m ON m.id=q.message_id WHERE q.key=? AND q.revision=?", vec![text(row.get_str("key")?), text(row.get_str("revision")?)]).await? {
            if cancelled() { return Ok(false); }
            let bytes = projection_source_bytes(&snapshot);
            if !statements.is_empty() && source_bytes.saturating_add(bytes) > BATCH_SOURCE_BYTES {
                // This token stays queued. Do not retain its source alongside
                // an already full batch; oversized single documents run alone.
                break;
            }
            source_bytes = source_bytes.saturating_add(bytes);
            statements.extend(projection_statements(snapshot)?);
            if source_bytes >= BATCH_SOURCE_BYTES { break; }
        }
        tokio::task::yield_now().await;
    }
    if cancelled() {
        return Ok(false);
    }
    if !statements.is_empty() {
        // One writer transaction and one native UPSERT cursor per batch,
        // plus at most one deletion cursor for invalid legacy documents.
        // Never select! or abort around an active commit.
        db.execute_transaction(combine_projection_statements(statements))
            .await?;
    }
    if cancelled() {
        return Ok(false);
    }
    // Live work has priority; cold history advances only when capacity remains.
    let backfilled = if count < BATCH {
        backfill(db).await?
    } else {
        false
    };
    Ok(count > 0 || backfilled)
}

/// Constant-size status probes; never COUNT/scan canonical history.
pub(super) async fn indexing(db: &Db) -> anyhow::Result<bool> {
    if db
        .fetch_optional("SELECT key FROM conversation_search_dirty LIMIT 1", vec![])
        .await?
        .is_some()
    {
        return Ok(true);
    }
    Ok(db.fetch_optional("SELECT name FROM data_migrations WHERE name IN ('conversation-search-sessions-v1','conversation-search-messages-v1') AND state <> 'complete' LIMIT 1", vec![]).await?.is_some())
}

/// Tests only: finish persistent backfill and drain dirties without debounce.
/// Search/open never call this. The actual worker always uses bounded ticks.
#[cfg(test)]
pub(super) async fn flush(db: &Db) -> anyhow::Result<()> {
    while tick(db, true).await? {
        tokio::task::yield_now().await;
    }
    Ok(())
}

/// Match Tantivy's default SimpleTokenizer + lowercase + RemoveLongFilter.
/// Punctuation in code (including `_`, `::`, paths and parser metacharacters)
/// becomes token boundaries, never query syntax. Quoting also neutralizes AND,
/// OR, NOT and field names. Bound input before parser/query allocation.
fn terms(query: &str) -> Vec<String> {
    let mut seen = HashSet::new();
    query
        .chars()
        .take(4096)
        .collect::<String>()
        .split(|ch: char| !ch.is_alphanumeric())
        .filter(|word| !word.is_empty() && word.len() < 40)
        // Preserve case until Tantivy tokenizes, then lowercases. Lowercasing
        // here first can introduce combining marks (e.g. İ), changing tokens.
        .filter(|word| seen.insert(word.to_lowercase()))
        .map(str::to_string)
        .take(32)
        .collect()
}

fn plain_query(terms: &[String], conversations: bool) -> String {
    let all = terms
        .iter()
        .map(|term| format!("\"{term}\""))
        .collect::<Vec<_>>()
        .join(" AND ");
    if !conversations || terms.len() < 2 {
        return all;
    }
    let any = terms
        .iter()
        .map(|term| format!("\"{term}\""))
        .collect::<Vec<_>>()
        .join(" OR ");
    // Keep partial-query recall, but prefer full coverage and words appearing
    // together. Terms are sanitized alphanumerics, never raw parser syntax.
    format!("({any}) OR ({all})^2 OR \"{}\"^2", terms.join(" "))
}

fn excerpt(body: &str, query: &[String]) -> String {
    // Preserve original Unicode byte offsets even when lowercase expands a char.
    for (offset, word) in
        body.split_inclusive(|ch: char| !ch.is_alphanumeric())
            .scan(0, |offset, word| {
                let old = *offset;
                *offset += word.len();
                Some((old, word))
            })
    {
        let word = word.trim_end_matches(|ch: char| !ch.is_alphanumeric());
        let normalized = word.to_lowercase();
        if query.iter().any(|term| term.to_lowercase() == normalized) {
            return like_excerpt(body, offset, word.len());
        }
    }
    let end = body
        .char_indices()
        .nth(180)
        .map(|(i, _)| i)
        .unwrap_or(body.len());
    body[..end].replace('\n', " ")
}

#[derive(Clone)]
struct CanonicalSession {
    info: SessionInfo,
    allowed: bool,
}

async fn canonical(
    db: &Db,
    id: &str,
    cache: &mut HashMap<String, Option<CanonicalSession>>,
    scope: TenantQueryScope<'_>,
    claims: Option<&crate::caller::CallerClaims>,
) -> anyhow::Result<Option<CanonicalSession>> {
    if let Some(value) = cache.get(id) {
        return Ok(value.clone());
    }
    anyhow::ensure!(cache.len() < MAX_SESSIONS, "{TOO_BROAD}");
    // Push the cheap indexed tenant restriction into canonical SQL, before
    // decoding metadata or doing filesystem-backed authorization. Other policy
    // predicates remain exclusively owned by caller::allows_session.
    let tenant = match scope {
        TenantQueryScope::Tenant(tenant) => Some(tenant),
        TenantQueryScope::LocalAll => claims.map(|claims| claims.tenant_id.as_str()),
    };
    let (sql, params) = if let Some(tenant) = tenant {
        (
            format!(
                "SELECT tenant_id, info_json FROM sessions s WHERE id=?1 AND {}",
                tenant_predicate("s", "?2")
            ),
            vec![text(id), text(tenant)],
        )
    } else {
        (
            "SELECT tenant_id, info_json FROM sessions WHERE id=?".to_string(),
            vec![text(id)],
        )
    };
    let value = if let Some(row) = db.fetch_optional(&sql, params).await? {
        let mut info: SessionInfo = match serde_json::from_str(&row.get_str("info_json")?)
        {
            Ok(info) => info,
            Err(error) => {
                tracing::warn!(session_id=id, %error, "skipping invalid legacy search session");
                cache.insert(id.to_string(), None);
                return Ok(None);
            }
        };
        db.hydrate_host_associations(std::slice::from_mut(&mut info))
            .await?;
        let local_access = info
            .extra
            .get(crate::caller::HOST_LOCAL_ACCESS_KEY)
            .and_then(Value::as_bool)
            == Some(true);
        let scoped = match scope {
            TenantQueryScope::LocalAll => true,
            TenantQueryScope::Tenant(tenant) => {
                crate::caller::session_tenant(&info) == tenant
                    || (tenant == "local" && local_access)
            }
        };
        // Cache canonicalize-aware policy once per session, after authoritative
        // hosting overlays strip any untrusted local-access marker from JSON.
        let allowed = scoped
            && claims.is_none_or(|claims| crate::caller::allows_session(claims, &info));
        Some(CanonicalSession { info, allowed })
    } else {
        None
    };
    cache.insert(id.to_string(), value.clone());
    Ok(value)
}

fn admitted(
    session: &CanonicalSession,
    _scope: TenantQueryScope<'_>,
    _claims: Option<&crate::caller::CallerClaims>,
) -> bool {
    session.allowed
}

async fn root(
    db: &Db,
    session: CanonicalSession,
    cache: &mut HashMap<String, Option<CanonicalSession>>,
    scope: TenantQueryScope<'_>,
    claims: Option<&crate::caller::CallerClaims>,
) -> anyhow::Result<Option<CanonicalSession>> {
    let tenant = crate::caller::session_tenant(&session.info).to_string();
    let mut current = session;
    let mut visited = HashSet::new();
    loop {
        anyhow::ensure!(visited.len() < MAX_ANCESTORS, "{TOO_BROAD}");
        if !visited.insert(current.info.id.to_string())
            || !admitted(&current, scope, claims)
            || crate::caller::session_tenant(&current.info) != tenant
        {
            return Ok(None);
        }
        let Some(parent) = current.info.parent_id.as_ref() else {
            return Ok(Some(current));
        };
        let Some(parent) = canonical(db, parent.as_str(), cache, scope, claims).await?
        else {
            return Ok(None);
        };
        current = parent;
    }
}

// Deliberately exactly Turso's combined_ordered_limit pattern (fts.rs:1438).
// No JOIN/GROUP BY/DISTINCT/window in this SELECT: those can suppress FTS
// substitution, leaving scalar fts_match (OR-only!) or scalar fts_score=0.
const CANDIDATES: &str = "SELECT key, session_id, message_id, role, created, fts_score(title, body, ?1) AS score FROM conversation_search_docs WHERE fts_match(title, body, ?1) ORDER BY score DESC LIMIT ?2";

// Canonical SQL is an eligibility prefilter, never a replacement for hydrated
// caller authorization. Local access via a registered host namespace is only
// provisionally admitted here; the authoritative hydration checks paths/markers.
fn tenant_predicate(alias: &str, parameter: &str) -> String {
    format!("((({alias}.tenant_id={parameter} OR ({alias}.tenant_id='local' AND EXISTS (SELECT 1 FROM hosted_chat_sessions h WHERE h.session_id={alias}.id AND 'workspace:' || h.workspace_id={parameter}))) AND NOT EXISTS (SELECT 1 FROM hosted_chat_revocations r WHERE r.session_id={alias}.id AND 'workspace:' || r.workspace_id={parameter})) OR ({parameter}='local' AND (EXISTS (SELECT 1 FROM hosted_chat_revocations r WHERE r.session_id={alias}.id AND 'workspace:' || r.workspace_id={alias}.tenant_id) OR EXISTS (SELECT 1 FROM hosted_chat_directories d WHERE 'workspace:' || d.workspace_id={alias}.tenant_id))))")
}

// Do NOT attach these predicates to the native FTS SELECT: Turso 0.6.1
// otherwise drops TopDocs LIMIT pushdown and may allocate a million hits.
// This second stage performs bounded primary-key lookups (256 IDs per query).
fn eligibility_query(
    keys: &[String],
    scope: TenantQueryScope<'_>,
    claims: Option<&crate::caller::CallerClaims>,
    session_id: Option<&str>,
    conversations: bool,
) -> (String, Vec<SqlValue>) {
    let mut params: Vec<_> = keys.iter().map(text).collect();
    let tenant = match scope {
        TenantQueryScope::Tenant(tenant) => Some(tenant),
        TenantQueryScope::LocalAll => claims.map(|claims| claims.tenant_id.as_str()),
    };
    let tenant_clause = if let Some(tenant) = tenant {
        params.push(text(tenant));
        format!(
            " AND {}",
            tenant_predicate("s", &format!("?{}", params.len()))
        )
    } else {
        String::new()
    };
    let mut sql = format!("SELECT d.key FROM conversation_search_docs d WHERE d.key IN ({}) AND EXISTS (SELECT 1 FROM sessions s WHERE s.id=d.session_id{tenant_clause} AND (d.message_id='' OR EXISTS (SELECT 1 FROM messages m WHERE m.id=d.message_id AND m.session_id=s.id AND m.tenant_id=s.tenant_id)))", vec!["?";keys.len()].join(","));
    if !conversations {
        sql.push_str(" AND d.message_id<>''");
        if let Some(session) = session_id {
            params.push(text(session));
            sql.push_str(&format!(" AND d.session_id=?{}", params.len()));
        }
    }
    (sql, params)
}

async fn eligible_keys(
    db: &Db,
    candidates: &[DbRow],
    seen: &HashSet<String>,
    scope: TenantQueryScope<'_>,
    claims: Option<&crate::caller::CallerClaims>,
    session_id: Option<&str>,
    conversations: bool,
) -> anyhow::Result<HashSet<String>> {
    let keys = candidates
        .iter()
        .map(|row| row.get_str("key"))
        .collect::<anyhow::Result<Vec<_>>>()?;
    let keys: Vec<_> = keys.into_iter().filter(|key| !seen.contains(key)).collect();
    let mut eligible = HashSet::new();
    for chunk in keys.chunks(256) {
        let (sql, params) =
            eligibility_query(chunk, scope, claims, session_id, conversations);
        for row in db.fetch_all(&sql, params).await? {
            eligible.insert(row.get_str("key")?);
        }
        tokio::task::yield_now().await;
    }
    Ok(eligible)
}

/// Score-frontier retrieval, NOT a small message topK followed by deduplication.
/// All metadata and authorization are canonical and applied before counting hits.
/// With K admitted roots/messages, stop only if Kth score exceeds the last
/// raw score times the maximum role/recency boost, or the index is exhausted.
/// Strict > includes all boundary ties.
/// Prefixes double up to 16,384 candidates. If the frontier cannot be proved,
/// return an explicit refine-query error, NEVER silently truncate recall.
/// Title excerpts are valid; prefer a genuine message only when encountered
/// within the proved score frontier. No special exhaustive title-only walk.
#[allow(clippy::too_many_arguments)]
pub(super) async fn search(
    db: &Db,
    scope: TenantQueryScope<'_>,
    query: &str,
    session_id: Option<&str>,
    directory: Option<&str>,
    limit: usize,
    conversations: bool,
    claims: Option<&crate::caller::CallerClaims>,
) -> anyhow::Result<Vec<MessageSearchHit>> {
    let semaphore = search_slots(db);
    let _permit = semaphore
        .try_acquire_owned()
        .map_err(|_| anyhow::anyhow!("Conversation search is busy; retry shortly"))?;
    let mut budget = SearchBudget::default();
    tokio::time::timeout(
        SEARCH_TIMEOUT,
        search_inner(
            db,
            scope,
            query,
            session_id,
            directory,
            limit,
            conversations,
            claims,
            &mut budget,
        ),
    )
    .await
    .map_err(|_| anyhow::anyhow!("{TOO_BROAD} (2 second query budget exceeded)"))?
}

#[allow(clippy::too_many_arguments)]
async fn search_inner(
    db: &Db,
    scope: TenantQueryScope<'_>,
    query: &str,
    session_id: Option<&str>,
    directory: Option<&str>,
    limit: usize,
    conversations: bool,
    claims: Option<&crate::caller::CallerClaims>,
    budget: &mut SearchBudget,
) -> anyhow::Result<Vec<MessageSearchHit>> {
    if limit == 0 {
        return Ok(Vec::new());
    }
    let terms = terms(query);
    if terms.is_empty() {
        return Ok(Vec::new());
    }
    let query = plain_query(&terms, conversations);
    let now = crate::now_millis();
    let maximum_boost = MAX_BOOST
        * if conversations {
            1.0 + MAX_RECENCY_BOOST
        } else {
            1.0
        };
    let limit = limit.min(100);
    let cap = budget
        .max_candidates
        .unwrap_or(MAX_CANDIDATES)
        .clamp(1, MAX_CANDIDATES);
    let mut prefix = PREFIX.max(limit).min(cap);
    let mut sessions = HashMap::new();
    let mut seen = HashSet::new();
    let mut hits: HashMap<String, MessageSearchHit> = HashMap::new();
    // Group rank can come from a title, independently of the chosen transcript
    // excerpt. Do not compare later message excerpts against a boosted title.
    let mut excerpt_scores: HashMap<String, f64> = HashMap::new();
    let mut projection_keys: HashMap<String, String> = HashMap::new();
    loop {
        let candidates = db
            .fetch_all(
                CANDIDATES,
                vec![text(&query), int(prefix.min(i64::MAX as usize) as i64)],
            )
            .await?;
        let exhausted = candidates.len() < prefix;
        let frontier = candidates
            .last()
            .map(|row| row.get_f64("score"))
            .transpose()?
            .unwrap_or(0.0);
        let eligible = eligible_keys(
            db,
            &candidates,
            &seen,
            scope,
            claims,
            session_id,
            conversations,
        )
        .await?;
        for row in candidates {
            let projection_key = row.get_str("key")?;
            if !seen.insert(projection_key.clone()) {
                continue;
            }
            budget.candidates += 1;
            anyhow::ensure!(
                budget.candidates <= cap,
                "{TOO_BROAD} ({cap} candidate budget exceeded)"
            );
            if !eligible.contains(&projection_key) {
                continue;
            }
            let source_id = row.get_str("session_id")?;
            if !conversations && session_id.is_some_and(|id| id != source_id) {
                continue;
            }
            let message_id = row.get_str("message_id")?;
            if message_id.is_empty() && !conversations {
                continue;
            }
            let Some(source) =
                canonical(db, &source_id, &mut sessions, scope, claims).await?
            else {
                continue;
            };
            if !admitted(&source, scope, claims) {
                continue;
            }
            let result = if conversations {
                let Some(root) = root(db, source, &mut sessions, scope, claims).await?
                else {
                    continue;
                };
                root
            } else {
                source
            };
            if conversations && session_id.is_some_and(|id| id != result.info.id.as_str())
            {
                continue;
            }
            if directory.is_some_and(|directory| directory != result.info.directory) {
                continue;
            }
            let role = row.get_str("role")?;
            let raw_score = row.get_f64("score")?;
            anyhow::ensure!(
                raw_score.is_finite() && raw_score > 0.0,
                "FTS optimizer did not provide positive relevance"
            );
            // Fresh activity adds at most 5%, halving after 30 days, so a
            // substantially better older match still wins.
            let recency = if conversations {
                let age = now.saturating_sub(result.info.time.updated) as f64;
                1.0 + MAX_RECENCY_BOOST / (1.0 + age / RECENCY_WINDOW_MS as f64)
            } else {
                1.0
            };
            let score =
                raw_score * (if role == "user" { MAX_BOOST } else { 1.0 }) * recency;
            // Retain only the winning projection identity. Bodies are loaded
            // exactly once per FINAL result, never per admitted candidate.
            let hit = MessageSearchHit {
                session_id: result.info.id.to_string(),
                message_id,
                role,
                created: row.get_i64("created")?.max(0) as u64,
                excerpt: String::new(),
                score,
            };
            let key = if conversations {
                hit.session_id.clone()
            } else {
                hit.message_id.clone()
            };
            match hits.get_mut(&key) {
                None => {
                    excerpt_scores.insert(key.clone(), hit.score);
                    projection_keys.insert(key.clone(), projection_key.clone());
                    hits.insert(key, hit);
                }
                Some(old) => {
                    let best_score = old.score.max(hit.score);
                    let prefer_message =
                        old.message_id.is_empty() && !hit.message_id.is_empty();
                    let comparable =
                        old.message_id.is_empty() == hit.message_id.is_empty();
                    let old_excerpt_score =
                        excerpt_scores.get(&key).copied().unwrap_or(old.score);
                    if prefer_message
                        || (comparable
                            && (hit.score > old_excerpt_score
                                || (hit.score == old_excerpt_score
                                    && (hit.created > old.created
                                        || (hit.created == old.created
                                            && hit.message_id < old.message_id)))))
                    {
                        projection_keys.insert(key.clone(), projection_key);
                        excerpt_scores.insert(key, hit.score);
                        *old = hit;
                    }
                    old.score = best_score;
                }
            }
        }
        let mut ranked: Vec<_> = hits.values().cloned().collect();
        let latest_activity = |hit: &MessageSearchHit| {
            if conversations {
                sessions
                    .get(&hit.session_id)
                    .and_then(Option::as_ref)
                    .map(|session| session.info.time.updated)
                    .unwrap_or(hit.created)
            } else {
                hit.created
            }
        };
        ranked.sort_by(|a, b| {
            b.score
                .total_cmp(&a.score)
                .then_with(|| latest_activity(b).cmp(&latest_activity(a)))
                .then_with(|| b.created.cmp(&a.created))
                .then_with(|| a.session_id.cmp(&b.session_id))
                .then_with(|| a.message_id.cmp(&b.message_id))
        });
        let complete_frontier =
            ranked.len() >= limit && ranked[limit - 1].score > frontier * maximum_boost;
        if exhausted || complete_frontier {
            ranked.truncate(limit);
            let mut results = Vec::with_capacity(ranked.len());
            for mut hit in ranked {
                let identity = if conversations {
                    &hit.session_id
                } else {
                    &hit.message_id
                };
                let key = projection_keys
                    .get(identity)
                    .context("missing winning search projection")?;
                budget.payloads += 1;
                let Some(payload) = db
                    .fetch_optional(
                        "SELECT title, body FROM conversation_search_docs WHERE key=?",
                        vec![text(key)],
                    )
                    .await?
                else {
                    continue;
                };
                let content = if hit.role == "title" {
                    payload.get_str("title")?
                } else {
                    payload.get_str("body")?
                };
                hit.excerpt = excerpt(&content, &terms);
                results.push(hit);
            }
            return Ok(results);
        }
        anyhow::ensure!(
            prefix < cap,
            "{TOO_BROAD} ({cap} candidate budget exhausted before relevance frontier)"
        );
        let next = prefix.saturating_mul(2).min(cap);
        anyhow::ensure!(next > prefix, "conversation search prefix overflow");
        prefix = next;
        tokio::task::yield_now().await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // Separate databases still share the production process-global admission
    // slots. Coordinate whole test lifetimes (including reopen/after-drop reads)
    // so unrelated tests cannot consume slots or race the saturation assertion.
    // Do not change production admission, deadlines, or retry busy searches.
    static SEARCH_TEST: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

    async fn running_store() -> SessionStore {
        let path = std::env::temp_dir().join(format!(
            "neoism-conversation-search-{}.db",
            neoism_agent_core::new_session_id()
        ));
        let store = SessionStore::open(path).await.unwrap();
        migrate(&store.db).await.unwrap();
        store
    }

    async fn store() -> SessionStore {
        let store = running_store().await;
        // Keep deterministic flush/backfill/race tests independent of wall time.
        store._search_worker.as_ref().unwrap().shutdown().await;
        store
    }

    async fn session(
        db: &Db,
        id: &str,
        title: &str,
        directory: &str,
        parent: Option<&str>,
        tenant: &str,
    ) {
        let info = json!({"id":id,"slug":id,"projectId":"global","directory":directory,"parentId":parent,"title":title,"version":"test","time":{"created":1,"updated":1}, crate::caller::TENANT_EXTRA_KEY:tenant});
        db.execute_transaction(vec![("INSERT INTO sessions(id,tenant_id,info_json,updated) VALUES(?,?,?,1) ON CONFLICT(id) DO UPDATE SET info_json=excluded.info_json, tenant_id=excluded.tenant_id".into(), vec![text(id),text(tenant),text(info.to_string())]), dirty_session(id)]).await.unwrap();
    }

    async fn message(db: &Db, id: &str, session: &str, body: &str, tenant: &str) {
        let value = json!({"info":{"role":"user","id":id,"sessionId":session,"time":{"created":2},"agent":"build","model":{"providerId":"test","modelId":"test"}},"parts":[{"type":"text","id":format!("prt_{id}"),"sessionId":session,"messageId":id,"text":body}]});
        serde_json::from_value::<MessageWithParts>(value.clone()).unwrap();
        db.execute_transaction(vec![("INSERT INTO messages(id,tenant_id,session_id,message_json,created,position) VALUES(?,?,?,?,2,0) ON CONFLICT(id) DO UPDATE SET message_json=excluded.message_json".into(),vec![text(id),text(tenant),text(session),text(value.to_string())]),dirty_message(id)]).await.unwrap();
    }

    async fn find(
        db: &Db,
        query: &str,
        conversations: bool,
        limit: usize,
    ) -> Vec<MessageSearchHit> {
        search(
            db,
            TenantQueryScope::LocalAll,
            query,
            None,
            None,
            limit,
            conversations,
            None,
        )
        .await
        .unwrap()
    }

    async fn snapshot(db: &Db, id: &str) -> DbRow {
        db.fetch_optional("SELECT q.key,q.session_id,q.message_id,q.revision,m.message_json AS source FROM conversation_search_dirty q LEFT JOIN messages m ON m.id=q.message_id WHERE q.message_id=?", vec![text(id)]).await.unwrap().unwrap()
    }

    #[tokio::test]
    async fn indexed_plan_parser_and_and_or_semantics() {
        let _search_test = SEARCH_TEST.lock().await;
        let store = store().await;
        let db = &store.db;
        session(db, "ses_code", "unrelated", "/project", None, "local").await;
        message(
            db,
            "msg_both",
            "ses_code",
            "Fix Rust::borrow_checker café λambda safely",
            "local",
        )
        .await;
        message(db, "msg_one", "ses_code", "Rust alone", "local").await;
        flush(db).await.unwrap();
        let plan = db
            .fetch_all(
                &format!("EXPLAIN QUERY PLAN {CANDIDATES}"),
                vec![text("\"rust\" AND \"borrow\""), int(64)],
            )
            .await
            .unwrap();
        let details = plan
            .iter()
            .flat_map(|row| row.values.iter())
            .map(|v| format!("{v:?}"))
            .collect::<Vec<_>>()
            .join(" ");
        assert!(
            details.contains("QUERY INDEX METHOD fts"),
            "scalar fallback is not search: {details}"
        );
        let hits = find(db, "Rust::borrow_checker", false, 20).await;
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].message_id, "msg_both");
        assert!(hits[0].score > 0.0 && hits[0].excerpt.contains(">>"));
        assert!(find(db, "rust missing", false, 20).await.is_empty());
        assert_eq!(find(db, "rust missing", true, 20).await.len(), 1);
        assert_eq!(find(db, "CAFÉ", false, 20).await.len(), 1);
        for query in [
            "+ - : ( ) [ ] {} * ? ~ ^",
            "\"rust\" OR body:missing",
            "NOT rust",
            "🔥",
        ] {
            let _ = find(db, query, true, 20).await;
        }
        assert!(find(db, "🔥", false, 20).await.is_empty());
    }

    #[tokio::test]
    async fn grouping_and_filters_do_not_truncate_to_message_top_k() {
        let _search_test = SEARCH_TEST.lock().await;
        let store = store().await;
        let db = &store.db;
        session(db, "ses_a", "target", "/yes", None, "local").await;
        session(db, "ses_child", "child", "/yes", Some("ses_a"), "local").await;
        session(db, "ses_b", "other", "/yes", None, "local").await;
        session(db, "ses_denied", "target", "/no", None, "foreign").await;
        for i in 0..80 {
            message(
                db,
                &format!("msg_a{i:03}"),
                "ses_child",
                "target target target",
                "local",
            )
            .await;
        }
        message(
            db,
            "msg_b",
            "ses_b",
            "target amid much more text for a lower relevance document",
            "local",
        )
        .await;
        message(
            db,
            "msg_denied",
            "ses_denied",
            "target target target",
            "foreign",
        )
        .await;
        flush(db).await.unwrap();
        let hits = search(
            db,
            TenantQueryScope::Tenant("local"),
            "target",
            None,
            Some("/yes"),
            2,
            true,
            None,
        )
        .await
        .unwrap();
        assert_eq!(hits.len(), 2);
        assert!(hits.iter().any(|hit| hit.session_id == "ses_b"));
        assert!(!hits
            .iter()
            .find(|hit| hit.session_id == "ses_a")
            .unwrap()
            .message_id
            .is_empty());
        assert_eq!(find(db, "target", false, 100).await.len(), 82);
        session(db, "ses_b", "other", "/moved", None, "local").await;
        let hits = search(
            db,
            TenantQueryScope::Tenant("local"),
            "target",
            None,
            Some("/yes"),
            2,
            true,
            None,
        )
        .await
        .unwrap();
        assert_eq!(hits.len(), 1);
    }

    #[tokio::test]
    async fn revisions_and_synchronous_deletes_prevent_stale_resurrection() {
        let _search_test = SEARCH_TEST.lock().await;
        let store = store().await;
        let db = &store.db;
        session(db, "ses_edit", "oldtitle", "/project", None, "local").await;
        message(db, "msg_edit", "ses_edit", "oldword", "local").await;
        let old = snapshot(db, "msg_edit").await;
        message(db, "msg_edit", "ses_edit", "newword", "local").await;
        project(db, old).await.unwrap();
        flush(db).await.unwrap();
        assert!(find(db, "oldword", false, 10).await.is_empty());
        assert_eq!(find(db, "newword", false, 10).await.len(), 1);
        // Even an out-of-band writer that forgets to change the dirty revision
        // cannot have its canonical JSON replaced by an observed old snapshot.
        let dirty = dirty_message("msg_edit");
        db.execute(&dirty.0, dirty.1).await.unwrap();
        let stale = snapshot(db, "msg_edit").await;
        db.execute("UPDATE messages SET message_json=json_set(message_json,'$.parts[0].text','revalidatedword') WHERE id='msg_edit'",vec![]).await.unwrap();
        project(db, stale).await.unwrap();
        assert!(indexing(db).await.unwrap());
        flush(db).await.unwrap();
        assert!(find(db, "newword", false, 10).await.is_empty());
        assert_eq!(find(db, "revalidatedword", false, 10).await.len(), 1);
        let dirty = dirty_message("msg_edit");
        db.execute(&dirty.0, dirty.1).await.unwrap();
        let old = snapshot(db, "msg_edit").await;
        let mut delete = delete_message("msg_edit");
        delete.push((
            "DELETE FROM messages WHERE id=?".into(),
            vec![text("msg_edit")],
        ));
        db.execute_transaction(delete).await.unwrap();
        project(db, old).await.unwrap();
        assert_eq!(db.fetch_scalar_i64("SELECT count(*) FROM conversation_search_docs WHERE message_id='msg_edit'",vec![]).await.unwrap(),0);
        session(db, "ses_edit", "renamedtitle", "/project", None, "local").await;
        flush(db).await.unwrap();
        assert!(find(db, "oldtitle", true, 10).await.is_empty());
        assert_eq!(find(db, "renamedtitle", true, 10).await.len(), 1);
        let mut delete = delete_session("ses_edit");
        delete.push((
            "DELETE FROM sessions WHERE id=?".into(),
            vec![text("ses_edit")],
        ));
        db.execute_transaction(delete).await.unwrap();
        assert!(find(db, "renamedtitle", true, 10).await.is_empty());
    }

    #[tokio::test]
    async fn backfill_is_bounded_persistent_and_payload_is_utf8_bounded() {
        let _search_test = SEARCH_TEST.lock().await;
        let store = store().await;
        let db = &store.db;
        flush(db).await.unwrap();
        for i in 0..40 {
            session(
                db,
                &format!("ses_legacy{i:03}"),
                "legacy",
                "/project",
                None,
                "local",
            )
            .await;
        }
        db.execute("DELETE FROM conversation_search_dirty", vec![])
            .await
            .unwrap();
        db.execute("UPDATE data_migrations SET state='pending',cursor_id='' WHERE name='conversation-search-sessions-v1'",vec![]).await.unwrap();
        assert!(backfill(db).await.unwrap());
        assert!(
            db.fetch_scalar_i64("SELECT count(*) FROM conversation_search_dirty", vec![])
                .await
                .unwrap()
                <= BATCH as i64
        );
        let cursor = db.fetch_optional("SELECT cursor_id FROM data_migrations WHERE name='conversation-search-sessions-v1'",vec![]).await.unwrap().unwrap().get_str("cursor_id").unwrap();
        migrate(db).await.unwrap();
        assert_eq!(db.fetch_optional("SELECT cursor_id FROM data_migrations WHERE name='conversation-search-sessions-v1'",vec![]).await.unwrap().unwrap().get_str("cursor_id").unwrap(),cursor);
        flush(db).await.unwrap();
        assert_eq!(find(db, "legacy", true, 100).await.len(), 40);
        db.execute("UPDATE data_migrations SET state='pending',cursor_id='' WHERE name='conversation-search-sessions-v1'", vec![]).await.unwrap();
        assert!(backfill(db).await.unwrap());
        assert_eq!(
            db.fetch_scalar_i64("SELECT count(*) FROM conversation_search_dirty", vec![])
                .await
                .unwrap(),
            0
        );
        flush(db).await.unwrap();
        message(
            db,
            "msg_large",
            "ses_legacy000",
            &format!("needle {}", "界".repeat(BODY_BYTES)),
            "local",
        )
        .await;
        flush(db).await.unwrap();
        let body = db
            .fetch_optional(
                "SELECT body FROM conversation_search_docs WHERE message_id='msg_large'",
                vec![],
            )
            .await
            .unwrap()
            .unwrap()
            .get_str("body")
            .unwrap();
        assert!(body.len() <= BODY_BYTES && body.ends_with('界'));
        assert_eq!(find(db, "needle", false, 10).await.len(), 1);
    }

    #[tokio::test]
    async fn authorization_precedes_limit_and_worker_is_not_a_store_cycle() {
        let _search_test = SEARCH_TEST.lock().await;
        let store = running_store().await;
        let db = store.db.clone();
        let dir = std::env::temp_dir();
        let allowed = dir.join(format!(
            "neoism-search-allowed-{}",
            neoism_agent_core::new_session_id()
        ));
        std::fs::create_dir_all(&allowed).unwrap();
        let denied = dir.join("neoism-search-denied");
        std::fs::create_dir_all(&denied).unwrap();
        let claims = crate::caller::CallerClaims {
            subject: "test".into(),
            workspace_id: None,
            tenant_id: "local".into(),
            directory_prefixes: vec![allowed.to_string_lossy().into_owned()],
            hosted: false,
            max_sessions: None,
            max_artifacts: None,
            max_artifact_bytes: None,
            artifact_retention_days: None,
            requests_per_minute: None,
            max_in_flight: None,
            resolved: None,
            worker: None,
        };
        session(
            &db,
            "ses_allowed",
            "visible",
            allowed.to_str().unwrap(),
            None,
            "local",
        )
        .await;
        message(
            &db,
            "msg_allowed",
            "ses_allowed",
            "lexical with extra context",
            "local",
        )
        .await;
        session(
            &db,
            "ses_blocked",
            "lexical",
            denied.to_str().unwrap(),
            None,
            "local",
        )
        .await;
        message(
            &db,
            "msg_blocked",
            "ses_blocked",
            "lexical lexical lexical",
            "local",
        )
        .await;
        flush(&db).await.unwrap();
        let hits = search(
            &db,
            TenantQueryScope::Tenant("local"),
            "lexical",
            None,
            None,
            1,
            true,
            Some(&claims),
        )
        .await
        .unwrap();
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].session_id, "ses_allowed");
        drop(store);
        message(&db, "msg_after_drop", "ses_allowed", "afterdrop", "local").await;
        tokio::time::sleep(Duration::from_millis(800)).await;
        assert!(find(&db, "afterdrop", false, 10).await.is_empty());
        let worker = spawn(db.clone());
        tokio::time::timeout(Duration::from_secs(10), async {
            loop {
                if !find(&db, "afterdrop", false, 10).await.is_empty() {
                    break;
                }
                tokio::time::sleep(Duration::from_millis(100)).await;
            }
        })
        .await
        .unwrap();
        drop(worker);
    }

    #[tokio::test]
    async fn streaming_is_coalesced_and_shutdown_is_repeatable() {
        let _search_test = SEARCH_TEST.lock().await;
        let store = store().await;
        let db = &store.db;
        flush(db).await.unwrap();
        assert!(!indexing(db).await.unwrap());
        session(db, "ses_stream", "stream", "/project", None, "local").await;
        for text in ["first", "second", "settled"] {
            message(db, "msg_stream", "ses_stream", text, "local").await;
        }
        // Make the deadline deterministic even on heavily loaded CI.
        db.execute(
            "UPDATE conversation_search_dirty SET ready_at=?",
            vec![int(store_i64(crate::now_millis().saturating_add(60_000)))],
        )
        .await
        .unwrap();
        assert!(indexing(db).await.unwrap());
        tick(db, false).await.unwrap();
        assert_eq!(db.fetch_scalar_i64("SELECT count(*) FROM conversation_search_dirty WHERE message_id='msg_stream'",vec![]).await.unwrap(),1);
        assert!(find(db, "settled", false, 10).await.is_empty());
        flush(db).await.unwrap();
        assert!(!indexing(db).await.unwrap());
        assert_eq!(find(db, "settled", false, 10).await.len(), 1);
        assert!(find(db, "first", false, 10).await.is_empty());
        let worker = spawn(db.clone());
        tokio::join!(worker.shutdown(), worker.shutdown());
        worker.shutdown().await;
        let missing = dirty_message("msg_does_not_exist");
        db.execute(&missing.0, missing.1).await.unwrap();
        assert!(!indexing(db).await.unwrap());
    }

    #[tokio::test]
    async fn hosted_local_history_and_revocations_use_authoritative_overlays() {
        let _search_test = SEARCH_TEST.lock().await;
        let store = store().await;
        let db = &store.db;
        let directory = std::env::temp_dir().join(format!(
            "neoism-search-hosted-{}",
            neoism_agent_core::new_session_id()
        ));
        std::fs::create_dir_all(&directory).unwrap();
        session(
            db,
            "ses_hosted",
            "hostedtitle",
            directory.to_str().unwrap(),
            None,
            "local",
        )
        .await;
        message(db, "msg_hosted", "ses_hosted", "hostedword", "local").await;
        db.execute_transaction(vec![
            (
                "INSERT INTO hosted_chat_directories(directory,workspace_id) VALUES(?,?)"
                    .into(),
                vec![text(directory.to_str().unwrap()), text("search-workspace")],
            ),
            (
                "INSERT INTO hosted_chat_sessions(session_id,workspace_id) VALUES(?,?)"
                    .into(),
                vec![text("ses_hosted"), text("search-workspace")],
            ),
        ])
        .await
        .unwrap();
        flush(db).await.unwrap();
        let hits = search(
            db,
            TenantQueryScope::Tenant("workspace:search-workspace"),
            "hostedword",
            None,
            None,
            10,
            true,
            None,
        )
        .await
        .unwrap();
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].message_id, "msg_hosted");
        db.execute_transaction(vec![
            ("DELETE FROM hosted_chat_sessions WHERE session_id=?".into(),vec![text("ses_hosted")]),
            ("INSERT INTO hosted_chat_revocations(session_id,workspace_id) VALUES(?,?)".into(),vec![text("ses_hosted"),text("search-workspace")]),
        ]).await.unwrap();
        assert!(search(
            db,
            TenantQueryScope::Tenant("workspace:search-workspace"),
            "hostedword",
            None,
            None,
            10,
            true,
            None
        )
        .await
        .unwrap()
        .is_empty());
        assert_eq!(
            search(
                db,
                TenantQueryScope::Tenant("local"),
                "hostedword",
                None,
                None,
                10,
                true,
                None
            )
            .await
            .unwrap()
            .len(),
            1
        );
    }

    #[tokio::test]
    async fn topdocs_limit_is_pushed_and_metadata_filtering_is_separate() {
        let _search_test = SEARCH_TEST.lock().await;
        let store = store().await;
        let db = &store.db;
        session(db, "ses_native", "native", "/project", None, "local").await;
        message(db, "msg_native", "ses_native", "native needle", "local").await;
        flush(db).await.unwrap();
        let bytecode = db
            .fetch_all(
                &format!("EXPLAIN {CANDIDATES}"),
                vec![text("\"needle\""), int(64)],
            )
            .await
            .unwrap();
        let native = bytecode
            .iter()
            .find(|row| row.get_str("opcode").unwrap_or_default() == "IndexMethodQuery")
            .expect("native FTS opcode");
        let register = native.get_i64("p3").unwrap();
        // turso_core main_loop/open.rs emits pattern index at start_reg,
        // followed by query and captured LIMIT. fts.rs pattern 1 is bounded;
        // patterns 2/4/6 instead allocate up to MAX_NO_LIMIT_RESULT (1 million).
        assert!(
            bytecode
                .iter()
                .any(|row| row.get_str("opcode").unwrap_or_default() == "Integer"
                    && row.get_i64("p1").ok() == Some(1)
                    && row.get_i64("p2").ok() == Some(register)),
            "must use bounded combined_ordered_limit pattern"
        );
        assert!(
            bytecode
                .iter()
                .any(
                    |row| row.get_str("opcode").unwrap_or_default() == "Variable"
                        && row.get_i64("p1").ok() == Some(2)
                        && row.get_i64("p2").ok() == Some(register + 2)
                ),
            "LIMIT must be passed into native TopDocs, not just applied outside it"
        );
        for (scope, conversations) in [
            (TenantQueryScope::LocalAll, true),
            (TenantQueryScope::Tenant("local"), true),
            (TenantQueryScope::Tenant("foreign"), false),
        ] {
            let (sql, params) = eligibility_query(
                &["message:msg_native".into()],
                scope,
                None,
                None,
                conversations,
            );
            assert!(!sql.contains("fts_"));
            let rows = db.fetch_all(&sql, params).await.unwrap();
            assert_eq!(
                rows.len(),
                if scope == TenantQueryScope::Tenant("foreign") {
                    0
                } else {
                    1
                }
            );
        }
    }

    #[tokio::test]
    async fn title_frontier_and_final_payloads_are_bounded() {
        let _search_test = SEARCH_TEST.lock().await;
        let store = store().await;
        let db = &store.db;
        session(
            db,
            "ses_titlewinner",
            "needle needle needle",
            "/project",
            None,
            "local",
        )
        .await;
        session(db, "ses_bodymass", "other", "/project", None, "local").await;
        for i in 0..150 {
            message(
                db,
                &format!("msg_mass{i:03}"),
                "ses_bodymass",
                &format!("needle {}", "padding ".repeat(100)),
                "local",
            )
            .await;
        }
        flush(db).await.unwrap();
        let mut budget = SearchBudget::default();
        let hits = search_inner(
            db,
            TenantQueryScope::LocalAll,
            "needle",
            None,
            None,
            1,
            true,
            None,
            &mut budget,
        )
        .await
        .unwrap();
        assert_eq!(hits[0].session_id, "ses_titlewinner");
        assert!(hits[0].message_id.is_empty());
        assert!(
            budget.candidates < 150,
            "title-only result must stop at relevance frontier"
        );
        assert_eq!(budget.payloads, 1);
        let mut budget = SearchBudget {
            max_candidates: Some(8),
            ..Default::default()
        };
        let error = search_inner(
            db,
            TenantQueryScope::LocalAll,
            "needle",
            None,
            None,
            1,
            false,
            None,
            &mut budget,
        )
        .await
        .unwrap_err();
        assert!(error.to_string().contains("refine"));
        assert_eq!(
            budget.payloads, 0,
            "broad queries must not load candidate bodies before proof"
        );
        assert_eq!(budget.candidates, 8);
        let mut budget = SearchBudget::default();
        let hits = search_inner(
            db,
            TenantQueryScope::LocalAll,
            "needle",
            None,
            None,
            3,
            false,
            None,
            &mut budget,
        )
        .await
        .unwrap();
        assert_eq!(hits.len(), 3);
        assert_eq!(budget.payloads, 3, "only final topK bodies are loaded");
    }

    #[tokio::test]
    async fn conversations_prefer_closest_words_then_recent_activity() {
        let _search_test = SEARCH_TEST.lock().await;
        let store = store().await;
        let db = &store.db;
        let now = crate::now_millis();
        let old = now.saturating_sub(365 * 24 * 60 * 60 * 1000);
        for (id, body, updated) in [
            ("ses_old_exact", "cursor hover ownership", old),
            ("ses_recent_exact", "cursor hover ownership", now),
            ("ses_recent_scattered", "hover cursor ownership", now),
            ("ses_recent_partial", "cursor ownership", now),
        ] {
            session(db, id, "discussion", "/project", None, "local").await;
            message(db, &format!("msg_{id}"), id, body, "local").await;
            db.execute(
                "UPDATE sessions SET info_json=json_set(info_json,'$.time.updated',?), updated=? WHERE id=?",
                vec![int(store_i64(updated)), int(store_i64(updated)), text(id)],
            ).await.unwrap();
        }
        flush(db).await.unwrap();
        assert_eq!(find(db, "cursor", true, 10).await.len(), 4);
        let hits = find(db, "cursor hover", true, 10).await;
        assert_eq!(
            hits.iter()
                .map(|hit| hit.session_id.as_str())
                .collect::<Vec<_>>(),
            vec![
                "ses_recent_exact",
                "ses_old_exact",
                "ses_recent_scattered",
                "ses_recent_partial",
            ]
        );
        assert!(
            hits[0].score > hits[1].score,
            "recent activity should decide equally close matches"
        );
        assert!(
            hits[1].score > hits[2].score,
            "an older exact phrase must beat newer scattered words"
        );

        // Recency follows the conversation, not the old matching message.
        db.execute(
            "UPDATE sessions SET info_json=json_set(info_json,'$.time.updated',?) WHERE id='ses_old_exact'",
            vec![int(store_i64(now.saturating_add(1)))],
        ).await.unwrap();
        assert_eq!(
            find(db, "cursor hover", true, 10).await[0].session_id,
            "ses_old_exact"
        );
    }

    #[tokio::test]
    async fn recency_ties_corrupt_sessions_and_backpressure() {
        let _search_test = SEARCH_TEST.lock().await;
        let store = store().await;
        let db = &store.db;
        session(db, "ses_ties", "ties", "/project", None, "local").await;
        message(db, "msg_a_old", "ses_ties", "tie needle", "local").await;
        message(db, "msg_z_new", "ses_ties", "tie needle", "local").await;
        flush(db).await.unwrap();
        db.execute(
            "UPDATE conversation_search_docs SET created=99 WHERE message_id='msg_z_new'",
            vec![],
        )
        .await
        .unwrap();
        assert_eq!(
            find(db, "needle", false, 1).await[0].message_id,
            "msg_z_new"
        );
        assert_eq!(find(db, "needle", true, 1).await[0].message_id, "msg_z_new");
        // Corrupt legacy metadata must skip that session, not fail all search.
        session(db, "ses_corrupt", "needle", "/project", None, "local").await;
        flush(db).await.unwrap();
        db.execute(
            "UPDATE sessions SET info_json='not json' WHERE id='ses_corrupt'",
            vec![],
        )
        .await
        .unwrap();
        let hits = find(db, "needle", true, 10).await;
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].session_id, "ses_ties");
        let semaphore = search_slots(db);
        let one = semaphore.clone().try_acquire_owned().unwrap();
        // The second slot remains usable; saturation alone must reject work.
        assert_eq!(find(db, "needle", false, 1).await.len(), 1);
        let two = semaphore.clone().try_acquire_owned().unwrap();
        let error = search(
            db,
            TenantQueryScope::LocalAll,
            "needle",
            None,
            None,
            1,
            false,
            None,
        )
        .await
        .unwrap_err();
        assert!(error.to_string().contains("busy"));
        drop((one, two));
        assert_eq!(find(db, "needle", false, 1).await.len(), 1);
    }

    #[tokio::test]
    async fn projection_batches_preserve_guards_and_source_byte_bound() {
        let _search_test = SEARCH_TEST.lock().await;
        let store = store().await;
        let db = &store.db;
        session(db, "ses_batch", "batch", "/project", None, "local").await;
        flush(db).await.unwrap();
        for i in 0..BATCH {
            message(
                db,
                &format!("msg_batch{i:02}"),
                "ses_batch",
                "batched original",
                "local",
            )
            .await;
        }
        assert!(tick(db, true).await.unwrap());
        assert_eq!(
            db.fetch_scalar_i64(
                "SELECT count(*) FROM conversation_search_docs WHERE message_id<>''",
                vec![]
            )
            .await
            .unwrap(),
            BATCH as i64
        );
        assert_eq!(
            db.fetch_scalar_i64("SELECT count(*) FROM conversation_search_dirty", vec![])
                .await
                .unwrap(),
            0
        );
        // Prepare several documents, then change/delete canonical rows before
        // committing the whole batch. Each statement must revalidate its token.
        let mut prepared = Vec::new();
        for i in 0..3 {
            let id = format!("msg_batch{i:02}");
            let dirty = dirty_message(&id);
            db.execute(&dirty.0, dirty.1).await.unwrap();
            prepared.extend(projection_statements(snapshot(db, &id).await).unwrap());
        }
        message(db, "msg_batch00", "ses_batch", "batchreplacement", "local").await;
        let mut delete = delete_message("msg_batch01");
        delete.push((
            "DELETE FROM messages WHERE id=?".into(),
            vec![text("msg_batch01")],
        ));
        db.execute_transaction(delete).await.unwrap();
        let prepared = combine_projection_statements(prepared);
        assert_eq!(
            prepared.len(),
            4,
            "one guarded UPSERT plus three queue cleanups"
        );
        db.execute_transaction(prepared).await.unwrap();
        assert_eq!(db.fetch_scalar_i64("SELECT count(*) FROM conversation_search_docs WHERE message_id='msg_batch01'",vec![]).await.unwrap(),0);
        assert_eq!(db.fetch_scalar_i64("SELECT count(*) FROM conversation_search_dirty WHERE message_id='msg_batch00'",vec![]).await.unwrap(),1);
        flush(db).await.unwrap();
        assert_eq!(find(db, "batchreplacement", false, 10).await.len(), 1);
        // Large non-searchable metadata still counts against the source budget.
        // Two >1 MiB sources cannot be retained in the same 2 MiB raw batch.
        for id in ["msg_big0", "msg_big1"] {
            message(db, id, "ses_batch", "large metadata", "local").await;
            db.execute("UPDATE messages SET message_json=json_set(message_json,'$.info.system',?) WHERE id=?",vec![text("x".repeat(BATCH_SOURCE_BYTES/2+1024)),text(id)]).await.unwrap();
        }
        tick(db, true).await.unwrap();
        assert_eq!(db.fetch_scalar_i64("SELECT count(*) FROM conversation_search_dirty WHERE message_id IN ('msg_big0','msg_big1')",vec![]).await.unwrap(),1);
        tick(db, true).await.unwrap();
        assert_eq!(
            db.fetch_scalar_i64("SELECT count(*) FROM conversation_search_dirty", vec![])
                .await
                .unwrap(),
            0
        );
        // An individually oversized payload is processed alone, not starved.
        message(db, "msg_oversized", "ses_batch", "oversizedsource", "local").await;
        db.execute("UPDATE messages SET message_json=json_set(message_json,'$.info.system',?) WHERE id='msg_oversized'",vec![text("x".repeat(BATCH_SOURCE_BYTES+1024))]).await.unwrap();
        tick(db, true).await.unwrap();
        assert_eq!(
            db.fetch_scalar_i64("SELECT count(*) FROM conversation_search_dirty", vec![])
                .await
                .unwrap(),
            0
        );
        assert_eq!(find(db, "oversizedsource", false, 10).await.len(), 1);
    }

    #[tokio::test]
    async fn failed_projection_batch_rolls_back_sql_and_native_fts() {
        let _search_test = SEARCH_TEST.lock().await;
        let path = std::env::temp_dir().join(format!(
            "neoism-search-rollback-{}.db",
            neoism_agent_core::new_session_id()
        ));
        let store = SessionStore::open(path.clone()).await.unwrap();
        store._search_worker.as_ref().unwrap().shutdown().await;
        let db = &store.db;
        session(db, "ses_rollback", "rollback", "/project", None, "local").await;
        message(
            db,
            "msg_rollback",
            "ses_rollback",
            "originalrollbackword",
            "local",
        )
        .await;
        message(
            db,
            "msg_secondrollback",
            "ses_rollback",
            "secondoriginalword",
            "local",
        )
        .await;
        flush(db).await.unwrap();
        assert_eq!(find(db, "originalrollbackword", false, 10).await.len(), 1);
        message(
            db,
            "msg_rollback",
            "ses_rollback",
            "uncommittedrollbackword",
            "local",
        )
        .await;
        message(
            db,
            "msg_secondrollback",
            "ses_rollback",
            "seconduncommittedword",
            "local",
        )
        .await;
        let mut statements =
            projection_statements(snapshot(db, "msg_rollback").await).unwrap();
        statements.extend(
            projection_statements(snapshot(db, "msg_secondrollback").await).unwrap(),
        );
        let mut statements = combine_projection_statements(statements);
        assert_eq!(
            statements.len(),
            3,
            "one native UPSERT cursor for both documents"
        );
        statements.push((
            "INSERT INTO conversation_search_deliberately_missing_table VALUES (1)"
                .into(),
            vec![],
        ));
        assert!(db.execute_transaction(statements).await.is_err());
        // Db reads acquire fresh connections; neither the table nor Tantivy's
        // reader/writer state may expose a partially applied failed batch.
        assert_eq!(db.fetch_optional("SELECT body FROM conversation_search_docs WHERE message_id='msg_rollback'",vec![]).await.unwrap().unwrap().get_str("body").unwrap(),"originalrollbackword");
        assert_eq!(db.fetch_scalar_i64("SELECT count(*) FROM conversation_search_dirty WHERE message_id='msg_rollback'",vec![]).await.unwrap(),1);
        assert!(find(db, "uncommittedrollbackword", false, 10)
            .await
            .is_empty());
        assert_eq!(find(db, "originalrollbackword", false, 10).await.len(), 1);
        assert!(find(db, "seconduncommittedword", false, 10)
            .await
            .is_empty());
        assert_eq!(find(db, "secondoriginalword", false, 10).await.len(), 1);
        drop(store);
        let reopened = SessionStore::open(path.clone()).await.unwrap();
        reopened._search_worker.as_ref().unwrap().shutdown().await;
        assert!(find(&reopened.db, "uncommittedrollbackword", false, 10)
            .await
            .is_empty());
        assert_eq!(
            find(&reopened.db, "originalrollbackword", false, 10)
                .await
                .len(),
            1
        );
        // A later successful retry must not inherit poisoned writer state.
        flush(&reopened.db).await.unwrap();
        assert_eq!(
            find(&reopened.db, "uncommittedrollbackword", false, 10)
                .await
                .len(),
            1
        );
        assert!(find(&reopened.db, "originalrollbackword", false, 10)
            .await
            .is_empty());
    }

    #[test]
    fn syntax_and_excerpt_unicode_offsets_are_safe() {
        assert_eq!(
            terms("Rust::borrow_checker /src/main.rs"),
            vec!["Rust", "borrow", "checker", "src", "main", "rs"]
        );
        assert_eq!(
            plain_query(&terms("OR NOT body:query"), false),
            "\"OR\" AND \"NOT\" AND \"body\" AND \"query\""
        );
        assert_eq!(excerpt("αβ CAFÉ done", &terms("café")), "αβ >>CAFÉ<< done");
        assert!(truncate_body("界".repeat(BODY_BYTES)).len() <= BODY_BYTES);
    }
}
