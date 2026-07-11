use super::*;

// ---------------------------------------------------------------------
// Wave 6B: automated host pairing + promote target resolution.
// ---------------------------------------------------------------------

/// `POST /hosts/pair`
///
/// Pair this daemon with a remote one. The operator mints a code on the
/// remote (`POST /pair` there), then calls this route here with the remote's
/// base URL + that code. We claim the code at `<base_url>/pair/claim`,
/// persist the granted device token in the paired-host store, and from then
/// on `POST /workspace/promote` can address the remote by `name` — no
/// NEOISM_HOST_URL / token env plumbing.
///
/// Auth: operator-local, same trust model as `POST /pair` (the daemon binds
/// to localhost by default and this route spends — not mints — a pairing
/// code the operator just created on the other machine).
#[derive(Debug, Deserialize)]
pub struct HostPairRequest {
    /// Friendly handle for the remote; defaults to the URL's hostname.
    #[serde(default)]
    pub name: Option<String>,
    /// `http(s)://host:port` of the remote daemon.
    pub base_url: String,
    /// Short-lived pairing code minted on the remote.
    pub code: String,
}

pub(crate) async fn hosts_pair(
    State(state): State<AppState>,
    Json(req): Json<HostPairRequest>,
) -> Response {
    let base_url = hosts::normalize_base_url(&req.base_url);
    if !base_url.starts_with("http://") && !base_url.starts_with("https://") {
        return (StatusCode::BAD_REQUEST, "base_url must be an http(s) URL")
            .into_response();
    }
    let name = req
        .name
        .as_deref()
        .map(str::trim)
        .filter(|n| !n.is_empty())
        .map(str::to_string)
        .unwrap_or_else(|| hosts::name_from_base_url(&base_url));

    // Ask for everything a promoted workspace needs on the remote: GitWrite
    // gates `/workspace/receive` (same provision gate as
    // `/workspace/from-git`); the rest let a follow-up client session drive
    // the moved tabs. The remote's operator-approval gate is still the final
    // arbiter of what's actually granted.
    let claim = PairClaimRequest {
        code: req.code.clone(),
        device_label: format!("neoism-daemon@{}", local_hostname()),
        requested_permissions: std::collections::BTreeSet::from([
            Permission::ReadFiles,
            Permission::WriteFiles,
            Permission::GitWrite,
            Permission::PtyCreate,
        ]),
    };
    let claim_url = format!("{base_url}/pair/claim");
    let response = match reqwest::Client::new()
        .post(&claim_url)
        .json(&claim)
        .send()
        .await
    {
        Ok(resp) => resp,
        Err(err) => {
            tracing::warn!(error = %err, url = %claim_url, "host pairing: claim request failed");
            return (
                StatusCode::BAD_GATEWAY,
                format!("could not reach target daemon at {base_url}: {err}"),
            )
                .into_response();
        }
    };
    let status = response.status();
    let claim_response: PairClaimResponse = match response.json().await {
        Ok(parsed) => parsed,
        Err(err) => {
            return (
                StatusCode::BAD_GATEWAY,
                format!(
                    "target daemon returned an unexpected claim reply ({status}): {err}"
                ),
            )
                .into_response();
        }
    };

    match claim_response {
        PairClaimResponse::Granted {
            device_id,
            device_token,
            granted_permissions,
        } => {
            if !granted_permissions.contains(&Permission::GitWrite) {
                tracing::warn!(
                    %name,
                    "host pairing granted without GitWrite; /workspace/receive on the target will reject promotes"
                );
            }
            let host = PairedHost {
                name: name.clone(),
                base_url: base_url.clone(),
                device_id,
                token: device_token,
                paired_at: time::OffsetDateTime::now_utc().unix_timestamp(),
            };
            let summary = crate::hosts::PairedHostSummary::from(&host);
            if let Err(err) = state.paired_hosts.upsert(host) {
                tracing::error!(error = %err, "host pairing: could not persist paired host");
                return (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    "paired but could not persist host record",
                )
                    .into_response();
            }
            tracing::info!(%name, %base_url, "paired with remote daemon");
            (StatusCode::OK, Json(summary)).into_response()
        }
        PairClaimResponse::Pending => (
            StatusCode::ACCEPTED,
            "pairing is pending operator approval on the target; approve there and retry with a fresh code",
        )
            .into_response(),
        PairClaimResponse::Rejected { reason } => (
            StatusCode::FORBIDDEN,
            format!("target rejected pairing: {reason}"),
        )
            .into_response(),
    }
}

/// `GET /hosts` — redacted list of paired hosts (no tokens on the wire).
pub(crate) async fn hosts_list(State(state): State<AppState>) -> Response {
    (StatusCode::OK, Json(state.paired_hosts.list())).into_response()
}

