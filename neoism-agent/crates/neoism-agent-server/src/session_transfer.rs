//! Portable agent-session transfer between hosts (Wave 3C of "work from
//! anywhere").
//!
//! The agent-server persists each session as a handful of durable SQLite rows
//! (see [`crate::state::SessionStore`]):
//!
//! * one `sessions` row — the [`SessionInfo`] (title, model, goal, the
//!   workspace `directory`/`path`, …) serialized as `info_json`,
//! * zero-or-more `messages` rows — the conversation transcript
//!   ([`MessageWithParts`]), and
//! * zero-or-more `prompt_queue` rows — turns enqueued while the session was
//!   busy ([`PromptRequest`]). `resume_prompt_queues` replays these on boot.
//!
//! That is *all* of the host-independent state needed to continue a
//! conversation: shells respawn in `cwd` and the agent simply resumes (decision
//! #3 in `WORK_FROM_ANYWHERE.md`). [`export_session`] gathers those rows into a
//! serializable [`SessionBundle`]; [`import_session`] writes the bundle into a
//! second host's store so the existing resume path picks it up.
//!
//! ## Path rebinding
//!
//! The only host-specific data in the bundle is the absolute workspace
//! `directory`. A session's `directory` is `<workspace_root>/<path>`, where
//! `path` (already stored on [`SessionInfo`]) is the worktree-relative subpath
//! and `workspace_root` is the git worktree root on the exporting host. We
//! record `workspace_root` separately and strip it from every absolute path we
//! ship, so the importing host can re-root the session under *its own*
//! workspace checkout via `target_workspace_root`.

use std::path::Path;

use anyhow::{anyhow, Context};
use neoism_agent_core::{
    AssistantMessage, MessageInfo, MessageWithParts, PromptRequest, SessionId,
    SessionInfo,
};
use serde::{Deserialize, Serialize};

use crate::state::AppState;

/// Wire format version for [`SessionBundle`]. Bumped if the shape changes in a
/// way that older importers cannot read.
pub const SESSION_BUNDLE_VERSION: u32 = 1;

/// A self-contained, host-independent snapshot of a single agent session.
///
/// Everything required to reconstruct the session on another host and have the
/// agent continue the same conversation: the [`SessionInfo`], the full
/// transcript, and any still-queued prompts. Absolute paths are recorded
/// relative to [`SessionBundle::workspace_root`] so the importer can rebind
/// them to its own checkout.
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionBundle {
    /// Wire-format version; see [`SESSION_BUNDLE_VERSION`].
    pub version: u32,
    /// The session record. Its `directory`/`path` are rewritten on import.
    pub session: SessionInfo,
    /// Conversation transcript, oldest-first (mirrors `list_messages`).
    pub messages: Vec<MessageWithParts>,
    /// Prompts still queued for this session, in queue order. `import_session`
    /// re-enqueues them so the resume path drains them on the new host.
    pub queued_prompts: Vec<PromptRequest>,
    /// Absolute workspace (git worktree) root on the exporting host, if known.
    /// Absolute paths inside the bundle are stripped of this prefix so the
    /// importer can re-root them under `target_workspace_root`. `None` when the
    /// session's directory is not under a recognizable workspace root.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub workspace_root: Option<String>,
}

impl SessionBundle {
    /// Serialize to pretty JSON for transport / on-disk handoff.
    pub fn to_json(&self) -> anyhow::Result<String> {
        serde_json::to_string_pretty(self).context("failed to serialize SessionBundle")
    }

    /// Deserialize a bundle produced by [`SessionBundle::to_json`].
    pub fn from_json(raw: &str) -> anyhow::Result<Self> {
        serde_json::from_str(raw).context("failed to deserialize SessionBundle")
    }
}

