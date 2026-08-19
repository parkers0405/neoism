use super::*;

pub(crate) async fn workspace_from_git(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(req): Json<GitWorkspaceRequest>,
) -> Response {
    let principal = match cloud_auth::authorize_provision(&headers, &state.auth) {
        Ok(principal) => principal,
        Err(err) => return (err.status, err.message).into_response(),
    };
    let root = workspace_provision::workspaces_dir();
    let provisioned = match tokio::task::spawn_blocking(move || {
        workspace_provision::provision_from_git(req, &root)
    })
    .await
    {
        Ok(Ok(provisioned)) => provisioned,
        Ok(Err(err)) => return provision_error_response(err),
        Err(err) => {
            tracing::error!(error = %err, "workspace provision task failed");
            return (StatusCode::INTERNAL_SERVER_ERROR, "provision task failed")
                .into_response();
        }
    };

    let mut conn = ConnectionWorkspace::default();
    let outcome = workspace_handler::handle(
        &state.workspaces,
        &mut conn,
        None,
        None,
        WorkspaceClientMessage::OpenProjectRoot {
            path: provisioned.path.clone(),
            init_if_missing: false,
        },
    );
    let workspace = outcome.replies.iter().find_map(|reply| match reply {
        WorkspaceServerMessage::ProjectRootOpened { project_root } => {
            Some(project_root.clone())
        }
        _ => None,
    });
    let Some(workspace) = workspace else {
        let message = outcome
            .replies
            .iter()
            .find_map(|reply| match reply {
                WorkspaceServerMessage::Error { message } => Some(message.clone()),
                _ => None,
            })
            .unwrap_or_else(|| {
                "workspace provisioned but could not be registered".into()
            });
        return (StatusCode::INTERNAL_SERVER_ERROR, message).into_response();
    };

    tracing::info!(
        subject = %principal.subject,
        method = principal.method,
        path = %provisioned.path.display(),
        cloned = provisioned.cloned,
        reused = provisioned.reused,
        updated = provisioned.updated,
        "provisioned git workspace"
    );

    (
        StatusCode::OK,
        Json(GitWorkspaceResponse {
            workspace,
            git_url: provisioned.git_url,
            git_ref: provisioned.git_ref,
            cloned: provisioned.cloned,
            reused: provisioned.reused,
            updated: provisioned.updated,
            path: provisioned.path,
        }),
    )
        .into_response()
}

pub(crate) fn provision_error_response(err: ProvisionError) -> Response {
    let status = match &err {
        ProvisionError::MissingGitUrl | ProvisionError::InvalidGitUrl => {
            StatusCode::BAD_REQUEST
        }
        ProvisionError::Io(_) | ProvisionError::Git(_) => {
            StatusCode::INTERNAL_SERVER_ERROR
        }
    };
    (status, err.to_string()).into_response()
}

/// Request body for `POST /workspace/receive` — the target side of
/// "promote a workspace to this host". It composes the two existing
/// "work from anywhere" primitives: clone/reuse a git repo
/// ([`workspace_provision::provision_from_git`]) and then replay the
/// source's uncommitted working state on top
/// ([`workspace_snapshot::apply_snapshot`]).
///
/// `git_url` / `git_ref` mirror [`GitWorkspaceRequest`]. `snapshot`
/// carries the source host's tracked diff + untracked files. `pull`
/// defaults to true so receiving onto an already-provisioned target
/// refreshes history before the snapshot is replayed.
///
/// Wave 6B (the "adopt" half of a cross-host move) extends the body with
/// the workspace's identity and tabs, all defaulted so older senders stay
/// wire-compatible:
/// * `workspace_id` — the source's host-workspace id. When present the
///   target registers a `WorkspaceSummary` with the SAME id in its own
///   HOST>WORKSPACE>TABS tree (under [`workspace_handler::local_host_id`])
///   so clients tracking the workspace follow it across the move.
/// * `title` — display name carried over from the source.
/// * `sessions` — the source's tabs, cwds workspace-relative; re-created
///   here with cwds remapped under the new checkout.
/// * `preferences` — per-workplace UI preferences carried verbatim.
/// * `source_host` — informational; logged.
#[derive(Debug, Clone, Deserialize)]
pub struct ReceiveWorkspaceRequest {
    pub git_url: String,
    #[serde(default, rename = "ref")]
    pub git_ref: Option<String>,
    #[serde(default = "default_receive_pull")]
    pub pull: bool,
    #[serde(default)]
    pub snapshot: WorkspaceSnapshot,
    #[serde(default)]
    pub workspace_id: Option<String>,
    #[serde(default)]
    pub title: Option<String>,
    #[serde(default)]
    pub sessions: Vec<PortableSession>,
    #[serde(default)]
    pub preferences: Option<neoism_protocol::workspace::WorkplacePreferences>,
    #[serde(default)]
    pub source_host: Option<String>,
}

pub(crate) fn default_receive_pull() -> bool {
    true
}

