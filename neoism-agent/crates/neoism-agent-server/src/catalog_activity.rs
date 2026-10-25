//! Transport-only projection of existing conversation-family authorities.
use crate::state::AppState;
use neoism_agent_core::{
    CatalogActivity, CatalogBranchSnapshot, CatalogFamilySnapshot, SessionInfo, SessionStatus,
};
use std::collections::{HashMap, HashSet};

#[derive(serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct CatalogSessionInfo {
    #[serde(flatten)]
    pub info: SessionInfo,
    pub catalog_activity: CatalogActivity,
}

impl std::ops::Deref for CatalogSessionInfo {
    type Target = SessionInfo;
    fn deref(&self) -> &SessionInfo {
        &self.info
    }
}

/// One membership read shared by the entire page/baseline. Never consult tools
/// or infer Running from an unfinished execution row.
pub(crate) fn families(sessions: &[SessionInfo]) -> HashMap<String, HashSet<String>> {
    let by_id: HashMap<_, _> = sessions.iter().map(|s| (s.id.as_str(), s)).collect();
    let mut families = HashMap::<String, HashSet<String>>::new();
    for session in sessions {
        let mut current = session;
        let mut visited = HashSet::new();
        while visited.insert(current.id.as_str()) {
            let Some(parent) = current
                .parent_id
                .as_ref()
                .and_then(|p| by_id.get(p.as_str()))
            else {
                break;
            };
            current = parent;
        }
        let root = session
            .extra
            .get(crate::execution_activity::EXECUTION_ROOT_KEY)
            .and_then(|v| v.as_str())
            .unwrap_or(current.id.as_str());
        families
            .entry(root.to_string())
            .or_default()
            .insert(session.id.to_string());
    }
    families
}

/// Stable iterative preorder. Each branch/edge is visited at most once after
/// sorting siblings; disconnected/cyclic components are retained deterministically.
fn branch_preorder(branches: &[neoism_agent_core::SubtaskLifecycleSnapshot], root: &str) -> Vec<usize> {
    let ids: HashSet<_> = branches.iter().map(|branch| branch.session_id.as_str()).collect();
    let mut sorted: Vec<_> = (0..branches.len()).collect();
    sorted.sort_by(|&a, &b| {
        branches[a].started_at.cmp(&branches[b].started_at)
            .then_with(|| branches[a].session_id.cmp(&branches[b].session_id))
    });
    let mut children: HashMap<&str, Vec<usize>> = HashMap::new();
    let mut roots = Vec::new();
    for &index in &sorted {
        let parent = branches[index].parent_session_id.as_str();
        if parent == root || !ids.contains(parent) {
            roots.push(index);
        } else {
            children.entry(parent).or_default().push(index);
        }
    }
    let mut visited = HashSet::new();
    let mut ordered = Vec::with_capacity(branches.len());
    // All real roots precede fallback seeds for corrupt cyclic components.
    for seed in roots.into_iter().chain(sorted) {
        let mut stack = vec![seed];
        while let Some(index) = stack.pop() {
            if !visited.insert(index) { continue; }
            ordered.push(index);
            if let Some(descendants) = children.get(branches[index].session_id.as_str()) {
                stack.extend(descendants.iter().rev().copied());
            }
        }
    }
    ordered
}

pub(crate) async fn activity(
    state: &AppState,
    root: &str,
    family: &HashSet<String>,
) -> anyhow::Result<CatalogActivity> {
    let runtime = state.inner.store.get_session_runtime_snapshot(root).await?;
    let mut family = family.clone();
    family.insert(root.to_string());
    family.extend(runtime.branches.iter().map(|b| b.session_id.clone()));
    if state
        .inner
        .permissions
        .read()
        .await
        .values()
        .any(|p| family.contains(&p.session_id))
    {
        return Ok(CatalogActivity::Permission);
    }
    // Child rows have their own foreground indicator. Only the root's own
    // provider/run activity should animate its catalog row.
    if runtime
        .execution
        .as_ref()
        .and_then(|execution| execution.session_activities.get(root))
        .is_some_and(|activity| !activity.active_segments.is_empty())
    {
        return Ok(CatalogActivity::Running);
    }
    if state.inner.session_coordinator.active_run(root).await.is_some() {
        return Ok(CatalogActivity::Running);
    }
    let busy = state
        .inner
        .statuses
        .read()
        .await
        .get(root)
        .is_some_and(|status| !matches!(status, SessionStatus::Idle));
    if busy
        || state.inner.session_coordinator.worker_active(root).await
        || crate::session_queue::queued_prompt_count(state, root).await > 0
    {
        return Ok(CatalogActivity::Running);
    }
    let (_, jobs) = crate::background_job::running_jobs_for_family(state, &family).await;
    Ok(if jobs.is_empty() {
        CatalogActivity::Idle
    } else {
        CatalogActivity::Background
    })
}