/// Gather everything needed to reconstruct `session_id` on another host.
///
/// Reads the session record, transcript and queued prompts from this host's
/// store and packages them as a [`SessionBundle`] with host-specific paths made
/// portable. Fails if the session does not exist.
pub async fn export_session(
    state: &AppState,
    session_id: &str,
) -> anyhow::Result<SessionBundle> {
    let store = &state.inner.store;
    let session = store
        .get_session(session_id)
        .await?
        .ok_or_else(|| anyhow!("session {session_id} not found"))?;
    let messages = store.list_messages(session_id).await?;
    let queued_prompts = store.list_queued_prompts(session_id).await?;

    // Derive the workspace root: directory == <root>/<path>. Stripping `path`
    // from the tail of `directory` yields the root the importer will replace.
    let workspace_root = workspace_root_of(&session);

    Ok(SessionBundle {
        version: SESSION_BUNDLE_VERSION,
        session,
        messages,
        queued_prompts,
        workspace_root,
    })
}

/// Export every session whose workspace root matches `workspace_root` into a
/// portable [`SessionBundle`], in store order (most-recently-updated first, as
/// [`crate::state::SessionStore::list_sessions`] yields).
///
/// This is the entry point a *promote* uses: it knows the workspace checkout
/// path it is relocating, not the individual session ids, so it asks for every
/// session living under that root. A session matches when its derived workspace
/// root (its `directory` with the trailing worktree-relative `path` stripped —
/// the same value [`export_session`] records in the bundle) equals
/// `workspace_root` after path normalization. Sessions under any other root are
/// excluded.
pub async fn export_sessions_under_workspace_root(
    state: &AppState,
    workspace_root: &str,
) -> anyhow::Result<Vec<SessionBundle>> {
    let target = normalize_root(workspace_root);
    let sessions = state.inner.store.list_sessions().await?;
    let mut bundles = Vec::new();
    for session in sessions {
        if workspace_root_of(&session)
            .map(|root| normalize_root(&root) == target)
            .unwrap_or(false)
        {
            bundles.push(export_session(state, session.id.as_str()).await?);
        }
    }
    Ok(bundles)
}

/// Normalize a workspace root for equality comparison: unify path separators and
/// drop a trailing slash so `/srv/work/proj`, `/srv/work/proj/` and the
/// backslash spelling all compare equal.
fn normalize_root(root: &str) -> String {
    root.replace('\\', "/").trim_end_matches('/').to_string()
}

/// Write `bundle` into this host's session store, rebinding all workspace paths
/// to `target_workspace_root`, so a subsequent `resume_prompt_queues` (or
/// explicit resume) continues the conversation.
///
/// Returns the imported [`SessionId`] (preserved from the bundle, so child /
/// parent relationships and event aggregates stay intact). If a session with
/// the same id already exists it is updated in place; otherwise it is inserted.
/// Existing transcript / queue rows for that id are replaced with the bundle's.
pub async fn import_session(
    state: &AppState,
    bundle: SessionBundle,
    target_workspace_root: impl AsRef<Path>,
) -> anyhow::Result<SessionId> {
    if bundle.version > SESSION_BUNDLE_VERSION {
        return Err(anyhow!(
            "session bundle version {} is newer than supported version {}",
            bundle.version,
            SESSION_BUNDLE_VERSION
        ));
    }

    let target_root = target_workspace_root.as_ref();
    let old_root = bundle.workspace_root.as_deref();

    let mut session = bundle.session;
    let session_id = session.id.clone();

    // Re-root the session directory under the importing host's workspace.
    session.directory = rebind_directory(target_root, session.path.as_deref());

    // Rewrite absolute paths embedded in the transcript (assistant cwd/root).
    let mut messages = bundle.messages;
    for message in &mut messages {
        if let MessageInfo::Assistant(assistant) = &mut message.info {
            rebind_assistant_paths(assistant, old_root, target_root);
        }
    }

    let store = &state.inner.store;

    // Upsert the session record.
    if store.get_session(session_id.as_str()).await?.is_some() {
        store.update_session(&session).await?;
        // Replace any prior transcript / queue so re-import is idempotent.
        store.delete_session_messages(session_id.as_str()).await?;
        store.clear_queued_prompts(session_id.as_str()).await?;
    } else {
        store.insert_session(&session).await?;
    }

    // Restore the transcript in order.
    for message in &messages {
        store.append_message(session_id.as_str(), message).await?;
    }

    // Re-enqueue still-pending prompts so resume_prompt_queues drains them.
    for request in &bundle.queued_prompts {
        store.enqueue_prompt(session_id.as_str(), request).await?;
    }

    Ok(session_id)
}