/// Response body for `POST /workspace/receive`: the registered workspace,
/// the [`ApplyReport`] from replaying the snapshot (so the caller can see
/// which hunks landed / were rejected), and the on-disk path.
///
/// `Deserialize` so the source-side `/workspace/promote` handler can parse the
/// target's reply back into the report it surfaces to its own caller.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReceiveWorkspaceResponse {
    pub workspace: neoism_protocol::workspace::ProjectRootSummary,
    pub apply_report: ApplyReport,
    pub git_url: String,
    #[serde(rename = "ref")]
    pub git_ref: Option<String>,
    pub cloned: bool,
    pub reused: bool,
    pub updated: bool,
    pub path: StdPathBuf,
    /// Wave 6B: the adopted sessions ("tabs") as registered here, cwds
    /// remapped under `path`. Empty when the sender carried none.
    #[serde(default)]
    pub sessions: Vec<neoism_protocol::workspace::SessionSummary>,
    /// Wave 6B: the host-workspace registered in this daemon's
    /// HOST>WORKSPACE>TABS tree for the move (same id as the source's when
    /// one was carried). `None` for plain (pre-6B) receives.
    #[serde(default)]
    pub host_workspace: Option<neoism_protocol::workspace::WorkspaceSummary>,
}

#[derive(Debug, Deserialize)]
pub(crate) struct DockerSandboxRequest {
    workspace_id: String,
}

/// Blocking core of the receive flow: clone/reuse the repo under `root`,
/// then apply the uncommitted snapshot onto the provisioned path. This is
/// the half that must run on a blocking thread (it shells out to `git`),
/// so it's factored out of the async handler and made `pub` for direct
/// integration testing without the auth gate / HTTP machinery.
///
/// Wave-5 5E: BEFORE cloning, check whether any `candidate_local_repos`
/// path is already a local clone of `git_url` (matched by its `origin`
/// remote). If so, REUSE that path — apply the snapshot onto it in place
/// — instead of cloning a fresh copy into `root`. This lets a hand-made
/// clone at a custom path get adopted on receive. When no candidate
/// matches we fall back to the existing clone-into-managed-dir behavior.
pub fn receive_workspace_blocking(
    req: ReceiveWorkspaceRequest,
    root: &std::path::Path,
    candidate_local_repos: &[StdPathBuf],
) -> Result<(workspace_provision::ProvisionedPath, ApplyReport), ProvisionError> {
    let provisioned = if let Some(existing) =
        workspace_promote::find_matching_local_repo(&req.git_url, candidate_local_repos)
    {
        // Adopt the existing local clone. We deliberately do NOT pull /
        // fetch here: the caller pointed us at a repo they manage, and the
        // snapshot we're about to replay was captured against its current
        // history. `provision_from_git`'s managed-dir refresh policy does
        // not apply to a custom-path clone.
        tracing::info!(
            git_url = %req.git_url,
            path = %existing.display(),
            "receive: reusing existing local clone (origin matched) instead of cloning"
        );
        if let Some(git_ref) = req.git_ref.as_deref() {
            // Best-effort: try to land on the same ref the source was on.
            // A failure (dirty tree, unknown ref) is non-fatal — the
            // snapshot apply below carries the working state regardless.
            let _ = crate::hidden_std_command("git")
                .arg("-C")
                .arg(&existing)
                .args(["checkout", git_ref])
                .output();
        }
        workspace_provision::ProvisionedPath {
            git_url: req.git_url.clone(),
            git_ref: req.git_ref.clone(),
            path: existing,
            cloned: false,
            reused: true,
            updated: false,
        }
    } else {
        workspace_provision::provision_from_git(
            GitWorkspaceRequest {
                git_url: req.git_url,
                git_ref: req.git_ref,
                pull: req.pull,
            },
            root,
        )?
    };
    // Replay the source's uncommitted working state on top of the fresh
    // checkout. `apply_snapshot` is best-effort and never fails — partial
    // hunks land and rejects are recorded in the report — so we surface
    // the report rather than turning a partial apply into an error.
    let apply_report =
        workspace_snapshot::apply_snapshot(&provisioned.path, &req.snapshot);
    Ok((provisioned, apply_report))
}

