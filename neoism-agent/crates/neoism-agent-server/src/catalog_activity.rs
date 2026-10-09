//! Transport-only projection of existing conversation-family authorities.
use crate::state::AppState;
use neoism_agent_core::{CatalogActivity, SessionInfo, SessionStatus};
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
    if runtime.branches.iter().any(|b| b.status == "outstanding")
        || runtime
            .execution
            .as_ref()
            .is_some_and(|e| !e.active_segments.is_empty())
    {
        return Ok(CatalogActivity::Running);
    }
    let runs = state.inner.session_coordinator.active_runs().await;
    if family.iter().any(|id| runs.contains_key(id)) {
        return Ok(CatalogActivity::Running);
    }
    // A terminal branch's status ledger can lag teardown. It is not a
    // resurrection authority; genuine resumes still win through live runs,
    // provider segments, workers, queues, or a new outstanding admission.
    // Keep root/untracked-child status fallback for external/manual runs.
    let terminal_children: HashSet<&str> = runtime
        .branches
        .iter()
        .filter(|b| matches!(b.status.as_str(), "completed" | "failed"))
        .map(|b| b.session_id.as_str())
        .collect();
    let busy = state
        .inner
        .statuses
        .read()
        .await
        .iter()
        .any(|(id, status)| {
            family.contains(id)
                && !matches!(status, SessionStatus::Idle)
                && (id.as_str() == root || !terminal_children.contains(id.as_str()))
        });
    if busy {
        return Ok(CatalogActivity::Running);
    }
    for id in &family {
        if state.inner.session_coordinator.worker_active(id).await
            || crate::session_queue::queued_prompt_count(state, id).await > 0
        {
            return Ok(CatalogActivity::Running);
        }
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
    membership: HashMap<String, HashSet<String>>,
    requests: HashMap<String, String>,
}

impl CatalogProjection {
    pub(crate) async fn load(state: &AppState) -> anyhow::Result<Self> {
        let sessions = state.inner.store.list_sessions().await?;
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
            sessions,
            membership,
            requests,
        })
    }

    pub(crate) async fn baseline(
        &self,
        state: &AppState,
        directory: &str,
        claims: Option<&crate::caller::CallerClaims>,
    ) -> anyhow::Result<Vec<neoism_agent_core::EventPayload>> {
        let mut events = Vec::new();
        for root in self.sessions.iter().filter(|s| s.parent_id.is_none()) {
            if let Some(root) =
                authorized_root(state, root.clone(), directory, claims).await?
            {
                events.push(self.activity_event(state, root.id.as_str()).await?);
            }
        }
        Ok(events)
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
        directory: &str,
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
        if lifecycle {
            self.sessions = state.inner.store.list_sessions().await?;
            self.membership = families(&self.sessions);
        }
        let info = match source.as_ref() {
            Some(source) => state.inner.store.get_session(source).await?.or_else(|| {
                (event.kind == et::SESSION_DELETED)
                    .then_some(payload_info)
                    .flatten()
            }),
            None => None,
        };
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
        let root = state.inner.store.get_session(&root_id).await?.or_else(|| {
            info.as_ref()
                .filter(|s| s.parent_id.is_none() && s.id.as_str() == root_id)
                .cloned()
        });
        let Some(root) = root else {
            return Ok(Vec::new());
        };
        let Some(root) = authorized_root(state, root, directory, claims).await? else {
            return Ok(Vec::new());
        };
        // New cross-directory child may arrive without a lifecycle edge yet.
        if let Some(source) = source {
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
        events.push(projected);
        Ok(events)
    }
}

async fn authorized_root(
    state: &AppState,
    root: SessionInfo,
    directory: &str,
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
        && root.directory == directory
        && crate::external_agent::catalog::is_neoism_owned_root(&root)
        && claims.is_none_or(|claims| crate::caller::allows_session(claims, &root)))
    .then_some(root))
}

pub(crate) async fn project(
    state: &AppState,
    items: Vec<SessionInfo>,
) -> anyhow::Result<Vec<CatalogSessionInfo>> {
    let membership = families(&state.inner.store.list_sessions().await?);
    let mut projected = Vec::with_capacity(items.len());
    for info in items {
        let family = membership
            .get(info.id.as_str())
            .cloned()
            .unwrap_or_else(|| HashSet::from([info.id.to_string()]));
        let catalog_activity = activity(state, info.id.as_str(), &family).await?;
        projected.push(CatalogSessionInfo {
            info,
            catalog_activity,
        });
    }
    Ok(projected)
}