/// Per-connection routing cache only; activity is always freshly read.
pub(crate) struct CatalogProjection {
    sessions: Vec<SessionInfo>,
    session_indices: HashMap<String, usize>,
    membership: HashMap<String, HashSet<String>>,
    requests: HashMap<String, String>,
}

impl CatalogProjection {
    pub(crate) async fn load(state: &AppState) -> anyhow::Result<Self> {
        let sessions = state.inner.store.list_catalog_sessions().await?;
        let membership = families(&sessions);
        let requests = state
            .inner
            .permissions
            .read()
            .await
            .values()
            .map(|p| (p.id.clone(), p.session_id.clone()))
            .collect();
        Ok(Self {
            session_indices: Self::index_sessions(&sessions),
            sessions,
            membership,
            requests,
        })
    }

    fn index_sessions(sessions: &[SessionInfo]) -> HashMap<String, usize> {
        sessions.iter().enumerate()
            .map(|(index, session)| (session.id.to_string(), index))
            .collect()
    }

    fn session_metadata(&self, id: &str) -> Option<&SessionInfo> {
        self.session_indices.get(id).and_then(|index| self.sessions.get(*index))
    }

    pub(crate) async fn baseline(
        &self,
        state: &AppState,
        directory: Option<&str>,
        claims: Option<&crate::caller::CallerClaims>,
    ) -> anyhow::Result<Vec<neoism_agent_core::EventPayload>> {
        let mut events = Vec::new();
        for root in self.sessions.iter().filter(|s| s.parent_id.is_none()) {
            if let Some(root) =
                authorized_root(state, root.clone(), directory, claims).await?
            {
                events.push(self.family_event(state, &root, false).await?);
                events.push(self.activity_event(state, root.id.as_str()).await?);
            }
        }
        Ok(events)
    }

    /// Runtime supplies the atomic execution/revision/branch roster; queued event
    /// payloads are only invalidation signals, never lifecycle authority.
    async fn family_event(
        &self,
        state: &AppState,
        root: &SessionInfo,
        deleted: bool,
    ) -> anyhow::Result<neoism_agent_core::EventPayload> {
        let runtime = state.inner.store.get_session_runtime_snapshot(root.id.as_str()).await?;
        let parents: HashMap<_, _> = runtime.branches.iter()
            .map(|branch| (branch.session_id.as_str(), branch.parent_session_id.as_str()))
            .collect();
        let pending: HashSet<_> = state.inner.permissions.read().await.values()
            .map(|request| request.session_id.clone()).collect();
        let branches = if deleted {
            Vec::new()
        } else {
            branch_preorder(&runtime.branches, root.id.as_str()).into_iter().map(|index| {
                let branch = &runtime.branches[index];
                let metadata = self.session_metadata(&branch.session_id);
                // Direct children are depth one. Bound traversal even for corrupt
                // cycles or orphaned parents; never recurse through the roster.
                let mut depth = 1usize;
                let mut parent = branch.parent_session_id.as_str();
                let mut visited = HashSet::from([branch.session_id.as_str()]);
                while parent != root.id.as_str() && visited.insert(parent) {
                    let Some(next) = parents.get(parent) else { break };
                    depth = depth.saturating_add(1).min(32);
                    if depth == 32 { break; }
                    parent = next;
                }
                CatalogBranchSnapshot {
                    session_id: branch.session_id.clone(),
                    parent_session_id: branch.parent_session_id.clone(),
                    status: branch.status.clone(),
                    started_at: branch.started_at,
                    title: metadata.map(|session| session.title.clone()).unwrap_or_default(),
                    agent: metadata.and_then(|session| session.agent.clone()),
                    depth,
                    permission_pending: pending.contains(&branch.session_id),
                }
            }).collect()
        };
        let snapshot = CatalogFamilySnapshot {
            directory: root.directory.clone(),
            execution_id: runtime.execution.map(|execution| execution.execution_id),
            family_revision: runtime.family_revision,
            branches,
        };
        Ok(neoism_agent_core::EventPayload::new(
            neoism_agent_core::event_type::SESSION_CATALOG_FAMILY,
            serde_json::json!({"sessionID": root.id, "snapshot": snapshot}),
        ))
    }