/// `POST /workspace/receive`
///
/// Target-side of promote-to-this-host. Auth is the same cloud gate as
/// `workspace_from_git` ([`cloud_auth::authorize_provision`]). On success
/// the repo is cloned/reused, the uncommitted snapshot is replayed, and
/// the workspace is registered exactly like `workspace_from_git` (via an
/// `OpenProjectRoot` keyed on the on-disk path).
pub(crate) async fn workspace_receive(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(req): Json<ReceiveWorkspaceRequest>,
) -> Response {
    let principal = match cloud_auth::authorize_provision(&headers, &state.auth) {
        Ok(principal) => principal,
        Err(err) => return (err.status, err.message).into_response(),
    };
    let root = workspace_provision::workspaces_dir();
    // Wave 6B: pull the adopt metadata out before `req` moves into the
    // blocking provision task — the post-provision steps below need it.
    let carried_workspace_id = req.workspace_id.clone();
    let carried_title = req.title.clone();
    let carried_sessions = req.sessions.clone();
    let carried_preferences = req.preferences.clone();
    let carried_source_host = req.source_host.clone();
    // Wave-5 5E: snapshot the daemon's known local repo paths so the
    // blocking core can REUSE a hand-made clone whose `origin` matches the
    // incoming `git_url` instead of always cloning into the managed dir.
    let candidate_local_repos = state.workspaces.candidate_local_repos();
    let (provisioned, apply_report) = match tokio::task::spawn_blocking(move || {
        receive_workspace_blocking(req, &root, &candidate_local_repos)
    })
    .await
    {
        Ok(Ok(result)) => result,
        Ok(Err(err)) => return provision_error_response(err),
        Err(err) => {
            tracing::error!(error = %err, "workspace receive task failed");
            return (StatusCode::INTERNAL_SERVER_ERROR, "receive task failed")
                .into_response();
        }
    };

    let mut conn = ConnectionWorkspace::default();
    let outcome = workspace_handler::handle(
        &state.workspaces,
        &mut conn,
        None,
        None,
        WorkspaceClientMessage::OpenProjectRoot {
            path: provisioned.path.clone(),
            init_if_missing: false,
        },
    );
    let workspace = outcome.replies.iter().find_map(|reply| match reply {
        WorkspaceServerMessage::ProjectRootOpened { project_root } => {
            Some(project_root.clone())
        }
        _ => None,
    });
    let Some(workspace) = workspace else {
        let message = outcome
            .replies
            .iter()
            .find_map(|reply| match reply {
                WorkspaceServerMessage::Error { message } => Some(message.clone()),
                _ => None,
            })
            .unwrap_or_else(|| "workspace received but could not be registered".into());
        return (StatusCode::INTERNAL_SERVER_ERROR, message).into_response();
    };

    // ----- Wave 6B adopt steps: tabs, preferences, tree registration. -----
    // The carried host-workspace id (when present) keys everything so the
    // workspace keeps its identity across the move; a plain receive without
    // one anchors to the freshly-registered project root instead, which makes
    // the host-workspace id and the project-root id coincide.
    let adopted_workspace_id = carried_workspace_id
        .clone()
        .map(|id| id.trim().to_string())
        .filter(|id| !id.is_empty())
        .unwrap_or_else(|| workspace.id.clone());
    // Re-create the tabs with cwds remapped under the new checkout. Ids are
    // preserved (UUIDs) so a round-trip move doesn't mint duplicates; they
    // surface in the HOST>WORKSPACE>TABS tree as synthetic session tabs of
    // the adopted workspace.
    let adopted_sessions: Vec<neoism_protocol::workspace::SessionSummary> =
        carried_sessions
            .iter()
            .map(|s| neoism_protocol::workspace::SessionSummary {
                id: s.id.clone(),
                workspace_id: adopted_workspace_id.clone(),
                cwd: workspace_promote::remap_session_cwd(&provisioned.path, &s.cwd),
                label: s.label.clone(),
                last_active: s.last_active,
            })
            .collect();
    if !adopted_sessions.is_empty() {
        state
            .workspaces
            .register_adopted_sessions(adopted_sessions.clone());
    }
    if let Some(prefs) = carried_preferences.clone() {
        state
            .workspaces
            .set_preferences(adopted_workspace_id.clone(), prefs);
    }
    // Register the moved workspace in OUR host tree (under this daemon's own
    // host id) so this daemon's clients see it appear. Goes through the
    // canonical dispatcher like every other tree mutation; the replies land
    // on this synthetic HTTP "connection" only — per the daemon rule, nothing
    // is echoed back to a publisher (see tests/publish_no_echo_loop.rs).
    // Only registered when the sender carried workspace identity — a plain
    // pre-6B receive stays a project-root-only operation.
    let host_workspace = if carried_workspace_id.is_some() || !carried_sessions.is_empty()
    {
        let title = carried_title
            .as_deref()
            .map(str::trim)
            .filter(|t| !t.is_empty())
            .map(str::to_string)
            .unwrap_or_else(|| workspace.name.clone());
        let local_host = workspace_handler::local_host_id();
        let tree = workspace_handler::handle(
            &state.workspaces,
            &mut conn,
            None,
            None,
            WorkspaceClientMessage::CreateHostWorkspace {
                host_id: local_host,
                workspace_id: Some(adopted_workspace_id.clone()),
                title: Some(title),
                root_dir: Some(provisioned.path.clone()),
            },
        );
        tree.replies.iter().find_map(|reply| match reply {
            WorkspaceServerMessage::HostWorkspaceList { workspaces } => workspaces
                .iter()
                .find(|w| w.id == adopted_workspace_id)
                .cloned(),
            _ => None,
        })
    } else {
        None
    };

    tracing::info!(
        subject = %principal.subject,
        method = principal.method,
        path = %provisioned.path.display(),
        cloned = provisioned.cloned,
        reused = provisioned.reused,
        updated = provisioned.updated,
        applied = apply_report.applied_files.len(),
        rejected = apply_report.failed_hunks.len(),
        wrote_untracked = apply_report.wrote_untracked.len(),
        adopted_sessions = adopted_sessions.len(),
        adopted_workspace = host_workspace.is_some(),
        source_host = carried_source_host.as_deref().unwrap_or("<unknown>"),
        "received git workspace + applied snapshot"
    );

    (
        StatusCode::OK,
        Json(ReceiveWorkspaceResponse {
            workspace,
            apply_report,
            git_url: provisioned.git_url,
            git_ref: provisioned.git_ref,
            cloned: provisioned.cloned,
            reused: provisioned.reused,
            updated: provisioned.updated,
            path: provisioned.path,
            sessions: adopted_sessions,
            host_workspace,
        }),
    )
        .into_response()
}