/// Resolve a promote `target` to `(base_url, bearer)`:
///
/// 1. Paired host by name (or by already-paired URL) — uses the stored
///    token unless the request carried an explicit one.
/// 2. Explicit `http(s)://` URL — request token only.
/// 3. Tailnet peer hostname — `http://<peer-ip>:7878` (default daemon
///    port), request token only.
pub(crate) async fn resolve_promote_target(
    paired_hosts: &PairedHostStore,
    target: &str,
    explicit_token: Option<String>,
) -> Result<(String, Option<String>), String> {
    let target = target.trim();
    if target.is_empty() {
        return Err(
            "target is required (paired-host name, URL, or tailnet hostname)".to_string(),
        );
    }
    if let Some(host) = paired_hosts.resolve(target) {
        return Ok((host.base_url, explicit_token.or(Some(host.token))));
    }
    if target.starts_with("http://") || target.starts_with("https://") {
        return Ok((hosts::normalize_base_url(target), explicit_token));
    }
    // Last resort: tailnet discovery. Default daemon port — operators
    // running on a custom port should pair or pass an explicit URL.
    let target_owned = target.to_string();
    let peers = tokio::task::spawn_blocking(crate::tailnet::discover_peers_blocking)
        .await
        .unwrap_or_default();
    if let Some(peer) = peers
        .peers
        .iter()
        .find(|p| p.hostname.eq_ignore_ascii_case(&target_owned))
    {
        return Ok((format!("http://{}:7878", peer.ip), explicit_token));
    }
    Err(format!(
        "could not resolve promote target `{target}`: not a paired host \
         (POST /hosts/pair first), not an http(s) URL, and not a tailnet peer"
    ))
}

/// Best-effort local hostname for device labels / source attribution.
pub(crate) fn local_hostname() -> String {
    std::env::var("HOSTNAME")
        .ok()
        .map(|h| h.trim().to_string())
        .filter(|h| !h.is_empty())
        .or_else(|| {
            std::fs::read_to_string("/etc/hostname")
                .ok()
                .map(|h| h.trim().to_string())
                .filter(|h| !h.is_empty())
        })
        .unwrap_or_else(|| "neoism-host".to_string())
}