    async fn activity_event(
        &self,
        state: &AppState,
        root: &str,
    ) -> anyhow::Result<neoism_agent_core::EventPayload> {
        let family = self
            .membership
            .get(root)
            .cloned()
            .unwrap_or_else(|| HashSet::from([root.to_string()]));
        let activity = activity(state, root, &family).await?;
        Ok(neoism_agent_core::EventPayload::new(
            neoism_agent_core::event_type::SESSION_CATALOG_ACTIVITY,
            serde_json::json!({"sessionID": root, "activity": activity}),
        ))
    }

    pub(crate) async fn project_event(
        &mut self,
        state: &AppState,
        mut event: neoism_agent_core::EventPayload,
        directory: Option<&str>,
        claims: Option<&crate::caller::CallerClaims>,
    ) -> anyhow::Result<Vec<neoism_agent_core::EventPayload>> {
        use neoism_agent_core::event_type as et;
        let lifecycle = matches!(
            event.kind.as_str(),
            et::SESSION_CREATED | et::SESSION_UPDATED | et::SESSION_DELETED
        );
        if !lifecycle
            && !matches!(
                event.kind.as_str(),
                et::SESSION_STATUS
                    | et::SESSION_EXECUTION_UPDATED
                    | et::SESSION_BACKGROUND_TASKS_UPDATED
                    | et::PERMISSION_ASKED
                    | et::PERMISSION_REPLIED
            )
        {
            return Ok(Vec::new());
        }
        let payload_info = event
            .properties
            .get("info")
            .cloned()
            .and_then(|v| serde_json::from_value::<SessionInfo>(v).ok());
        let p = &event.properties;
        let request_id = p
            .get("requestID")
            .or_else(|| p.get("requestId"))
            .or_else(|| p.get("id"))
            .and_then(|v| v.as_str());
        let source = p["sessionID"]
            .as_str()
            .or_else(|| p["sessionId"].as_str())
            .or_else(|| p["info"]["sessionId"].as_str())
            .or_else(|| p["info"]["sessionID"].as_str())
            .or_else(|| payload_info.as_ref().map(|s| s.id.as_str()))
            .or_else(|| {
                request_id.and_then(|id| self.requests.get(id).map(String::as_str))
            })
            .map(str::to_string);
        if event.kind == et::PERMISSION_REPLIED {
            if let Some(id) = request_id {
                self.requests.remove(id);
            }
        } else if event.kind == et::PERMISSION_ASKED {
            if let (Some(id), Some(source)) = (request_id, source.as_ref()) {
                self.requests.insert(id.to_string(), source.clone());
            }
        }
        let prior_root = source.as_ref().and_then(|source| {
            self.membership
                .iter()
                .find(|(_, family)| family.contains(source))
                .map(|(root, _)| root.clone())
        });
        let prior_info = source.as_ref()
            .and_then(|source| self.session_metadata(source)).cloned();
        // Lifecycle events invalidate one canonical row, not the entire catalog.
        // Never trust queued metadata as authority: a later rename/delete may
        // already have committed before this event reaches the subscriber.
        let current_info = match source.as_ref() {
            Some(source) => state.inner.store.catalog_session(source).await?,
            None => None,
        };
        if lifecycle {
            if let Some(source) = source.as_ref() {
                if let Some(index) = self.session_indices.get(source).copied() {
                    if let Some(info) = current_info.as_ref() {
                        self.sessions[index] = info.clone();
                    } else {
                        self.sessions.swap_remove(index);
                    }
                } else if let Some(info) = current_info.as_ref() {
                    self.sessions.push(info.clone());
                }
                self.session_indices = Self::index_sessions(&self.sessions);
                self.membership = families(&self.sessions);
            }
        }
        let source_exists = current_info.is_some();
        let info = current_info.or_else(|| {
            (event.kind == et::SESSION_DELETED)
                .then_some(payload_info.or(prior_info))
                .flatten()
        });
        let root_id = match info.as_ref() {
            Some(info) => {
                Some(crate::execution_activity::root_session_id(state, info).await)
            }
            None => prior_root,
        };
        // A sessionless fallback reply may outlive the cached request. Re-read
        // all authorized roots instead of dropping a permission-clear edge.
        if root_id.is_none() && event.kind == et::PERMISSION_REPLIED {
            *self = Self::load(state).await?;
            return self.baseline(state, directory, claims).await;
        }
        let Some(root_id) = root_id else {
            return Ok(Vec::new());
        };
        let root = if let Some(info) = info.as_ref().filter(|s| s.id.as_str() == root_id) {
            Some(info.clone())
        } else {
            state.inner.store.catalog_session(&root_id).await?
        };
        let Some(root) = root else {
            return Ok(Vec::new());
        };
        let Some(root) = authorized_root(state, root, directory, claims).await? else {
            return Ok(Vec::new());
        };
        let family_event = self.family_event(
            state,
            &root,
            event.kind == et::SESSION_DELETED && source.as_deref() == Some(root_id.as_str()),
        ).await?;
        // New cross-directory child may arrive without a lifecycle edge yet.
        if let Some(source) = source.filter(|_| !lifecycle || source_exists) {
            self.membership
                .entry(root_id.clone())
                .or_default()
                .insert(source);
        }
        let projected = self.activity_event(state, &root_id).await?;
        let mut events = Vec::new();
        if lifecycle
            && info
                .as_ref()
                .is_some_and(|s| s.parent_id.is_none() && s.id.as_str() == root_id)
        {
            let mut value = serde_json::to_value(root)?;
            value["catalogActivity"] = projected.properties["activity"].clone();
            event.properties["info"] = value;
            events.push(event);
        }
        events.push(family_event);
        events.push(projected);
        Ok(events)
    }
}