pub(crate) async fn workspace_docker_sandbox(
    State(state): State<AppState>,
    Json(req): Json<DockerSandboxRequest>,
) -> Response {
    let workspace_id = req.workspace_id;
    let snapshot_path =
        workspace_handler::export_workspace_snapshot(&state.workspaces, &workspace_id)
            .map_err(|e| (StatusCode::BAD_REQUEST, e).into_response());
    let snapshot_path = match snapshot_path {
        Ok(path) => path,
        Err(resp) => return resp,
    };
    let sandbox = tokio::task::spawn_blocking({
        let workspace_id = workspace_id.clone();
        move || workspace_handler::start_local_docker_sandbox(&workspace_id)
    })
    .await
    .map_err(|e| {
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            format!("docker task failed: {e}"),
        )
            .into_response()
    })
    .and_then(|result| result.map_err(|e| (StatusCode::BAD_GATEWAY, e).into_response()));
    let sandbox = match sandbox {
        Ok(sandbox) => sandbox,
        Err(resp) => return resp,
    };
    tracing::info!(workspace_id = %workspace_id, snapshot = %snapshot_path.display(), target = %sandbox.url, "promoting workspace to local docker sandbox");
    state.workspaces.set_host_workspace_host_kind(
        &workspace_id,
        neoism_protocol::workspace::WorkspaceHostKind::DockerSandbox,
    );
    let response = workspace_promote_route(
        State(state),
        HeaderMap::new(),
        Json(PromoteWorkspaceRequest {
            workspace_id,
            target: sandbox.url.clone(),
            target_token: None,
            git_remote: None,
        }),
    )
    .await;
    if !response.status().is_success() {
        workspace_handler::cleanup_local_docker_sandbox(&sandbox);
    }
    response
}

