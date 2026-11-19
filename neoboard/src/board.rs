//! SQLite is the single writer authority. Mutations, derived task transitions and events share a transaction.
use crate::protocol::*;
use axum::{extract::{Path, Query, State}, http::{HeaderMap, StatusCode}, response::{IntoResponse, Response}, routing::{delete, get, post}, Json, Router};
use rand::RngCore;
use rusqlite::{params, Connection, OptionalExtension, Transaction};
use serde::Deserialize;
use sha2::{Digest, Sha256};
use std::{collections::{BTreeMap, BTreeSet}, path::Path as FsPath, sync::{Arc, Mutex}, time::{Duration, SystemTime, UNIX_EPOCH}};

#[derive(Debug)]
pub struct BoardError(pub StatusCode, pub String);
impl std::fmt::Display for BoardError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result { write!(f, "{}", self.1) }
}
impl std::error::Error for BoardError {}
impl From<rusqlite::Error> for BoardError {
    fn from(e: rusqlite::Error) -> Self { Self(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()) }
}
impl From<serde_json::Error> for BoardError {
    fn from(e: serde_json::Error) -> Self { Self(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()) }
}
impl IntoResponse for BoardError {
    fn into_response(self) -> Response {
        let message = if self.0.is_server_error() { "internal board error".to_string() } else { self.1 };
        (self.0, Json(ApiError { error: message })).into_response()
    }
}
type Result<T> = std::result::Result<T, BoardError>;
fn invalid(s: &str) -> BoardError { BoardError(StatusCode::BAD_REQUEST, s.into()) }
fn conflict(s: &str) -> BoardError { BoardError(StatusCode::CONFLICT, s.into()) }
fn forbidden() -> BoardError { BoardError(StatusCode::FORBIDDEN, "credential does not permit this operation".into()) }
fn unauthorized() -> BoardError { BoardError(StatusCode::UNAUTHORIZED, "invalid or expired bearer credential".into()) }
pub fn now() -> u64 { SystemTime::now().duration_since(UNIX_EPOCH).unwrap_or_default().as_secs() }
pub fn random_token() -> String {
    let mut bytes = [0u8; 32]; rand::thread_rng().fill_bytes(&mut bytes);
    bytes.iter().map(|v| format!("{v:02x}")).collect()
}
fn digest(token: &str) -> String { format!("{:x}", Sha256::digest(token.as_bytes())) }

#[derive(Clone)]
pub struct Board { db: Arc<Mutex<Connection>>, admin_hash: String }
#[derive(Clone)]
struct Identity { id: u64, actor: String, channels: Vec<String> }
impl Identity {
    fn permits(&self, channel: &str) -> bool {
        self.channels.is_empty() || self.channels.iter().any(|c| c == channel || project_scope(c).is_some_and(|project| {
            channel == project || channel.strip_prefix(project).is_some_and(|suffix| suffix.starts_with('/'))
        }))
    }
    fn check(&self, channel: &str) -> Result<()> { if self.permits(channel) { Ok(()) } else { Err(forbidden()) } }
}