/// Compute the absolute workspace root of a session: its `directory` with the
/// trailing worktree-relative `path` removed. Returns `None` when `path` is
/// present but does not match the directory tail (so we never ship a wrong
/// root). When `path` is absent the directory *is* the root.
fn workspace_root_of(session: &SessionInfo) -> Option<String> {
    let Some(rel) = session.path.as_deref().filter(|p| !p.is_empty()) else {
        // directory already is the workspace root.
        return Some(session.directory.clone());
    };
    strip_relative_suffix(&session.directory, rel)
}

/// Remove a forward-slash relative suffix from the tail of an absolute path,
/// tolerating either path separator in `directory`.
fn strip_relative_suffix(directory: &str, relative: &str) -> Option<String> {
    let normalized_dir = directory.replace('\\', "/");
    let relative = relative.trim_matches('/');
    let trimmed = normalized_dir.trim_end_matches('/');
    let suffix = format!("/{relative}");
    trimmed
        .strip_suffix(&suffix)
        .map(|root| root.to_string())
        .or_else(|| (trimmed == relative).then(String::new))
}

/// Join the importing host's workspace root with the session's worktree-relative
/// subpath to form the new absolute working directory.
fn rebind_directory(target_root: &Path, relative: Option<&str>) -> String {
    match relative.filter(|p| !p.is_empty()) {
        Some(rel) => {
            let mut path = target_root.to_path_buf();
            for segment in rel.split('/').filter(|s| !s.is_empty()) {
                path.push(segment);
            }
            path_to_string(&path)
        }
        None => path_to_string(target_root),
    }
}

/// Rewrite an assistant message's `cwd`/`root` from the old workspace root to
/// the new one. Paths not under `old_root` are left untouched (they may point
/// outside the workspace and we must not corrupt them).
fn rebind_assistant_paths(
    assistant: &mut AssistantMessage,
    old_root: Option<&str>,
    target_root: &Path,
) {
    assistant.path.cwd = rebind_absolute(&assistant.path.cwd, old_root, target_root);
    assistant.path.root = rebind_absolute(&assistant.path.root, old_root, target_root);
}

/// Replace a leading `old_root` prefix on `value` with `target_root`. If
/// `old_root` is unknown or `value` is not under it, `value` is returned
/// unchanged.
fn rebind_absolute(value: &str, old_root: Option<&str>, target_root: &Path) -> String {
    let Some(old_root) = old_root else {
        return value.to_string();
    };
    let normalized = value.replace('\\', "/");
    let old_root_norm = old_root.replace('\\', "/");
    let old_trimmed = old_root_norm.trim_end_matches('/');
    if normalized == old_trimmed {
        return path_to_string(target_root);
    }
    let prefix = format!("{old_trimmed}/");
    match normalized.strip_prefix(&prefix) {
        Some(rest) => {
            let mut path = target_root.to_path_buf();
            for segment in rest.split('/').filter(|s| !s.is_empty()) {
                path.push(segment);
            }
            path_to_string(&path)
        }
        None => value.to_string(),
    }
}

fn path_to_string(path: &Path) -> String {
    path.to_string_lossy().replace('\\', "/")
}

#[cfg(test)]
mod tests {
    use super::*;
    use neoism_agent_core::{
        AssistantPath, CompletedTime, CreatedTime, Id, IdKind, Part, PromptPart,
        TextPart, TimeInfo, TokenUsage, UserMessage, UserModel,
    };
    use std::collections::BTreeMap;