/// `POST /workspace/promote`
///
/// **The keystone of the move plane.** Source-side of "relocate a workspace's
/// home to another host". Server-driven: this daemon orchestrates the move by
/// composing primitives that already exist — it invents no new sync.
///
/// Wave 6B unifies the 5D move plane with real cross-machine transfer
/// mechanics. Flow:
///
///   1. Auth: [`authorize_move`] — anonymous only when NO local auth is
///      configured (dev); otherwise the same cloud gate as
///      `/workspace/receive`.
///   2. Resolve the workspace: first against the HOST>WORKSPACE>TABS tree
///      (via the canonical `ListHostWorkspaces` dispatch — the desktop's
///      drag-a-workspace gesture promotes these), then against the
///      project-root registry (plain HTTP callers).
///   3. Resolve `target` → (base URL, bearer): paired-host name (token from
///      the `/hosts/pair` store), explicit `http(s)://` URL, or tailnet peer
///      hostname on the default daemon port.
///   4. Blocking git: derive the git URL from `origin` (or the caller's
///      `git_remote` override — **v1 requires a remote**, `409` without one),
///      resolve the current branch (`409` on a detached HEAD), **`git push`
///      the branch** so committed-but-unpushed work travels, and
///      [`workspace_snapshot::capture_uncommitted`] the working state
///      (tracked diff + untracked files).
///   5. `POST {target}/workspace/receive` with the repo coordinates, the
///      snapshot, the workspace's title/preferences and its sessions
///      ("tabs", cwds flattened workspace-relative). The target clones (or
///      reuses a matching local clone, 5E), replays the snapshot, registers
///      the workspace in its own tree and re-creates the tabs.
///   6. On a 2xx from the target: flip `running_on_host_id` to the target
///      via the `MoveWorkspaceToHost` dispatch so subscribers observe
///      `WorkspaceControlChanged` (clients re-dial — Wave 4B/4D), and drop
///      the legacy-plane state that travelled (project root, sessions,
///      surfaces, layout, prefs) — a move, not a copy.
///   7. Best-effort agent-session handoff (Wave 4C-agent), never fatal.
///
/// The pointer is only flipped *after* the target confirms receipt, so a
/// failed ship leaves the source authoritative. What does NOT travel: live
/// PTY processes. The git remote must be reachable from the target machine
/// for a real two-machine move.
pub(crate) async fn workspace_promote_route(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(req): Json<PromoteWorkspaceRequest>,
) -> Response {
    let principal = match authorize_move(&headers, &state) {
        Ok(principal) => principal,
        Err(err) => return (err.status, err.message).into_response(),
    };
    let subject = principal
        .as_ref()
        .map(|p| p.subject.clone())
        .unwrap_or_else(|| "local-anonymous".to_string());

    // --- 1. Resolve the workspace. Host-workspace tree first (the desktop
    // drag path), through the public `ListHostWorkspaces` dispatch so we
    // never reach into private manager internals; then the project-root
    // registry as a fallback for plain HTTP callers.
    let mut conn = ConnectionWorkspace::default();
    let list = workspace_handler::handle(
        &state.workspaces,
        &mut conn,
        None,
        None,
        WorkspaceClientMessage::ListHostWorkspaces { host_id: None },
    );
    let host_workspace = list.replies.iter().find_map(|reply| match reply {
        WorkspaceServerMessage::HostWorkspaceList { workspaces } => workspaces
            .iter()
            .find(|w| w.id == req.workspace_id)
            .cloned(),
        _ => None,
    });
    let project_root_direct = if host_workspace.is_none() {
        state.workspaces.project_root_summary(&req.workspace_id)
    } else {
        None
    };
    let (root_dir, title) = if let Some(ws) = &host_workspace {
        let Some(root_dir) = ws.root_dir.clone() else {
            return (
                StatusCode::BAD_REQUEST,
                format!(
                    "workspace {} has no on-disk root_dir to promote",
                    req.workspace_id
                ),
            )
                .into_response();
        };
        (root_dir, ws.title.clone())
    } else if let Some(pr) = &project_root_direct {
        (pr.path.clone(), pr.name.clone())
    } else {
        return (
            StatusCode::NOT_FOUND,
            format!("no such workspace: {}", req.workspace_id),
        )
            .into_response();
    };

    // --- 2. Resolve the target (Wave 6B): paired-host name → explicit URL →
    // tailnet peer hostname. A paired host contributes its stored device
    // bearer unless the request carried an explicit one.
    let (target_base_url, target_bearer) = match resolve_promote_target(
        &state.paired_hosts,
        &req.target,
        req.target_token.clone(),
    )
    .await
    {
        Ok(resolved) => resolved,
        Err(message) => return (StatusCode::BAD_REQUEST, message).into_response(),
    };

    // --- 3. Blocking git: validate the root, derive the git URL, resolve the
    // branch, PUSH it (so committed-but-unpushed work travels — the exact
    // "metadata-only move" gap Wave 6B closes), and capture the uncommitted
    // working state. All of this shells out to `git`, so it runs off the
    // async runtime.
    let workspace_id = req.workspace_id.clone();
    let git_remote = req.git_remote.clone();
    // Keep the source checkout path for session flattening + the best-effort
    // agent-session export (`root_dir` itself moves into the closure below).
    let source_root = root_dir.clone();
    let prep = tokio::task::spawn_blocking(move || {
        if !root_dir.exists() {
            return Err(PromoteError::RootMissing(root_dir));
        }
        let git_url =
            workspace_promote::derive_git_url(&root_dir, git_remote.as_deref())?;
        let Some(branch) = workspace_promote::current_branch(&root_dir) else {
            return Err(PromoteError::DetachedHead);
        };
        workspace_promote::push_branch(&root_dir, &git_url, &branch)?;
        let snapshot = workspace_snapshot::capture_uncommitted(&root_dir)?;
        Ok::<_, PromoteError>((git_url, branch, snapshot))
    })
    .await;
    let (git_url, branch, snapshot) = match prep {
        Ok(Ok(prepared)) => prepared,
        Ok(Err(err)) => return promote_error_response(err),
        Err(err) => {
            tracing::error!(error = %err, "workspace promote prep task failed");
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                "promote prep task failed",
            )
                .into_response();
        }
    };
    let uncommitted_diff_carried = !snapshot.is_empty();

    // --- 4. Gather the sessions ("tabs") that travel with the move. The two
    // registries key sessions differently — by host-workspace id (tree tabs)
    // or by project-root id (websocket sessions) — so export the union over
    // both ids, flattened to workspace-relative cwds.
    let project_root = project_root_direct
        .clone()
        .or_else(|| state.workspaces.project_root_for_path(&source_root));
    let mut export_ids: Vec<String> = vec![req.workspace_id.clone()];
    if let Some(pr) = &project_root {
        if pr.id != req.workspace_id {
            export_ids.push(pr.id.clone());
        }
    }
    let mut seen_session_ids = std::collections::HashSet::new();
    let mut portable_sessions: Vec<PortableSession> = Vec::new();
    for id in &export_ids {
        for session in state.workspaces.workspace_sessions(id) {
            if seen_session_ids.insert(session.id.clone()) {
                portable_sessions.push(PortableSession {
                    id: session.id.clone(),
                    cwd: workspace_promote::relative_session_cwd(
                        &source_root,
                        &session.cwd,
                    ),
                    label: session.label.clone(),
                    last_active: session.last_active,
                });
            }
        }
    }
    // Preferences travel verbatim; try the promoted id first, then the
    // project-root id (chromes key by either).
    let mut preferences = state.workspaces.get_preferences(&req.workspace_id);
    if preferences == neoism_protocol::workspace::WorkplacePreferences::default() {
        if let Some(pr) = &project_root {
            preferences = state.workspaces.get_preferences(&pr.id);
        }
    }
    let preferences = (preferences
        != neoism_protocol::workspace::WorkplacePreferences::default())
    .then_some(preferences);

    // --- 5. Ship to the target's `/workspace/receive`. The target clones the
    // history from `git_url`, replays the snapshot, registers the workspace
    // in its tree and re-creates the tabs, returning its ApplyReport. A
    // non-2xx (or transport failure) aborts before we flip the pointer or
    // remove anything, so the source stays authoritative — promote is
    // all-or-nothing on the source side.
    let endpoint = workspace_promote::receive_endpoint(&target_base_url);
    let sessions_shipped = portable_sessions.len();
    let payload = ReceivePayload {
        git_url: git_url.clone(),
        git_ref: Some(branch.clone()),
        snapshot,
        workspace_id: Some(workspace_id.clone()),
        title: Some(title.clone()),
        sessions: portable_sessions,
        preferences,
        source_host: Some(local_hostname()),
    };
    let client = reqwest::Client::new();
    let mut request = client.post(&endpoint).json(&payload);
    if let Some(token) = target_bearer.as_deref() {
        request = request.bearer_auth(token);
    }
    let target_response = match request.send().await {
        Ok(resp) => resp,
        Err(err) => {
            tracing::error!(error = %err, endpoint = %endpoint, "promote: target unreachable");
            return (
                StatusCode::BAD_GATEWAY,
                format!("target /workspace/receive unreachable: {err}"),
            )
                .into_response();
        }
    };
    let target_status = target_response.status();
    let body_bytes = target_response.bytes().await.unwrap_or_default();
    if !target_status.is_success() {
        let detail = String::from_utf8_lossy(&body_bytes);
        tracing::error!(
            status = target_status.as_u16(),
            endpoint = %endpoint,
            "promote: target rejected receive"
        );
        return (
            StatusCode::BAD_GATEWAY,
            format!(
                "target /workspace/receive returned {}: {}",
                target_status.as_u16(),
                detail.trim()
            ),
        )
            .into_response();
    }
    let received: ReceiveWorkspaceResponse = match serde_json::from_slice(&body_bytes) {
        Ok(parsed) => parsed,
        Err(err) => {
            tracing::error!(error = %err, "promote: could not parse target receive response");
            return (
                StatusCode::BAD_GATEWAY,
                "target /workspace/receive returned an unparseable body",
            )
                .into_response();
        }
    };

    // --- 6a. Flip `running_on_host_id` to the target. We dispatch the
    // canonical `MoveWorkspaceToHost` so the same `WorkspaceControlChanged`
    // reply websocket clients get is produced here, and the shared registry
    // every socket reads from is mutated. The target host id is the resolved
    // target base URL — a stable, re-dialable identifier (clients resolve
    // `running_on_host_id` → daemon_url to reconnect after a move). Only
    // applies when the promoted id names a host-workspace; a bare
    // project-root promote has no tree pointer to flip.
    let flipped = if host_workspace.is_some() {
        let flip = workspace_handler::handle(
            &state.workspaces,
            &mut conn,
            None,
            None,
            WorkspaceClientMessage::MoveWorkspaceToHost {
                workspace_id: workspace_id.clone(),
                target_host_id: target_base_url.clone(),
            },
        );
        let flipped = flip.replies.iter().find_map(|reply| match reply {
            WorkspaceServerMessage::WorkspaceControlChanged { workspace } => {
                Some(workspace.clone())
            }
            _ => None,
        });
        if flipped.is_none() {
            // The target already has the workspace; failing to flip locally
            // is a server bug rather than the caller's fault. Surface it.
            let message = flip
                .replies
                .iter()
                .find_map(|reply| match reply {
                    WorkspaceServerMessage::Error { message } => Some(message.clone()),
                    _ => None,
                })
                .unwrap_or_else(|| {
                    "workspace shipped to target but pointer flip produced no \
                     WorkspaceControlChanged"
                        .into()
                });
            return (StatusCode::INTERNAL_SERVER_ERROR, message).into_response();
        }
        flipped
    } else {
        None
    };

    // --- 6b. Drop the legacy-plane state that travelled (project root,
    // sessions, editor surfaces, pane layout, preferences) so the move is a
    // move, not a copy. The host-workspace summary itself stays — its flipped
    // pointer is how the tree tells every client where the workspace now
    // lives. The source checkout on disk is never touched.
    let cleanup_ids: Vec<&str> = export_ids.iter().map(String::as_str).collect();
    let removed_locally = state.workspaces.complete_workspace_move(&cleanup_ids);

    tracing::info!(
        subject = %subject,
        workspace_id = %workspace_id,
        target = %req.target,
        target_base_url = %target_base_url,
        git_url = %git_url,
        branch = %branch,
        sessions_moved = received.sessions.len(),
        removed_locally,
        applied = received.apply_report.applied_files.len(),
        rejected = received.apply_report.failed_hunks.len(),
        wrote_untracked = received.apply_report.wrote_untracked.len(),
        "promoted workspace to target host"
    );

    // --- 7. Best-effort: ship the workspace's AI-agent session(s) to the
    // target so the agent resumes on its new home ("shut the laptop, agent
    // keeps working"). Runs only *after* the files landed and the pointer
    // flipped — and NEVER fails the promote: every error is folded into the
    // returned `agent_ship` summary instead. The target's checkout path
    // (`received.path`) is where its `/sessions/import` rebases the bundles.
    let agent_ship = ship_agent_sessions(
        &source_root,
        &target_base_url,
        target_bearer.as_deref(),
        &received.path,
    )
    .await;
    if !agent_ship.errors.is_empty() {
        tracing::warn!(
            workspace_id = %workspace_id,
            target_base_url = %target_base_url,
            exported = agent_ship.exported,
            imported = agent_ship.imported,
            errors = ?agent_ship.errors,
            "promote: agent-session handoff was partial/failed (promote still succeeded)"
        );
    } else {
        tracing::info!(
            workspace_id = %workspace_id,
            exported = agent_ship.exported,
            imported = agent_ship.imported,
            "promote: shipped agent sessions to target host"
        );
    }

    let sessions_moved = if received.sessions.is_empty() {
        // Older target that ignored the session payload: report what we
        // shipped rather than what it confirmed.
        sessions_shipped
    } else {
        received.sessions.len()
    };
    (
        StatusCode::OK,
        Json(PromoteWorkspaceResponse {
            workspace: flipped,
            target_apply_report: received.apply_report,
            git_url,
            agent_ship,
            workspace_id,
            target: req.target,
            target_base_url,
            git_ref: Some(branch),
            remote_path: received.path,
            sessions_moved,
            uncommitted_diff_carried,
        }),
    )
        .into_response()
}