impl Board {
    /// The two bootstrap tokens must be distinct. The admin token is never a board credential.
    pub fn open(path: impl AsRef<FsPath>, board_token: &str, admin_token: &str) -> Result<Self> {
        if board_token.len() < 32 || admin_token.len() < 32 || board_token == admin_token { return Err(invalid("distinct strong bootstrap tokens required")); }
        let db = Connection::open(path)?;
        db.busy_timeout(Duration::from_secs(5))?;
        db.execute_batch("PRAGMA journal_mode=WAL; PRAGMA synchronous=FULL; PRAGMA foreign_keys=ON;
            CREATE TABLE IF NOT EXISTS channels(name TEXT PRIMARY KEY);
            INSERT OR IGNORE INTO channels VALUES ('general');
            CREATE TABLE IF NOT EXISTS messages(id INTEGER PRIMARY KEY AUTOINCREMENT, channel TEXT NOT NULL, creator TEXT NOT NULL, body TEXT NOT NULL);
            CREATE TABLE IF NOT EXISTS events(cursor INTEGER PRIMARY KEY AUTOINCREMENT, channel TEXT NOT NULL, body TEXT NOT NULL);
            CREATE INDEX IF NOT EXISTS event_channels ON events(channel,cursor);
            CREATE TABLE IF NOT EXISTS credentials(id INTEGER PRIMARY KEY AUTOINCREMENT, hash TEXT UNIQUE NOT NULL, actor TEXT UNIQUE NOT NULL, channels TEXT NOT NULL, expires INTEGER, revoked INTEGER NOT NULL DEFAULT 0);
            CREATE TABLE IF NOT EXISTS actor_display_names(credential INTEGER PRIMARY KEY REFERENCES credentials(id), name TEXT NOT NULL);
            CREATE TABLE IF NOT EXISTS task_blocks(message INTEGER PRIMARY KEY REFERENCES messages(id) ON DELETE CASCADE, actor TEXT NOT NULL, reason TEXT NOT NULL);
            CREATE TABLE IF NOT EXISTS idempotency(credential INTEGER NOT NULL, key TEXT NOT NULL, request TEXT NOT NULL, outcome TEXT NOT NULL, PRIMARY KEY(credential,key));
            CREATE TABLE IF NOT EXISTS reactions(message INTEGER NOT NULL, actor TEXT NOT NULL, reaction TEXT NOT NULL, PRIMARY KEY(message,actor,reaction));
            CREATE TABLE IF NOT EXISTS attachments(id INTEGER PRIMARY KEY AUTOINCREMENT, message_id INTEGER NOT NULL REFERENCES messages(id) ON DELETE CASCADE, name TEXT NOT NULL, bytes BLOB NOT NULL CHECK(length(bytes)<=8388608));
            CREATE INDEX IF NOT EXISTS attachment_messages ON attachments(message_id,id);
            CREATE TABLE IF NOT EXISTS schedules(id INTEGER PRIMARY KEY AUTOINCREMENT, credential INTEGER NOT NULL, actor TEXT NOT NULL, channel TEXT NOT NULL, text TEXT NOT NULL, labels TEXT NOT NULL, publish_at INTEGER NOT NULL);")?;
        db.execute("INSERT OR IGNORE INTO credentials(hash,actor,channels) VALUES (?1,'local','[]')", [digest(board_token)])?;
        Ok(Self { db: Arc::new(Mutex::new(db)), admin_hash: digest(admin_token) })
    }
    fn lock(&self) -> Result<std::sync::MutexGuard<'_, Connection>> {
        self.db.lock().map_err(|_| BoardError(StatusCode::INTERNAL_SERVER_ERROR, "board lock poisoned".into()))
    }
    fn identity(db: &Connection, token: &str) -> Result<Identity> {
        let row: Option<(u64,String,String)> = db.query_row("SELECT id,actor,channels FROM credentials WHERE hash=?1 AND revoked=0 AND (expires IS NULL OR expires>?2)", params![digest(token), now()], |r| Ok((r.get(0)?,r.get(1)?,r.get(2)?))).optional()?;
        let (id,actor,channels) = row.ok_or_else(unauthorized)?;
        Ok(Identity { id, actor, channels: serde_json::from_str(&channels)? })
    }
    pub fn create_invite(&self, admin: &str, request: InviteRequest) -> Result<Invite> {
        if digest(admin) != self.admin_hash { return Err(unauthorized()); }
        validate_name(&request.actor)?;
        if request.actor == "local" || request.actor == "system" { return Err(invalid("reserved actor")); }
        if request.expires_at.is_some_and(|v| v <= now() || v > i64::MAX as u64) { return Err(invalid("expiry must be in the future")); }
        if request.channels.len() > 100 { return Err(invalid("too many channels")); }
        for c in &request.channels { if project_scope(c).is_none() { validate_channel(c)?; } }
        let token = random_token();
        let db = self.lock()?;
        let exists: bool = db.query_row("SELECT EXISTS(SELECT 1 FROM credentials WHERE actor=?1)", [&request.actor], |r| r.get(0))?;
        if exists { return Err(conflict("actor already registered; use a new actor identity")); }
        db.execute("INSERT INTO credentials(hash,actor,channels,expires) VALUES (?1,?2,?3,?4)", params![digest(&token),request.actor,serde_json::to_string(&request.channels)?,request.expires_at])?;
        Ok(Invite { id: db.last_insert_rowid() as u64, token, actor: request.actor, channels: request.channels, expires_at: request.expires_at })
    }
    pub fn revoke_invite(&self, admin: &str, id: u64) -> Result<()> {
        if digest(admin) != self.admin_hash { return Err(unauthorized()); }
        let mut db = self.lock()?;
        let tx = db.transaction()?;
        if tx.execute("UPDATE credentials SET revoked=1 WHERE id=?1 AND actor!='local'", [sql_id(id)?])? == 0 { return Err(invalid("unknown invite")); }
        let schedules = tx.prepare("SELECT id,channel FROM schedules WHERE credential=?1")?.query_map([sql_id(id)?], |r| Ok((r.get::<_,u64>(0)?,r.get::<_,String>(1)?)))?.collect::<std::result::Result<Vec<_>,_>>()?;
        for (id,channel) in schedules { emit(&tx,"system",&channel,EventKind::ScheduleCanceled { id, channel:channel.clone() })?; }
        tx.execute("DELETE FROM schedules WHERE credential=?1", [sql_id(id)?])?;
        tx.commit()?; Ok(())
    }
    pub fn snapshot(&self, token: &str) -> Result<Snapshot> {
        let mut db = self.lock()?;
        let tx = db.transaction()?;
        let identity = Self::identity(&tx, token)?;
        maintain(&tx)?;
        let channels = tx.prepare("SELECT name FROM channels ORDER BY name")?.query_map([], |r| r.get::<_,String>(0))?.collect::<std::result::Result<Vec<_>,_>>()?.into_iter().filter(|c| identity.permits(c)).collect();
        let messages = all_messages(&tx)?.into_iter().filter(|m| identity.permits(&m.channel)).collect();
        let cursor = cursor(&tx)?;
        tx.commit()?;
        Ok(Snapshot { cursor, channels, messages })
    }
    /// Directory membership is based on live credentials, never message authors.
    /// Check the caller before reading identities, including for future channels.
    pub fn members(&self, token: &str, channel: &str) -> Result<Vec<Member>> {
        let db = self.lock()?;
        let caller = Self::identity(&db, token)?;
        caller.check(channel)?;
        validate_channel(channel)?;
        let rows = db.prepare("SELECT c.id,c.actor,c.channels,n.name FROM credentials c LEFT JOIN actor_display_names n ON n.credential=c.id WHERE c.revoked=0 AND (c.expires IS NULL OR c.expires>?1) ORDER BY c.actor")?
            .query_map([now()], |r| Ok((r.get::<_,u64>(0)?,r.get::<_,String>(1)?,r.get::<_,String>(2)?,r.get::<_,Option<String>>(3)?)))?
            .collect::<std::result::Result<Vec<_>,_>>()?;
        let mut members = Vec::new();
        for (id,actor,channels,display_name) in rows {
            let identity = Identity { id, actor, channels: serde_json::from_str(&channels)? };
            if identity.permits(channel) { members.push(Member { actor: identity.actor, display_name }); }
        }
        Ok(members)
    }
    pub fn set_display_name(&self, token: &str, name: &str) -> Result<()> {
        let db = self.lock()?;
        let identity = Self::identity(&db, token)?;
        // Allow human names, including spaces and Unicode, but not control or
        // bidi override characters that can spoof surrounding UI text.
        if name.len() > 256 || name.chars().any(|c| c.is_control() || matches!(c, '\u{202a}'..='\u{202e}' | '\u{2066}'..='\u{2069}')) {
            return Err(invalid("display name must be at most 256 bytes without control or bidi override characters"));
        }
        let name = name.trim();
        if name.is_empty() {
            db.execute("DELETE FROM actor_display_names WHERE credential=?1", [identity.id])?;
        } else {
            db.execute("INSERT INTO actor_display_names(credential,name) VALUES (?1,?2) ON CONFLICT(credential) DO UPDATE SET name=excluded.name", params![identity.id,name])?;
        }
        Ok(())
    }
    pub fn events(&self, token: &str, after: u64) -> Result<Vec<Event>> {
        let mut db = self.lock()?;
        let tx = db.transaction()?;
        let identity = Self::identity(&tx, token)?;
        maintain(&tx)?;
        let projects: Vec<_> = identity.channels.iter().filter_map(|c| project_scope(c)).collect();
        // Apply visibility BEFORE paging. Filtering a global page afterwards can return an
        // empty/short page while visible events remain beyond it. Use literal comparisons,
        // not LIKE/GLOB: underscores and dots in valid slugs are not wildcard syntax.
        let rows = tx.prepare("SELECT cursor,body FROM events WHERE cursor>?1 AND (
            ?2='[]' OR channel IN (SELECT value FROM json_each(?2)) OR EXISTS (
                SELECT 1 FROM json_each(?3) AS project WHERE channel=project.value
                OR substr(channel,1,length(project.value)+1)=project.value || '/'
            )) ORDER BY cursor LIMIT 1000")?.query_map(params![sql_id(after)?,serde_json::to_string(&identity.channels)?,serde_json::to_string(&projects)?], |r| Ok((r.get::<_,u64>(0)?,r.get::<_,String>(1)?)))?.collect::<std::result::Result<Vec<_>,_>>()?;
        let mut events = Vec::new();
        for (cursor,body) in rows {
            let mut event: Event = serde_json::from_str(&body)?; event.cursor = cursor; events.push(event);
        }
        tx.commit()?; Ok(events)
    }
    pub fn tick(&self) -> Result<()> {
        let mut db = self.lock()?; let tx = db.transaction()?; maintain(&tx)?; tx.commit()?; Ok(())
    }
    pub fn attach(&self, token: &str, id: u64, name: &str, bytes: Vec<u8>) -> Result<Attachment> {
        let mut db = self.lock()?;
        let tx = db.transaction()?;
        let identity = Self::identity(&tx, token)?;
        let msg = message(&tx, id, &identity)?;
        validate_attachment_name(name)?;
        if bytes.len() > MAX_ATTACHMENT_BYTES {
            return Err(BoardError(StatusCode::PAYLOAD_TOO_LARGE, "attachment exceeds 8 MiB".into()));
        }
        let (count, total): (usize, usize) = tx.query_row("SELECT COUNT(*),COALESCE(SUM(length(bytes)),0) FROM attachments WHERE message_id=?1", [sql_id(id)?], |r| Ok((r.get(0)?,r.get(1)?)))?;
        if count >= MAX_MESSAGE_ATTACHMENTS || total + bytes.len() > MAX_MESSAGE_ATTACHMENT_BYTES {
            return Err(BoardError(StatusCode::PAYLOAD_TOO_LARGE, "message attachment quota exceeded".into()));
        }
        tx.execute("INSERT INTO attachments(message_id,name,bytes) VALUES (?1,?2,?3)", params![sql_id(id)?,name,bytes])?;
        let attachment = Attachment { id: tx.last_insert_rowid() as u64, message_id: id, name: name.into(), size: bytes.len() as u64 };
        // Existing upsert event tells consumers to refresh this message's attachment list.
        emit(&tx, &identity.actor, &msg.channel, EventKind::Message { message: msg.clone() })?;
        tx.commit()?;
        Ok(attachment)
    }
    pub fn attachments(&self, token: &str, id: u64) -> Result<Vec<Attachment>> {
        let mut db = self.lock()?;
        let tx = db.transaction()?;
        let identity = Self::identity(&tx, token)?;
        message(&tx, id, &identity)?;
        let rows = tx.prepare("SELECT id,message_id,name,length(bytes) FROM attachments WHERE message_id=?1 ORDER BY id")?.query_map([sql_id(id)?], |r| Ok(Attachment { id:r.get(0)?, message_id:r.get(1)?, name:r.get(2)?, size:r.get(3)? }))?.collect::<std::result::Result<Vec<_>,_>>()?;
        Ok(rows)
    }
    pub fn download_attachment(&self, token: &str, id: u64) -> Result<Vec<u8>> {
        let mut db = self.lock()?;
        let tx = db.transaction()?;
        let identity = Self::identity(&tx, token)?;
        let message_id: Option<u64> = tx.query_row("SELECT message_id FROM attachments WHERE id=?1", [sql_id(id)?], |r| r.get(0)).optional()?;
        let missing = || BoardError(StatusCode::NOT_FOUND, "attachment not found".into());
        message(&tx, message_id.ok_or_else(missing)?, &identity).map_err(|e| if e.0 == StatusCode::NOT_FOUND { missing() } else { e })?;
        Ok(tx.query_row("SELECT bytes FROM attachments WHERE id=?1", [sql_id(id)?], |r| r.get(0))?)
    }
    pub fn reactions(&self, token: &str) -> Result<Vec<Reaction>> {
        let db = self.lock()?;
        let identity = Self::identity(&db, token)?;
        let rows = db.prepare("SELECT r.message,m.channel,r.actor,r.reaction FROM reactions r JOIN messages m ON m.id=r.message ORDER BY r.message,r.actor,r.reaction")?.query_map([], |r| Ok(Reaction { id:r.get(0)?,channel:r.get(1)?,actor:r.get(2)?,reaction:r.get(3)? }))?.collect::<std::result::Result<Vec<_>,_>>()?;
        Ok(rows.into_iter().filter(|r|identity.permits(&r.channel)).collect())
    }
    pub fn schedules(&self, token: &str) -> Result<Vec<ScheduledPost>> {
        let mut db = self.lock()?;
        let tx = db.transaction()?;
        let identity = Self::identity(&tx, token)?;
        maintain(&tx)?;
        let rows = tx.prepare("SELECT id,channel,actor,text,labels,publish_at FROM schedules WHERE credential=?1 ORDER BY publish_at,id")?.query_map([identity.id], |r| Ok((r.get::<_,u64>(0)?,r.get::<_,String>(1)?,r.get::<_,String>(2)?,r.get::<_,String>(3)?,r.get::<_,String>(4)?,r.get::<_,u64>(5)?)))?.collect::<std::result::Result<Vec<_>,_>>()?;
        let mut result=Vec::new();
        for (id,channel,author,text,labels,publish_at) in rows {
            if identity.permits(&channel) { result.push(ScheduledPost { id,channel,author,text,labels:serde_json::from_str(&labels)?,publish_at }); }
        }
        tx.commit()?; Ok(result)
    }
    pub fn action(&self, token: &str, profile: Option<&str>, key: Option<&str>, action: Action) -> Result<Outcome> {
        let mut db = self.lock()?;
        let tx = db.transaction()?;
        let identity = Self::identity(&tx, token)?;
        let author = if let Some(p) = profile { validate_name(p)?; format!("{}/{}", identity.actor, p) } else { identity.actor.clone() };
        let request = serde_json::to_string(&(profile, &action))?;
        if let Some(key) = key {
            if key.is_empty() || key.len() > 128 { return Err(invalid("idempotency key must be 1..128 bytes")); }
            let cached: Option<(String,String)> = tx.query_row("SELECT request,outcome FROM idempotency WHERE credential=?1 AND key=?2", params![identity.id,key], |r| Ok((r.get(0)?,r.get(1)?))).optional()?;
            if let Some((prior,outcome)) = cached { if prior != request { return Err(conflict("idempotency key reused with different action")); } return Ok(serde_json::from_str(&outcome)?); }
        }
        maintain(&tx)?;
        let id = apply(&tx, &identity, &author, action)?;
        settle_tasks(&tx)?;
        let outcome = Outcome { id, cursor: cursor(&tx)? };
        if let Some(key) = key { tx.execute("INSERT INTO idempotency VALUES (?1,?2,?3,?4)", params![identity.id,key,request,serde_json::to_string(&outcome)?])?; }
        tx.commit()?; Ok(outcome)
    }
}