async fn authorized_root(
    state: &AppState,
    root: SessionInfo,
    directory: Option<&str>,
    claims: Option<&crate::caller::CallerClaims>,
) -> anyhow::Result<Option<SessionInfo>> {
    let mut hydrated = [root];
    state
        .inner
        .store
        .hydrate_host_associations(&mut hydrated)
        .await?;
    let [root] = hydrated;
    Ok((root.parent_id.is_none()
        && directory.is_none_or(|directory| root.directory == directory)
        && crate::external_agent::catalog::is_neoism_owned_root(&root)
        && claims.is_none_or(|claims| crate::caller::allows_session(claims, &root)))
    .then_some(root))
}

/// Single-session reads retain the requested ID as the activity root, including
/// child GETs. Do not substitute the child's execution/parent root here.
/// Runtime-only branches are merged by `activity`, just as in catalog projection.
pub(crate) async fn project_one(
    state: &AppState,
    info: SessionInfo,
) -> anyhow::Result<CatalogSessionInfo> {
    let family = state.inner.store.catalog_family_members(info.id.as_str()).await?;
    let catalog_activity = activity(state, info.id.as_str(), &family).await?;
    Ok(CatalogSessionInfo { info, catalog_activity })
}

pub(crate) async fn project(
    state: &AppState,
    items: Vec<SessionInfo>,
) -> anyhow::Result<Vec<CatalogSessionInfo>> {
    let mut projected = Vec::with_capacity(items.len());
    for info in items {
        projected.push(project_one(state, info).await?);
    }
    Ok(projected)
}