/// Best-effort hop 1+2 of the agent-session handoff a promote performs after the
/// workspace files land. **Never returns an error** — every failure becomes an
/// `errors` line in the returned [`AgentShipSummary`] so the caller sees what
/// happened without the promote failing.
///
/// 1. `POST {NEOISM_AGENT_SERVER}/sessions/export { workspaceRoot }` against
///    THIS host's local agent-server → the session bundles for `source_root`.
/// 2. `POST {target_url}/workspace/receive-agent { bundles, target_workspace_root }`
///    → the target daemon proxies each bundle into its own loopback
///    agent-server. We fold the target's `{ imported, errors }` into the summary.
///
/// A source with no agent-server / no sessions under the root yields the empty
/// summary (`exported: 0, imported: 0, errors: []`) — a clean no-op.
pub(crate) async fn ship_agent_sessions(
    source_root: &std::path::Path,
    target_url: &str,
    target_token: Option<&str>,
    target_workspace_root: &std::path::Path,
) -> AgentShipSummary {
    let mut summary = AgentShipSummary::default();

    // --- hop 1: export from this host's local agent-server.
    let agent_server = workspace_promote::agent_server_url();
    let export_endpoint = workspace_promote::agent_export_endpoint(&agent_server);
    let client = reqwest::Client::new();
    let export_body = ExportSessionsRequest {
        workspace_root: source_root.to_string_lossy().to_string(),
    };
    let export_resp = match client
        .post(&export_endpoint)
        .json(&export_body)
        .send()
        .await
    {
        Ok(resp) => resp,
        Err(err) => {
            // No reachable agent-server (e.g. none running) — log + continue.
            summary.errors.push(format!(
                "agent export unreachable ({export_endpoint}): {err}"
            ));
            return summary;
        }
    };
    let export_status = export_resp.status();
    let export_bytes = export_resp.bytes().await.unwrap_or_default();
    if !export_status.is_success() {
        summary.errors.push(format!(
            "agent export returned {}: {}",
            export_status.as_u16(),
            String::from_utf8_lossy(&export_bytes).trim()
        ));
        return summary;
    }
    let exported: ExportSessionsResponse = match serde_json::from_slice(&export_bytes) {
        Ok(parsed) => parsed,
        Err(err) => {
            summary
                .errors
                .push(format!("agent export returned an unparseable body: {err}"));
            return summary;
        }
    };
    summary.exported = exported.bundles.len();
    if exported.bundles.is_empty() {
        // Nothing to ship — a clean no-op, not an error.
        return summary;
    }

    // --- hop 2: ship the bundles to the target daemon, which proxies them into
    // its own loopback agent-server (we can't reach the target's loopback).
    let receive_endpoint = workspace_promote::receive_agent_endpoint(target_url);
    let ship_body = ReceiveAgentRequest {
        bundles: exported.bundles,
        target_workspace_root: target_workspace_root.to_string_lossy().to_string(),
    };
    let mut request = client.post(&receive_endpoint).json(&ship_body);
    if let Some(token) = target_token {
        request = request.bearer_auth(token);
    }
    let ship_resp = match request.send().await {
        Ok(resp) => resp,
        Err(err) => {
            summary.errors.push(format!(
                "target /workspace/receive-agent unreachable ({receive_endpoint}): {err}"
            ));
            return summary;
        }
    };
    let ship_status = ship_resp.status();
    let ship_bytes = ship_resp.bytes().await.unwrap_or_default();
    if !ship_status.is_success() {
        summary.errors.push(format!(
            "target /workspace/receive-agent returned {}: {}",
            ship_status.as_u16(),
            String::from_utf8_lossy(&ship_bytes).trim()
        ));
        return summary;
    }
    match serde_json::from_slice::<ReceiveAgentResponse>(&ship_bytes) {
        Ok(parsed) => {
            summary.imported = parsed.imported;
            summary.errors.extend(parsed.errors);
        }
        Err(err) => {
            summary.errors.push(format!(
                "target /workspace/receive-agent returned an unparseable body: {err}"
            ));
        }
    }
    summary
}