/// `POST /workspace/demote`
///
/// **The flip-back half of the move plane** — bring a workspace HOME to *this*
/// host. Demote is promote-in-reverse and invents no new sync: this daemon
/// asks the workspace's *current home* to `/workspace/promote` it BACK here.
/// We are the *target* of that remote promote, so this is a thin orchestrator:
///
///   1. Auth via the same cloud gate as `/workspace/promote`
///      ([`cloud_auth::authorize_provision`]).
///   2. Resolve the workspace from the registry (`ListHostWorkspaces`) and read
///      its current home host id (`running_on_host_id`, falling back to
///      `host_id`).
///   3. Resolve THIS host's own receive URL from `NEOISM_HOST_URL`. Without it
///      the remote home has nowhere to ship to → `400`.
///   4. Resolve the home host id → that host's `daemon_url` (the remote home
///      daemon) from the host registry (`ListHosts`). If the workspace is
///      already homed at this host (home URL == our URL, or the home host has no
///      distinct `daemon_url`) → `200` no-op.
///   5. `POST {remote_home}/workspace/promote` with
///      `{ workspace_id, target_url: <our url>, target_token }` and bearer auth.
///      The remote home captures + ships its working state to OUR
///      `/workspace/receive` and flips the pointer to us.
///   6. Pass the promote result (`{ workspace, target_apply_report, git_url }`)
///      back to the caller verbatim.
pub(crate) async fn workspace_demote_route(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(req): Json<DemoteWorkspaceRequest>,
) -> Response {
    // Same relaxed operator-local gate as `/workspace/promote` — anonymous
    // only when no local auth is configured at all.
    let principal = match authorize_move(&headers, &state) {
        Ok(principal) => principal,
        Err(err) => return (err.status, err.message).into_response(),
    };
    let subject = principal
        .as_ref()
        .map(|p| p.subject.clone())
        .unwrap_or_else(|| "local-anonymous".to_string());

    // --- 1. Resolve THIS host's own receive URL. The remote home needs a
    // dialable address to POST the workspace back to; without one demote is
    // impossible. (Same env the bootstrap host advertises as its `daemon_url`.)
    let this_host_url = std::env::var("NEOISM_HOST_URL")
        .ok()
        .map(|u| u.trim().to_string())
        .filter(|u| !u.is_empty());
    let Some(this_host_url) = this_host_url else {
        return (
            StatusCode::BAD_REQUEST,
            "demote requires NEOISM_HOST_URL (this host has no advertised \
             address to receive at)",
        )
            .into_response();
    };

    // --- 2. Resolve the workspace + its current home host id from the registry.
    // We go through the public `ListHostWorkspaces` dispatch (no private
    // manager access), mirroring `/workspace/promote`.
    let mut conn = ConnectionWorkspace::default();
    let list = workspace_handler::handle(
        &state.workspaces,
        &mut conn,
        None,
        None,
        WorkspaceClientMessage::ListHostWorkspaces { host_id: None },
    );
    let summary = list.replies.iter().find_map(|reply| match reply {
        WorkspaceServerMessage::HostWorkspaceList { workspaces } => workspaces
            .iter()
            .find(|w| w.id == req.workspace_id)
            .cloned(),
        _ => None,
    });
    let Some(summary) = summary else {
        return (
            StatusCode::NOT_FOUND,
            format!("no such workspace: {}", req.workspace_id),
        )
            .into_response();
    };
    // The current home is `running_on_host_id`; fall back to `host_id` (the
    // owning host) when the running pointer is unset.
    let home_host_id = summary
        .running_on_host_id
        .clone()
        .unwrap_or_else(|| summary.host_id.clone());

    // --- 3. Resolve the home host id → its dialable `daemon_url` from the host
    // registry (`ListHosts`). A `MoveWorkspaceToHost` pointer flip records the
    // target *URL* directly as the host id, so the home host id may already BE a
    // URL even when it isn't registered as a host with a `daemon_url`.
    let hosts = workspace_handler::handle(
        &state.workspaces,
        &mut conn,
        None,
        None,
        WorkspaceClientMessage::ListHosts,
    );
    let home_daemon_url = hosts.replies.iter().find_map(|reply| match reply {
        WorkspaceServerMessage::HostList { hosts } => hosts
            .iter()
            .find(|h| h.id == home_host_id)
            .and_then(|h| h.daemon_url.clone()),
        _ => None,
    });
    // Prefer the registered `daemon_url`; otherwise treat a URL-shaped home host
    // id as the dialable URL itself (the promote pointer-flip convention).
    let remote_home_url = home_daemon_url.unwrap_or_else(|| home_host_id.clone());

    // --- 4. No-op when the workspace is already homed here. Two shapes:
    //   * the resolved home URL is our own `NEOISM_HOST_URL`, or
    //   * the home host id has no distinct dialable URL and matches nothing
    //     remote (it equals our URL after normalisation).
    let remote_home_dialable =
        remote_home_url.starts_with("http://") || remote_home_url.starts_with("https://");
    if !remote_home_dialable
        || workspace_promote::same_host_url(&remote_home_url, &this_host_url)
    {
        tracing::info!(
            subject = %subject,
            workspace_id = %req.workspace_id,
            home_host_id = %home_host_id,
            this_host_url = %this_host_url,
            "demote no-op: workspace already homed at this host"
        );
        return (
            StatusCode::OK,
            Json(serde_json::json!({
                "noop": true,
                "message": format!(
                    "workspace {} is already homed at this host",
                    req.workspace_id
                ),
                "workspace": summary,
            })),
        )
            .into_response();
    }

    // --- 5. Ask the remote home to promote the workspace BACK to us. We are the
    // target of that promote: `target_url` is our own receive URL and
    // `target_token` is the bearer the remote home presents to our cloud gate.
    let endpoint = workspace_promote::promote_endpoint(&remote_home_url);
    let body = serde_json::json!({
        "workspace_id": req.workspace_id,
        "target_url": this_host_url,
        "target_token": req.target_token,
    });
    let client = reqwest::Client::new();
    let mut request = client.post(&endpoint).json(&body);
    if let Some(token) = req.target_token.as_deref() {
        request = request.bearer_auth(token);
    }
    let home_response = match request.send().await {
        Ok(resp) => resp,
        Err(err) => {
            tracing::error!(error = %err, endpoint = %endpoint, "demote: remote home unreachable");
            return (
                StatusCode::BAD_GATEWAY,
                format!("remote home /workspace/promote unreachable: {err}"),
            )
                .into_response();
        }
    };
    let home_status = home_response.status();
    let body_bytes = home_response.bytes().await.unwrap_or_default();
    if !home_status.is_success() {
        let detail = String::from_utf8_lossy(&body_bytes);
        tracing::error!(
            status = home_status.as_u16(),
            endpoint = %endpoint,
            "demote: remote home rejected promote"
        );
        return (
            StatusCode::BAD_GATEWAY,
            format!(
                "remote home /workspace/promote returned {}: {}",
                home_status.as_u16(),
                detail.trim()
            ),
        )
            .into_response();
    }

    // --- 6. Pass the promote result through verbatim. The remote home has
    // already shipped to our `/workspace/receive` and flipped its pointer to us;
    // the response's `workspace.running_on_host_id` now names our URL.
    let promote_result: PromoteWorkspaceResponse = match serde_json::from_slice(
        &body_bytes,
    ) {
        Ok(parsed) => parsed,
        Err(err) => {
            tracing::error!(error = %err, "demote: could not parse remote home promote response");
            return (
                StatusCode::BAD_GATEWAY,
                "remote home /workspace/promote returned an unparseable body",
            )
                .into_response();
        }
    };

    tracing::info!(
        subject = %subject,
        workspace_id = %req.workspace_id,
        remote_home_url = %remote_home_url,
        this_host_url = %this_host_url,
        git_url = %promote_result.git_url,
        applied = promote_result.target_apply_report.applied_files.len(),
        rejected = promote_result.target_apply_report.failed_hunks.len(),
        wrote_untracked = promote_result.target_apply_report.wrote_untracked.len(),
        "demoted workspace home to this host"
    );

    (StatusCode::OK, Json(promote_result)).into_response()
}