    fn sample_session(directory: &str, path: Option<&str>) -> SessionInfo {
        SessionInfo {
            id: neoism_agent_core::new_session_id(),
            slug: "transfer-test".to_string(),
            project_id: "global".to_string(),
            workspace_id: None,
            directory: directory.to_string(),
            path: path.map(ToString::to_string),
            parent_id: None,
            title: "Portable session".to_string(),
            agent: Some("build".to_string()),
            model: None,
            version: "0.1".to_string(),
            time: TimeInfo {
                created: 10,
                updated: 20,
                compacting: None,
                archived: None,
            },
            permission: None,
            extra: BTreeMap::new(),
        }
    }

    fn user_message(session_id: &SessionId, text: &str) -> MessageWithParts {
        let message_id = Id::ascending(IdKind::Message);
        MessageWithParts {
            info: MessageInfo::User(UserMessage {
                id: message_id.clone(),
                session_id: session_id.clone(),
                time: CreatedTime { created: 1 },
                agent: "build".to_string(),
                model: UserModel {
                    provider_id: "neoism".to_string(),
                    model_id: "stub".to_string(),
                    variant: None,
                },
                system: None,
                tools: None,
                author: None,
            }),
            parts: vec![Part::Text(TextPart {
                id: Id::ascending(IdKind::Part),
                session_id: session_id.clone(),
                message_id,
                text: text.to_string(),
                synthetic: None,
                time: None,
            })],
        }
    }

    fn assistant_message(
        session_id: &SessionId,
        cwd: &str,
        root: &str,
    ) -> MessageWithParts {
        MessageWithParts {
            info: MessageInfo::Assistant(AssistantMessage {
                id: Id::ascending(IdKind::Message),
                session_id: session_id.clone(),
                time: CompletedTime {
                    created: 2,
                    completed: Some(3),
                },
                parent_id: Id::ascending(IdKind::Message),
                mode: "build".to_string(),
                agent: "build".to_string(),
                path: AssistantPath {
                    cwd: cwd.to_string(),
                    root: root.to_string(),
                },
                cost: 0.0,
                tokens: TokenUsage::default(),
                model_id: "stub".to_string(),
                provider_id: "neoism".to_string(),
                finish: None,
                error: None,
            }),
            parts: Vec::new(),
        }
    }

    fn text_prompt(text: &str) -> PromptRequest {
        PromptRequest {
            message_id: None,
            model: None,
            agent: None,
            no_reply: false,
            system: None,
            tools: None,
            author: None,
            parts: vec![PromptPart::Text {
                text: text.to_string(),
            }],
        }
    }

    #[test]
    fn workspace_root_strips_relative_subpath() {
        let session = sample_session("/home/alice/proj/nested", Some("nested"));
        assert_eq!(
            workspace_root_of(&session).as_deref(),
            Some("/home/alice/proj")
        );
    }

    #[test]
    fn workspace_root_is_directory_when_no_subpath() {
        let session = sample_session("/home/alice/proj", None);
        assert_eq!(
            workspace_root_of(&session).as_deref(),
            Some("/home/alice/proj")
        );
    }

    #[test]
    fn rebind_directory_rejoins_subpath() {
        let dir = rebind_directory(Path::new("/srv/work/proj"), Some("nested"));
        assert_eq!(dir, "/srv/work/proj/nested");
    }

    #[test]
    fn rebind_absolute_only_rewrites_under_old_root() {
        let target = Path::new("/srv/work/proj");
        // Under the old root -> rebased.
        assert_eq!(
            rebind_absolute(
                "/home/alice/proj/src/main.rs",
                Some("/home/alice/proj"),
                target
            ),
            "/srv/work/proj/src/main.rs"
        );
        // Exactly the old root -> becomes the new root.
        assert_eq!(
            rebind_absolute("/home/alice/proj", Some("/home/alice/proj"), target),
            "/srv/work/proj"
        );
        // Outside the old root -> untouched.
        assert_eq!(
            rebind_absolute("/etc/hosts", Some("/home/alice/proj"), target),
            "/etc/hosts"
        );
        // Unknown old root -> untouched.
        assert_eq!(
            rebind_absolute("/home/alice/proj/x", None, target),
            "/home/alice/proj/x"
        );
    }