/// `POST /workspace/receive-agent`
///
/// TARGET side of the promote agent-session handoff. The SOURCE daemon can't
/// reach the target's *loopback* agent-server, so it ships the exported bundles
/// here and this handler proxies each into THIS host's local agent-server via
/// `POST {NEOISM_AGENT_SERVER}/sessions/import { bundle, targetWorkspaceRoot }`.
///
/// Auth is the same cloud gate as the other `/workspace/*` routes
/// ([`cloud_auth::authorize_provision`]). Per-bundle import is best-effort: a
/// failed bundle is recorded in `errors` but the others still import and the
/// route returns `200` with an `{ imported, errors }` summary, so one bad
/// session never sinks the whole agent move.
pub(crate) async fn workspace_receive_agent(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(req): Json<ReceiveAgentRequest>,
) -> Response {
    let principal = match cloud_auth::authorize_provision(&headers, &state.auth) {
        Ok(principal) => principal,
        Err(err) => return (err.status, err.message).into_response(),
    };

    let agent_server = workspace_promote::agent_server_url();
    let import_endpoint = workspace_promote::agent_import_endpoint(&agent_server);
    let client = reqwest::Client::new();

    let mut imported = 0usize;
    let mut errors: Vec<String> = Vec::new();
    for (index, bundle) in req.bundles.into_iter().enumerate() {
        let body = ImportSessionRequest {
            bundle,
            target_workspace_root: req.target_workspace_root.clone(),
        };
        let resp = match client.post(&import_endpoint).json(&body).send().await {
            Ok(resp) => resp,
            Err(err) => {
                errors.push(format!(
                    "bundle {index}: agent import unreachable ({import_endpoint}): {err}"
                ));
                continue;
            }
        };
        let status = resp.status();
        if status.is_success() {
            imported += 1;
        } else {
            let detail = resp.bytes().await.unwrap_or_default();
            errors.push(format!(
                "bundle {index}: agent import returned {}: {}",
                status.as_u16(),
                String::from_utf8_lossy(&detail).trim()
            ));
        }
    }

    tracing::info!(
        subject = %principal.subject,
        method = principal.method,
        target_workspace_root = %req.target_workspace_root,
        imported,
        errors = errors.len(),
        "received agent session bundles + forwarded to local agent-server"
    );

    (
        StatusCode::OK,
        Json(ReceiveAgentResponse { imported, errors }),
    )
        .into_response()
}