fn sql_id(id: u64) -> Result<i64> { i64::try_from(id).map_err(|_| invalid("integer exceeds SQLite range")) }
fn validate_attachment_name(name: &str) -> Result<()> {
    if name.is_empty() || name.len() > 255 || name.trim() != name || name == "." || name == ".."
        || name.chars().any(|c| c.is_control() || matches!(c, '/' | '\\' | ':' | '\u{202a}'..='\u{202e}' | '\u{2066}'..='\u{2069}')) {
        return Err(invalid("attachment name must be a plain filename of 1..255 bytes"));
    }
    Ok(())
}
fn validate_name(s: &str) -> Result<()> {
    if s.is_empty() || s.len()>80 || !s.bytes().all(|c| c.is_ascii_alphanumeric() || b"-_.".contains(&c)) { return Err(invalid("name must be 1..80 ASCII letters, digits, dash, underscore or dot")); } Ok(())
}
fn validate_channel(s: &str) -> Result<()> {
    if s.len()>200 || s.split('/').any(|part| part == "." || part == ".." || validate_name(part).is_err()) { return Err(invalid("invalid channel path")); } Ok(())
}
/// Only a single validated channel segment may introduce a project scope. The
/// reserved marker is invite metadata, never a valid channel or general glob.
fn project_scope(scope: &str) -> Option<&str> {
    let project = scope.strip_suffix("/*")?;
    (!project.contains('/') && validate_channel(project).is_ok()).then_some(project)
}
fn validate_text(s: &str) -> Result<()> { if s.trim().is_empty() || s.len()>65536 || s.contains('\0') { return Err(invalid("text must be nonblank, NUL-free and at most 65536 bytes")); } Ok(()) }
fn validate_labels(labels: &BTreeMap<String,String>) -> Result<()> {
    if labels.len()>64 { return Err(invalid("at most 64 labels")); }
    for (k,v) in labels { validate_name(k)?; if v.len()>1024 || v.contains('\0') { return Err(invalid("invalid label value")); } } Ok(())
}
fn cursor(tx: &Transaction<'_>) -> Result<u64> { Ok(tx.query_row("SELECT COALESCE(MAX(cursor),0) FROM events", [], |r| r.get(0))?) }
fn emit(tx: &Transaction<'_>, actor: &str, channel: &str, kind: EventKind) -> Result<()> {
    let event = Event { cursor: 0, created_at: now(), actor: actor.into(), kind };
    tx.execute("INSERT INTO events(channel,body) VALUES (?1,?2)", params![channel,serde_json::to_string(&event)?])?; Ok(())
}
fn all_messages(tx: &Transaction<'_>) -> Result<Vec<Message>> {
    let rows = tx.prepare("SELECT body FROM messages ORDER BY id")?.query_map([], |r| r.get::<_,String>(0))?.collect::<std::result::Result<Vec<_>,_>>()?;
    rows.into_iter().map(|s| Ok(serde_json::from_str(&s)?)).collect()
}
fn message(tx: &Transaction<'_>, id: u64, identity: &Identity) -> Result<Message> {
    let body: Option<String> = tx.query_row("SELECT body FROM messages WHERE id=?1", [sql_id(id)?], |r| r.get(0)).optional()?;
    let msg: Message = serde_json::from_str(&body.ok_or_else(|| BoardError(StatusCode::NOT_FOUND,"message not found".into()))?)?;
    // Avoid distinguishing a hidden message from a nonexistent one.
    if !identity.permits(&msg.channel) { return Err(BoardError(StatusCode::NOT_FOUND,"message not found".into())); } Ok(msg)
}
fn save(tx: &Transaction<'_>, actor: &str, m: &Message) -> Result<()> {
    tx.execute("UPDATE messages SET body=?1 WHERE id=?2", params![serde_json::to_string(m)?,m.id])?;
    emit(tx,actor,&m.channel,EventKind::Message { message: m.clone() })
}
struct NewMessage {
    channel: String,
    parent: Option<u64>,
    author: String,
    text: String,
    labels: BTreeMap<String,String>,
    task: Option<Task>,
}
fn insert(tx: &Transaction<'_>, actor: &str, new: NewMessage) -> Result<u64> {
    let NewMessage { channel, parent, author, text, labels, task } = new;
    tx.execute("INSERT INTO messages(channel,creator,body) VALUES (?1,?2,'')", params![channel,actor])?;
    let m = Message { id: tx.last_insert_rowid() as u64, channel, parent, author, text, labels, created_at: now(), task };
    save(tx,actor,&m)?; Ok(m.id)
}
fn ensure_channel(tx: &Transaction<'_>, identity: &Identity, channel: &str) -> Result<()> {
    validate_channel(channel)?; identity.check(channel)?;
    if tx.execute("INSERT OR IGNORE INTO channels VALUES (?1)", [channel])? != 0 { emit(tx,&identity.actor,channel,EventKind::ChannelCreated { channel: channel.into() })?; } Ok(())
}
fn require_creator(tx: &Transaction<'_>, m: &Message, identity: &Identity) -> Result<()> {
    let creator: String = tx.query_row("SELECT creator FROM messages WHERE id=?1", [m.id], |r| r.get(0))?;
    if creator != identity.actor { return Err(forbidden()); } Ok(())
}
fn validate_assignee(tx: &Transaction<'_>, actor: &str, channel: &str) -> Result<()> {
    validate_name(actor)?;
    let row: Option<(u64,String)> = tx.query_row("SELECT id,channels FROM credentials WHERE actor=?1 AND revoked=0 AND (expires IS NULL OR expires>?2)", params![actor,now()], |r| Ok((r.get(0)?,r.get(1)?))).optional()?;
    let (id,channels) = row.ok_or_else(|| invalid("assignee must be an active channel member"))?;
    let target = Identity { id, actor: actor.into(), channels: serde_json::from_str(&channels)? };
    if !target.permits(channel) { return Err(invalid("assignee must be an active channel member")); }
    Ok(())
}
fn mutable_task(m: &Message) -> Result<&Task> {
    let t = m.task.as_ref().ok_or_else(|| invalid("not a task"))?;
    if matches!(t.state,TaskState::Completed|TaskState::Failed|TaskState::Canceled) { return Err(conflict("task is terminal")); }
    Ok(t)
}
// A creator may manage unleased work, but must not take another worker's live lease.
fn require_task_manager(tx: &Transaction<'_>, m: &Message, who: &Identity) -> Result<()> {
    let t = mutable_task(m)?;
    if t.state == TaskState::Claimed {
        if t.owner.as_deref() != Some(&who.actor) { return Err(conflict("only the active lease owner can change claimed work")); }
        Ok(())
    } else { require_creator(tx,m,who) }
}
fn apply(tx: &Transaction<'_>, who: &Identity, author: &str, action: Action) -> Result<Option<u64>> {
    use Action::*;
    match action {
        CreateChannel { channel } => { ensure_channel(tx,who,&channel)?; Ok(None) }
        Post { channel,text,labels,task,needs,assignee } => {
            validate_text(&text)?; validate_labels(&labels)?;
            if needs.len()>128 || (!task && (!needs.is_empty() || assignee.is_some())) { return Err(invalid("dependencies and assignment require a task; at most 128 dependencies")); }
            who.check(&channel)?;
            if let Some(a) = &assignee { validate_assignee(tx,a,&channel)?; }
            let needs: Vec<_> = needs.into_iter().collect::<BTreeSet<_>>().into_iter().collect();
            for id in &needs { if message(tx,*id,who)?.task.is_none() { return Err(invalid("dependency must be a visible task")); } }
            // Channel scopes must not expose dependency IDs from outside the task channel.
            for id in &needs { if message(tx,*id,who)?.channel != channel { return Err(invalid("dependencies must be in the same channel")); } }
            ensure_channel(tx,who,&channel)?;
            let task = task.then_some(Task { state: TaskState::Waiting, needs, assignee, owner: None, expires_at: None, finished_by:None,result:None });
            Ok(Some(insert(tx,&who.actor,NewMessage { channel,parent:None,author:author.into(),text,labels,task })?))
        }
        Reply { thread,text } => {
            apply(tx,who,author,ReplyWithLabels { thread,text,labels:BTreeMap::new() })
        }
        ReplyWithLabels { thread,text,labels } => {
            validate_labels(&labels)?;
            validate_text(&text)?; let root = message(tx,thread,who)?;
            if root.parent.is_some() { return Err(invalid("reply thread must be a root message")); }
            Ok(Some(insert(tx,&who.actor,NewMessage { channel:root.channel,parent:Some(thread),author:author.into(),text,labels,task:None })?))
        }
        Edit { id,text } => { validate_text(&text)?; let mut m = message(tx,id,who)?; require_creator(tx,&m,who)?; m.text=text; save(tx,&who.actor,&m)?; Ok(Some(id)) }
        DeleteThread { id } => {
            let root = message(tx,id,who)?;
            require_creator(tx,&root,who)?;
            if root.parent.is_some() { return Err(invalid("delete thread requires a root message")); }
            let messages = all_messages(tx)?;
            // Include terminal dependents: removing their prerequisite would leave dangling IDs.
            if messages.iter().any(|m| m.task.as_ref().is_some_and(|t| t.needs.contains(&id))) {
                return Err(conflict("thread has dependent tasks"));
            }
            let ids: Vec<_> = messages.iter().filter(|m| m.id == id || m.parent == Some(id)).map(|m| m.id).collect();
            let ids = serde_json::to_string(&ids)?;
            // Do not leave historical upserts that can resurrect deleted content on replay.
            // Existing cursors remain valid; AUTOINCREMENT and the tombstone preserve ordering.
            tx.execute("DELETE FROM events WHERE
                (json_extract(body,'$.type')='message' AND json_extract(body,'$.message.id') IN (SELECT value FROM json_each(?1))) OR
                (json_extract(body,'$.type')='reaction' AND json_extract(body,'$.id') IN (SELECT value FROM json_each(?1)))", [&ids])?;
            tx.execute("DELETE FROM reactions WHERE message IN (SELECT value FROM json_each(?1))", [&ids])?;
            // Foreign keys cascade attachments and manual task blocks, including reply attachments.
            tx.execute("DELETE FROM messages WHERE id IN (SELECT value FROM json_each(?1))", [&ids])?;
            emit(tx,&who.actor,&root.channel,EventKind::ThreadDeleted { id,channel:root.channel.clone() })?;
            Ok(Some(id))
        }
        SetLabels { id,labels } => { validate_labels(&labels)?; let mut m = message(tx,id,who)?; require_creator(tx,&m,who)?; m.labels=labels; save(tx,&who.actor,&m)?; Ok(Some(id)) }
        Assign { id,assignee } => {
            let mut m=message(tx,id,who)?; require_task_manager(tx,&m,who)?;
            if let Some(a)=&assignee { validate_assignee(tx,a,&m.channel)?; }
            let t=m.task.as_mut().unwrap();
            if t.assignee != assignee {
                t.assignee=assignee;
                // Handoff releases, rather than transfers, a lease. The recipient must claim.
                if t.state==TaskState::Claimed { t.state=TaskState::Waiting; t.owner=None; t.expires_at=None; }
                save(tx,&who.actor,&m)?;
            }
            Ok(Some(id))
        }
        Block { id,text } => {
            validate_text(&text)?;
            let mut m=message(tx,id,who)?; require_task_manager(tx,&m,who)?;
            tx.execute("INSERT INTO task_blocks(message,actor,reason) VALUES (?1,?2,?3) ON CONFLICT(message) DO UPDATE SET actor=excluded.actor,reason=excluded.reason",params![id,who.actor,text])?;
            let t=m.task.as_mut().unwrap(); t.state=TaskState::Blocked; t.owner=None; t.expires_at=None;
            save(tx,&who.actor,&m)?;
            insert(tx,&who.actor,NewMessage { channel:m.channel,parent:Some(id),author:author.into(),text,labels:BTreeMap::new(),task:None })?;
            Ok(Some(id))
        }
        Unblock { id } => {
            let m=message(tx,id,who)?; mutable_task(&m)?;
            let blocker: Option<String>=tx.query_row("SELECT actor FROM task_blocks WHERE message=?1",[id],|r|r.get(0)).optional()?;
            // The worker who released their lease by blocking may also clear that block.
            if blocker.as_deref()!=Some(&who.actor) { require_task_manager(tx,&m,who)?; }
            if blocker.is_none() { return Err(conflict("task has no manual block")); }
            tx.execute("DELETE FROM task_blocks WHERE message=?1",[id])?;
            save(tx,&who.actor,&m)?;
            Ok(Some(id))
        }
        Claim { id,lease_secs } | Renew { id,lease_secs } => {
            if !(1..=86400).contains(&lease_secs) { return Err(invalid("lease_secs must be 1..86400")); }
            let renew = matches!(action, Renew { .. });
            let mut m = message(tx,id,who)?; let t=m.task.as_mut().ok_or_else(|| invalid("not a task"))?;
            if renew { if t.state != TaskState::Claimed || t.owner.as_deref()!=Some(&who.actor) { return Err(conflict("only the active lease owner can renew")); } }
            else {
                if t.state != TaskState::Ready { return Err(conflict("task is not ready")); }
                if t.assignee.as_ref().is_some_and(|a| a!=&who.actor) { return Err(forbidden()); }
            }
            t.state=TaskState::Claimed; t.owner=Some(who.actor.clone()); t.expires_at=Some(now()+lease_secs);
            save(tx,&who.actor,&m)?; Ok(Some(id))
        }
        Complete { id,ref text } | Fail { id,ref text } => {
            let completed=matches!(action,Complete { .. });
            if !text.is_empty() { validate_text(text)?; }
            let mut m=message(tx,id,who)?; let t=m.task.as_mut().ok_or_else(|| invalid("not a task"))?;
            if t.state!=TaskState::Claimed || t.owner.as_deref()!=Some(&who.actor) { return Err(conflict("only the active lease owner can finish")); }
            t.state=if completed { TaskState::Completed } else { TaskState::Failed }; t.finished_by=t.owner.take(); t.expires_at=None;
            t.result=(!text.is_empty()).then(||text.clone());
            save(tx,&who.actor,&m)?;
            if !text.is_empty() { insert(tx,&who.actor,NewMessage { channel:m.channel,parent:Some(id),author:author.into(),text:text.clone(),labels:BTreeMap::new(),task:None })?; }
            Ok(Some(id))
        }
        Cancel { id } => {
            let mut m=message(tx,id,who)?; require_creator(tx,&m,who)?;
            let t=m.task.as_mut().ok_or_else(|| invalid("not a task"))?;
            if matches!(t.state,TaskState::Completed|TaskState::Failed|TaskState::Canceled) { return Err(conflict("task is terminal")); }
            t.state=TaskState::Canceled; t.owner=None; t.expires_at=None; save(tx,&who.actor,&m)?; Ok(Some(id))
        }
        React { id,reaction,remove } => {
            let m=message(tx,id,who)?;
            if reaction.trim().is_empty() || reaction.len()>64 || reaction.chars().any(char::is_control) { return Err(invalid("reaction must be 1..64 bytes without control characters")); }
            let changed=if remove { tx.execute("DELETE FROM reactions WHERE message=?1 AND actor=?2 AND reaction=?3",params![id,who.actor,reaction])? } else { tx.execute("INSERT OR IGNORE INTO reactions VALUES (?1,?2,?3)",params![id,who.actor,reaction])? };
            if changed>0 { emit(tx,&who.actor,&m.channel,EventKind::Reaction { id,channel:m.channel.clone(),reaction,remove })?; } Ok(Some(id))
        }
        Schedule { channel,text,publish_at,labels } => {
            validate_text(&text)?; validate_labels(&labels)?;
            if publish_at<=now() || publish_at>i64::MAX as u64 { return Err(invalid("publish_at must be in the future")); }
            ensure_channel(tx,who,&channel)?;
            tx.execute("INSERT INTO schedules(credential,actor,channel,text,labels,publish_at) VALUES (?1,?2,?3,?4,?5,?6)",params![who.id,author,channel,text,serde_json::to_string(&labels)?,publish_at])?;
            let id=tx.last_insert_rowid() as u64;
            emit(tx,&who.actor,&channel,EventKind::Scheduled { id,channel:channel.clone(),publish_at })?; Ok(Some(id))
        }
        CancelSchedule { id } => {
            let row: Option<(u64,String)> = tx.query_row("SELECT credential,channel FROM schedules WHERE id=?1",[sql_id(id)?],|r| Ok((r.get(0)?,r.get(1)?))).optional()?;
            let (credential,channel)=row.ok_or_else(|| invalid("unknown schedule"))?; who.check(&channel)?;
            if credential!=who.id { return Err(forbidden()); }
            tx.execute("DELETE FROM schedules WHERE id=?1",[id])?;
            emit(tx,&who.actor,&channel,EventKind::ScheduleCanceled { id,channel:channel.clone() })?; Ok(Some(id))
        }
    }
}

fn settle_tasks(tx: &Transaction<'_>) -> Result<()> {
    let messages=all_messages(tx)?;
    let mut states: BTreeMap<_,_>=messages.iter().filter_map(|m| m.task.as_ref().map(|t|(m.id,t.state))).collect();
    let manual: BTreeSet<u64>=tx.prepare("SELECT message FROM task_blocks")?.query_map([],|r|r.get(0))?.collect::<std::result::Result<_,_>>()?;
    for mut m in messages {
        let Some(t)=m.task.as_mut() else { continue };
        let expired=t.state==TaskState::Claimed && t.expires_at.is_none_or(|v|v<=now());
        if !expired && !matches!(t.state,TaskState::Ready|TaskState::Waiting|TaskState::Blocked) { continue; }
        let next=if manual.contains(&m.id) || t.needs.iter().any(|id|matches!(states.get(id),Some(TaskState::Failed|TaskState::Canceled|TaskState::Blocked))) { TaskState::Blocked }
            else if t.needs.iter().all(|id|states.get(id)==Some(&TaskState::Completed)) { TaskState::Ready } else { TaskState::Waiting };
        states.insert(m.id,next);
        if expired || next!=t.state { t.state=next; t.owner=None; t.expires_at=None; save(tx,"system",&m)?; }
    }
    // Dependencies point backwards; updating states during the ordered pass handles
    // both blocking and unblocking the entire descendant closure atomically.
    Ok(())
}
fn maintain(tx: &Transaction<'_>) -> Result<()> {
    let expired=tx.prepare("SELECT s.id,s.channel FROM schedules s JOIN credentials c ON c.id=s.credential WHERE c.revoked=1 OR (c.expires IS NOT NULL AND c.expires<=?1)")?.query_map([now()],|r|Ok((r.get::<_,u64>(0)?,r.get::<_,String>(1)?)))?.collect::<std::result::Result<Vec<_>,_>>()?;
    for (id,channel) in expired { tx.execute("DELETE FROM schedules WHERE id=?1",[id])?; emit(tx,"system",&channel,EventKind::ScheduleCanceled { id,channel:channel.clone() })?; }
    let rows=tx.prepare("SELECT s.id,s.actor,s.channel,s.text,s.labels,c.actor FROM schedules s JOIN credentials c ON c.id=s.credential WHERE s.publish_at<=?1 AND c.revoked=0 AND (c.expires IS NULL OR c.expires>?1) ORDER BY s.publish_at,s.id")?.query_map([now()],|r|Ok((r.get::<_,u64>(0)?,r.get::<_,String>(1)?,r.get::<_,String>(2)?,r.get::<_,String>(3)?,r.get::<_,String>(4)?,r.get::<_,String>(5)?)))?.collect::<std::result::Result<Vec<_>,_>>()?;
    for (id,author,channel,text,labels,actor) in rows {
        let message_id=insert(tx,&actor,NewMessage { channel:channel.clone(),parent:None,author,text,labels:serde_json::from_str(&labels)?,task:None })?;
        tx.execute("DELETE FROM schedules WHERE id=?1",[id])?;
        emit(tx,"system",&channel,EventKind::SchedulePublished { id,channel:channel.clone(),message_id })?;
    }
    settle_tasks(tx)
}

fn bearer(headers: &HeaderMap) -> Result<String> {
    headers.get("authorization").and_then(|h|h.to_str().ok()).and_then(|s|s.strip_prefix("Bearer ")).filter(|s|s.len()<=256).map(str::to_owned).ok_or_else(unauthorized)
}
async fn blocking<T: Send+'static>(f: impl FnOnce()->Result<T>+Send+'static) -> Result<T> {
    tokio::task::spawn_blocking(f).await.map_err(|_|BoardError(StatusCode::INTERNAL_SERVER_ERROR,"board worker failed".into()))?
}
pub fn router(board: Board) -> Router {
    let uploads = Router::new().route("/v1/messages/{id}/attachments", post(http_attach).get(http_attachments))
        .layer(axum::extract::DefaultBodyLimit::max(MAX_ATTACHMENT_BYTES));
    Router::new().route("/v1/snapshot",get(http_snapshot)).route("/v1/events",get(http_events)).route("/v1/actions",post(http_action))
        .route("/v1/reactions",get(http_reactions)).route("/v1/schedules",get(http_schedules))
        .route("/v1/members",get(http_members)).route("/v1/display-name",post(http_display_name)).route("/v1/me",get(http_me))
        .route("/v1/admin/invites",post(http_invite)).route("/v1/admin/invites/{id}",delete(http_revoke))
        .route("/v1/attachments/{id}",get(http_download_attachment))
        .layer(axum::extract::DefaultBodyLimit::max(128*1024)).merge(uploads).with_state(board)
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct AttachmentQuery { name: String }
async fn http_attach(State(board):State<Board>, headers:HeaderMap, Path(id):Path<u64>, Query(q):Query<AttachmentQuery>, bytes:axum::body::Bytes)->Result<Json<Attachment>> {
    let token = bearer(&headers)?;
    Ok(Json(blocking(move || board.attach(&token,id,&q.name,bytes.to_vec())).await?))
}
async fn http_attachments(State(board):State<Board>, headers:HeaderMap, Path(id):Path<u64>)->Result<Json<Vec<Attachment>>> {
    let token = bearer(&headers)?;
    Ok(Json(blocking(move || board.attachments(&token,id)).await?))
}
async fn http_download_attachment(State(board):State<Board>, headers:HeaderMap, Path(id):Path<u64>)->Result<Response> {
    let token = bearer(&headers)?;
    let bytes = blocking(move || board.download_attachment(&token,id)).await?;
    Ok(([("content-type","application/octet-stream"),("content-disposition","attachment"),("x-content-type-options","nosniff"),("cache-control","no-store"),("content-security-policy","sandbox; default-src 'none'")],bytes).into_response())
}
async fn http_snapshot(State(board):State<Board>,headers:HeaderMap)->Result<Json<Snapshot>> {
    let token=bearer(&headers)?; Ok(Json(blocking(move||board.snapshot(&token)).await?))
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct MembersQuery { channel: String }
async fn http_me(State(board):State<Board>,headers:HeaderMap)->Result<Json<Member>> {
    let token=bearer(&headers)?;Ok(Json(blocking(move||{
        let db=board.lock()?;let who=Board::identity(&db,&token)?;
        let name=db.query_row("SELECT name FROM actor_display_names WHERE credential=?1",[who.id],|r|r.get(0)).optional()?;
        Ok(Member{actor:who.actor,display_name:name})
    }).await?))
}
async fn http_members(State(board):State<Board>,headers:HeaderMap,Query(q):Query<MembersQuery>)->Result<Json<Vec<Member>>> {
    let token=bearer(&headers)?; Ok(Json(blocking(move||board.members(&token,&q.channel)).await?))
}
async fn http_display_name(State(board):State<Board>,headers:HeaderMap,Json(request):Json<DisplayNameRequest>)->Result<StatusCode> {
    let token=bearer(&headers)?; blocking(move||board.set_display_name(&token,&request.name)).await?; Ok(StatusCode::NO_CONTENT)
}
async fn http_reactions(State(board):State<Board>,headers:HeaderMap)->Result<Json<Vec<Reaction>>> {
    let token=bearer(&headers)?; Ok(Json(blocking(move||board.reactions(&token)).await?))
}
async fn http_schedules(State(board):State<Board>,headers:HeaderMap)->Result<Json<Vec<ScheduledPost>>> {
    let token=bearer(&headers)?; Ok(Json(blocking(move||board.schedules(&token)).await?))
}
#[derive(Deserialize)]
struct EventsQuery { #[serde(default)] after:u64, #[serde(default)] wait_ms:u64 }
async fn http_events(State(board):State<Board>,headers:HeaderMap,Query(q):Query<EventsQuery>)->Result<Json<Vec<Event>>> {
    let token=bearer(&headers)?;
    if q.wait_ms>30000 { return Err(invalid("wait_ms must be at most 30000")); }
    let deadline=tokio::time::Instant::now()+Duration::from_millis(q.wait_ms);
    loop {
        let b=board.clone(); let t=token.clone();
        let events=blocking(move||b.events(&t,q.after)).await?;
        if !events.is_empty() || tokio::time::Instant::now()>=deadline { return Ok(Json(events)); }
        tokio::time::sleep_until(std::cmp::min(deadline,tokio::time::Instant::now()+Duration::from_millis(200))).await;
    }
}
async fn http_action(State(board):State<Board>,headers:HeaderMap,Json(action):Json<Action>)->Result<Json<Outcome>> {
    let token=bearer(&headers)?;
    let header=|name| -> Result<Option<String>> { headers.get(name).map(|v|v.to_str().map(str::to_owned).map_err(|_|invalid("invalid header encoding"))).transpose() };
    let profile=header("x-neochat-profile")?; let key=header("idempotency-key")?;
    Ok(Json(blocking(move||board.action(&token,profile.as_deref(),key.as_deref(),action)).await?))
}
async fn http_invite(State(board):State<Board>,headers:HeaderMap,Json(request):Json<InviteRequest>)->Result<Json<Invite>> {
    let token=bearer(&headers)?; Ok(Json(blocking(move||board.create_invite(&token,request)).await?))
}
async fn http_revoke(State(board):State<Board>,headers:HeaderMap,Path(id):Path<u64>)->Result<StatusCode> {
    let token=bearer(&headers)?; blocking(move||board.revoke_invite(&token,id)).await?; Ok(StatusCode::NO_CONTENT)
}