    #[tokio::test]
    async fn export_import_round_trip_rebinds_and_resumes() {
        // First host: persist a session, transcript, and a queued prompt.
        let source_db = std::env::temp_dir().join(format!(
            "neoism-transfer-src-{}.sqlite3",
            Id::ascending(IdKind::Event)
        ));
        cleanup(&source_db);
        let source = AppState::open_database(source_db.clone()).await.unwrap();

        let session = sample_session("/home/alice/proj/nested", Some("nested"));
        let session_id = session.id.clone();
        source.inner.store.insert_session(&session).await.unwrap();
        source
            .inner
            .store
            .append_message(
                session_id.as_str(),
                &user_message(&session_id, "hello from host A"),
            )
            .await
            .unwrap();
        source
            .inner
            .store
            .append_message(
                session_id.as_str(),
                &assistant_message(
                    &session_id,
                    "/home/alice/proj/nested",
                    "/home/alice/proj",
                ),
            )
            .await
            .unwrap();
        source
            .inner
            .store
            .enqueue_prompt(session_id.as_str(), &text_prompt("continue please"))
            .await
            .unwrap();

        // Export, round-trip through JSON to prove it is fully serializable.
        let bundle = export_session(&source, session_id.as_str()).await.unwrap();
        assert_eq!(bundle.workspace_root.as_deref(), Some("/home/alice/proj"));
        assert_eq!(bundle.messages.len(), 2);
        assert_eq!(bundle.queued_prompts.len(), 1);
        let wire = bundle.to_json().unwrap();
        let bundle = SessionBundle::from_json(&wire).unwrap();

        // Second host: a fresh, independent store.
        let target_db = std::env::temp_dir().join(format!(
            "neoism-transfer-dst-{}.sqlite3",
            Id::ascending(IdKind::Event)
        ));
        cleanup(&target_db);
        let target = AppState::open_database(target_db.clone()).await.unwrap();

        // Sanity: the session is absent before import.
        assert!(target
            .inner
            .store
            .get_session(session_id.as_str())
            .await
            .unwrap()
            .is_none());

        let imported = import_session(&target, bundle, "/srv/work/proj")
            .await
            .unwrap();
        assert_eq!(imported, session_id);

        // The session is present with its directory rebased onto the new root.
        let restored = target
            .inner
            .store
            .get_session(session_id.as_str())
            .await
            .unwrap()
            .expect("imported session present");
        assert_eq!(restored.directory, "/srv/work/proj/nested");
        assert_eq!(restored.path.as_deref(), Some("nested"));
        // Non-path fields are preserved verbatim.
        assert_eq!(restored.title, "Portable session");
        assert_eq!(restored.slug, "transfer-test");
        assert_eq!(restored.time.created, 10);

        // Transcript preserved, assistant paths rebased.
        let messages = target
            .inner
            .store
            .list_messages(session_id.as_str())
            .await
            .unwrap();
        assert_eq!(messages.len(), 2);
        match &messages[1].info {
            MessageInfo::Assistant(assistant) => {
                assert_eq!(assistant.path.cwd, "/srv/work/proj/nested");
                assert_eq!(assistant.path.root, "/srv/work/proj");
            }
            other => panic!("expected assistant message, got {other:?}"),
        }

        // The queued prompt is present so resume_prompt_queues will drain it.
        let queued = target
            .inner
            .store
            .list_queued_prompts(session_id.as_str())
            .await
            .unwrap();
        assert_eq!(queued.len(), 1);
        assert!(target
            .inner
            .store
            .queued_session_ids()
            .await
            .unwrap()
            .contains(&session_id.to_string()));

        source.inner.store.close().await;
        target.inner.store.close().await;
        cleanup(&source_db);
        cleanup(&target_db);
    }

    fn cleanup(path: &Path) {
        let base = path.to_string_lossy();
        let _ = std::fs::remove_file(path);
        let _ = std::fs::remove_file(format!("{base}-wal"));
        let _ = std::fs::remove_file(format!("{base}-shm"));
    }
}