/// Map a [`PromoteError`] to an HTTP response. A missing git remote is a
/// `400` (the caller's repo isn't promotable in v1); a missing workspace is a
/// `404`; everything else is a `500`.
pub(crate) fn promote_error_response(err: PromoteError) -> Response {
    let status = match &err {
        PromoteError::NoSuchWorkspace(_) => StatusCode::NOT_FOUND,
        PromoteError::NoRootDir(_) | PromoteError::RootMissing(_) => {
            StatusCode::BAD_REQUEST
        }
        // Repo-state problems the operator can fix (add a remote / check out
        // a branch) — `409 Conflict`, matching the two-daemon promote suite.
        PromoteError::NoGitRemote(_) | PromoteError::DetachedHead => StatusCode::CONFLICT,
        PromoteError::Git(_) | PromoteError::Snapshot(_) => {
            StatusCode::INTERNAL_SERVER_ERROR
        }
    };
    (status, err.to_string()).into_response()
}

/// Gate for the operator-facing move routes (`/workspace/promote`,
/// `/workspace/demote`).
///
/// Unlike `/workspace/receive` (remote-facing, ALWAYS gated), the move routes
/// accept an anonymous local caller when no local auth is configured at all —
/// no `NEOISM_REQUIRE_AUTH=1`, no cloud-provision token, no legacy daemon
/// token, and no bearer offered. The moment any of those is present the full
/// [`cloud_auth::authorize_provision`] gate applies, so a hardened daemon
/// never lets an anonymous caller evict workspaces. (The embedded desktop
/// daemon always sets `NEOISM_DAEMON_TOKEN` and sends it as the bearer.)
pub(crate) fn authorize_move(
    headers: &HeaderMap,
    state: &AppState,
) -> Result<Option<cloud_auth::CloudPrincipal>, cloud_auth::CloudAuthError> {
    let locally_configured = handshake::require_auth_enabled()
        || cloud_auth::provision_token_configured()
        || cloud_auth::legacy_daemon_token_configured();
    if !locally_configured && cloud_auth::extract_bearer(headers).is_none() {
        return Ok(None);
    }
    cloud_auth::authorize_provision(headers, &state.auth).map(Some)
}
