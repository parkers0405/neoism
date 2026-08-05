//! Message dispatch: the `handle` / `handle_preauthenticated` entry
//! points, the ~500-line `handle_inner` match router, the `Hello`
//! handshake arm, and full-snapshot assembly. Pure code-move out of the
//! former monolithic `workspace.rs`.

use super::clipboard::materialize_clipboard_image;
use super::handlers::*;
use super::shell_ops::{export_workspace_snapshot, start_local_docker_sandbox};
use super::*;

/// Dispatch a single workspace client message. Returns the reply (or a
/// flurry of replies — `OpenProjectRoot` emits both `ProjectRootOpened`
/// and `ProjectRootChanged`, etc.) the websocket task should send back
/// to the client.
///
/// `pairing_tokens` and `peer_ip` are only consulted by the `Hello`
/// arm and may be `None` in tests or on call sites that pre-resolve
/// the handshake.
pub fn handle(
    manager: &WorkspaceManager,
    conn: &mut ConnectionWorkspace,
    pairing_tokens: Option<&PairingTokenStore>,
    peer_ip: Option<&str>,
    msg: WorkspaceClientMessage,
) -> DispatchOutcome {
    handle_inner(manager, conn, pairing_tokens, None, peer_ip, msg)
}

/// Dispatch a workspace message when the transport has already
/// authenticated the peer (for example `Authorization: Bearer` on the
/// websocket upgrade or the legacy `?token=` daemon token). This keeps
/// `Hello` token introspection compatible with existing pairing tokens
/// while letting cloud clients use their long-lived device token as the
/// gate.
pub fn handle_preauthenticated(
    manager: &WorkspaceManager,
    conn: &mut ConnectionWorkspace,
    pairing_tokens: Option<&PairingTokenStore>,
    preauthenticated_reason: Option<&'static str>,
    peer_ip: Option<&str>,
    msg: WorkspaceClientMessage,
) -> DispatchOutcome {
    handle_inner(
        manager,
        conn,
        pairing_tokens,
        preauthenticated_reason,
        peer_ip,
        msg,
    )
}

fn handle_inner(
    manager: &WorkspaceManager,
    conn: &mut ConnectionWorkspace,
    pairing_tokens: Option<&PairingTokenStore>,
    preauthenticated_reason: Option<&'static str>,
    peer_ip: Option<&str>,
    msg: WorkspaceClientMessage,
) -> DispatchOutcome {
    let outcome = match msg {
        WorkspaceClientMessage::OpenProjectRoot {
            path,
            init_if_missing,
        } => DispatchOutcome::just(open_workspace(manager, conn, path, init_if_missing)),
        WorkspaceClientMessage::CloseProjectRoot { id } => {
            DispatchOutcome::just(close_workspace(manager, conn, &id))
        }
        WorkspaceClientMessage::ListProjectRoots => {
            DispatchOutcome::just(vec![WorkspaceServerMessage::ProjectRootList {
                project_roots: manager.list_workspaces(),
            }])
        }
        WorkspaceClientMessage::ListHosts => DispatchOutcome::just(vec![
            WorkspaceServerMessage::HostList {
                hosts: manager.list_hosts(),
            },
        ]),
        WorkspaceClientMessage::UpsertHost { host } => {
            let host = manager.upsert_host(host);
            manager.broadcast_tree_changed(Some(conn.client_id));
            DispatchOutcome::just(vec![WorkspaceServerMessage::HostList { hosts: vec![host] }])
        }
        WorkspaceClientMessage::ListHostWorkspaces { host_id } => {
            DispatchOutcome::just(vec![WorkspaceServerMessage::HostWorkspaceList {
                workspaces: manager.list_host_workspaces(host_id.as_deref()),
            }])
        }
        WorkspaceClientMessage::ListWorkspaceTabs { workspace_id } => {
            DispatchOutcome::just(vec![WorkspaceServerMessage::WorkspaceTabList {
                tabs: manager.list_workspace_tabs(&workspace_id),
            }])
        }
        WorkspaceClientMessage::RequestHostWorkspaceTree => {
            let (hosts, workspaces, tabs) = manager.host_workspace_tree();
            DispatchOutcome::just(vec![WorkspaceServerMessage::HostWorkspaceTree {
                hosts,
                workspaces,
                tabs,
            }])
        }
        WorkspaceClientMessage::ResolveInitialWorkspace { preferred_host_id } => {
            let (workspace, reason) = manager.resolve_initial_workspace(conn, preferred_host_id);
            manager.broadcast_tree_changed(Some(conn.client_id));
            DispatchOutcome::just(vec![
                WorkspaceServerMessage::InitialWorkspaceResolved {
                    workspace,
                    reason,
                },
                WorkspaceServerMessage::ProjectRootChanged {
                    id: conn.active_workspace.clone(),
                },
            ])
        }
        WorkspaceClientMessage::CreateHostWorkspace {
            host_id,
            workspace_id,
            title,
            root_dir,
        } => {
            let workspace = manager.create_host_workspace(host_id.clone(), workspace_id, title, root_dir);
            DispatchOutcome::just(vec![
                WorkspaceServerMessage::HostWorkspaceUpserted {
                    workspace: workspace.clone(),
                },
                WorkspaceServerMessage::HostWorkspaceList {
                    workspaces: manager.list_host_workspaces(Some(&host_id)),
                },
            ])
        }
        WorkspaceClientMessage::CreateWorkspace {
            workspace_id,
            title,
            root_dir,
        } => {
            let host_id = machine_host_id();
            let workspace = manager.create_host_workspace(host_id.clone(), workspace_id, title, root_dir);
            DispatchOutcome::just(vec![
                WorkspaceServerMessage::HostWorkspaceUpserted {
                    workspace: workspace.clone(),
                },
                WorkspaceServerMessage::HostWorkspaceList {
                    workspaces: manager.list_host_workspaces(Some(&host_id)),
                },
            ])
        }
        WorkspaceClientMessage::CloseHostWorkspace { workspace_id } => {
            DispatchOutcome::just(match manager.close_host_workspace(&workspace_id) {
                Some(_) => {
                    manager.broadcast_tree_changed(Some(conn.client_id));
                    let (hosts, workspaces, tabs) = manager.host_workspace_tree();
                    vec![WorkspaceServerMessage::HostWorkspaceTree {
                        hosts,
                        workspaces,
                        tabs,
                    }]
                }
                None => vec![err(format!("no such host workspace: {workspace_id}"))],
            })
        }
        WorkspaceClientMessage::SwitchHostWorkspace { workspace_id } => {
            DispatchOutcome::just(match manager.switch_host_workspace(&workspace_id) {
                Some(workspace) => vec![WorkspaceServerMessage::HostWorkspaceChanged {
                    host_id: workspace.host_id,
                    workspace_id: Some(workspace.id),
                }],
                None => vec![err(format!("no such host workspace: {workspace_id}"))],
            })
        }
        WorkspaceClientMessage::SetWorkspaceRoot {
            workspace_id,
            root_dir,
        } => match manager.set_host_workspace_root(&workspace_id, root_dir) {
            Some(workspace) => {
                // Re-pointing the dir is a shared change — broadcast so every
                // client in this workspace re-roots its Explorer.
                manager.broadcast_tree_changed(Some(conn.client_id));
                DispatchOutcome::just(vec![WorkspaceServerMessage::HostWorkspaceUpserted {
                    workspace,
                }])
            }
            None => {
                DispatchOutcome::just(vec![err(format!("no such host workspace: {workspace_id}"))])
            }
        },
        WorkspaceClientMessage::CreateWorkspaceVault { workspace_id } => {
            DispatchOutcome::just(create_workspace_vault(manager, conn, &workspace_id))
        },
        WorkspaceClientMessage::ShareWorkspace { workspace_id } => match manager
            .set_host_workspace_visibility(
                &workspace_id,
                neoism_protocol::workspace::WorkspaceVisibility::Shared,
            ) {
            Some(workspace) => {
                manager.broadcast_tree_changed(Some(conn.client_id));
                DispatchOutcome::just(vec![WorkspaceServerMessage::HostWorkspaceUpserted {
                    workspace,
                }])
            }
            None => {
                DispatchOutcome::just(vec![err(format!("no such host workspace: {workspace_id}"))])
            }
        },
        WorkspaceClientMessage::StopSharingWorkspace { workspace_id } => match manager
            .set_host_workspace_visibility(
                &workspace_id,
                neoism_protocol::workspace::WorkspaceVisibility::Private,
            ) {
            Some(workspace) => {
                manager.broadcast_tree_changed(Some(conn.client_id));
                // Un-sharing must also KICK any guest who already
                // adopted this workspace: the visibility flip alone only
                // hides it from future joins. Broadcasting the farewell
                // to every client is simplest — a client with nothing
                // adopted no-ops on receipt.
                manager.broadcast_host_ended("Host stopped sharing this workspace");
                DispatchOutcome::just(vec![WorkspaceServerMessage::HostWorkspaceUpserted {
                    workspace,
                }])
            }
            None => {
                DispatchOutcome::just(vec![err(format!("no such host workspace: {workspace_id}"))])
            }
        },
        WorkspaceClientMessage::SendWorkspaceToDockerSandbox { workspace_id } => {
            DispatchOutcome::just(match export_workspace_snapshot(manager, &workspace_id) {
                Ok(path) => match start_local_docker_sandbox(&workspace_id) {
                    Ok(sandbox) => vec![err(format!(
                        "docker sandbox started at {url}; exported snapshot at {path} (receive/import not wired yet)",
                        url = sandbox.url,
                        path = path.display()
                    ))],
                    Err(e) => vec![err(format!(
                        "exported snapshot at {}; docker sandbox start failed: {e}",
                        path.display()
                    ))],
                },
                Err(e) => vec![err(e)],
            })
        }
        WorkspaceClientMessage::SendWorkspaceToCloud { workspace_id } => DispatchOutcome::just(
            match export_workspace_snapshot(manager, &workspace_id) {
                Ok(path) => vec![err(format!(
                    "cloud upload is not implemented yet; exported snapshot at {}",
                    path.display()
                ))],
                Err(e) => vec![err(e)],
            },
        ),
        WorkspaceClientMessage::SubscribeWorkspace { workspace_id } => DispatchOutcome::just({
            conn.active_workspace = Some(workspace_id.clone());
            manager
                .list_host_workspaces(None)
                .into_iter()
                .find(|workspace| workspace.id == workspace_id)
                .map(|workspace| vec![WorkspaceServerMessage::HostWorkspaceUpserted { workspace }])
                .unwrap_or_else(|| vec![err(format!("no such host workspace: {workspace_id}"))])
        }),
        WorkspaceClientMessage::UnsubscribeWorkspace { workspace_id } => DispatchOutcome::just({
            if conn.active_workspace.as_deref() == Some(workspace_id.as_str()) {
                conn.active_workspace = None;
            }
            Vec::new()
        }),
        WorkspaceClientMessage::ControlWorkspace {
            workspace_id,
            controller_host_id,
        } => DispatchOutcome::just(match manager.control_workspace(&workspace_id, controller_host_id) {
            Some(workspace) => vec![WorkspaceServerMessage::WorkspaceControlChanged { workspace }],
            None => vec![err(format!("no such host workspace: {workspace_id}"))],
        }),
        WorkspaceClientMessage::ReleaseWorkspaceControl {
            workspace_id,
            controller_host_id,
        } => DispatchOutcome::just(match manager.release_workspace_control(&workspace_id, &controller_host_id) {
            Some(workspace) => vec![WorkspaceServerMessage::WorkspaceControlChanged { workspace }],
            None => vec![err(format!("no such host workspace: {workspace_id}"))],
        }),
        WorkspaceClientMessage::MoveWorkspaceToHost {
            workspace_id,
            target_host_id,
        } => DispatchOutcome::just(match manager.move_workspace_to_host(&workspace_id, target_host_id) {
            Some(workspace) => vec![WorkspaceServerMessage::WorkspaceControlChanged { workspace }],
            None => vec![err(format!("no such host workspace: {workspace_id}"))],
        }),
        WorkspaceClientMessage::MoveTabToWorkspace {
            tab_id,
            target_workspace_id,
        } => DispatchOutcome::just(match manager.move_tab_to_workspace(&tab_id, target_workspace_id) {
            Some(tab) => vec![WorkspaceServerMessage::WorkspaceTabMoved { tab }],
            None => vec![err(format!("no such workspace tab: {tab_id}"))],
        }),
        WorkspaceClientMessage::MoveTabToHostWorkspace {
            tab_id,
            target_host_id,
            target_workspace_id,
        } => DispatchOutcome::just({
            let target_matches_host = manager
                .list_host_workspaces(Some(&target_host_id))
                .iter()
                .any(|workspace| workspace.id == target_workspace_id);
            if !target_matches_host {
                vec![err(format!(
                    "workspace {target_workspace_id} does not belong to host {target_host_id}"
                ))]
            } else {
                match manager.move_tab_to_workspace(&tab_id, target_workspace_id) {
                    Some(tab) => vec![WorkspaceServerMessage::WorkspaceTabMoved { tab }],
                    None => vec![err(format!("no such workspace tab: {tab_id}"))],
                }
            }
        }),
        WorkspaceClientMessage::PublishWorkspaceTabs {
            workspace_id,
            tabs,
        } => {
            // Store child tabs for one daemon-owned workspace. Other
            // clients refresh from the tree broadcast.
            manager.publish_workspace_tabs(&workspace_id, tabs);
            manager.broadcast_tree_changed(Some(conn.client_id));
            DispatchOutcome::default()
        }
        WorkspaceClientMessage::SwitchProjectRoot { id } => {
            DispatchOutcome::just(switch_workspace(manager, conn, id))
        }
        WorkspaceClientMessage::GetProjectRootInfo { id } => {
            DispatchOutcome::just(get_workspace_info(manager, conn, id))
        }
        WorkspaceClientMessage::RenameProjectRoot { id, name } => {
            DispatchOutcome::just(if manager.rename_workspace(&id, name) {
                Vec::new()
            } else {
                vec![err(format!("no such workspace: {id}"))]
            })
        }
        WorkspaceClientMessage::ForgetProjectRoot { id } => {
            DispatchOutcome::just(forget_workspace(manager, conn, id))
        }
        WorkspaceClientMessage::ListSessions => {
            DispatchOutcome::just(list_sessions(manager, conn))
        }
        WorkspaceClientMessage::SwitchSession { session_id } => {
            DispatchOutcome::just(switch_session(manager, conn, session_id))
        }
        WorkspaceClientMessage::NewSession { cwd, label } => {
            DispatchOutcome::just(new_session(manager, conn, cwd, label))
        }
        WorkspaceClientMessage::CloseSession { session_id } => {
            DispatchOutcome::just(close_session(manager, conn, session_id))
        }
        WorkspaceClientMessage::GetSessionState { session_id } => {
            DispatchOutcome::just(match manager.get_session(&session_id) {
                Some(s) => vec![WorkspaceServerMessage::SessionState {
                    id: s.id,
                    workspace_id: s.workspace_id,
                    cwd: s.cwd,
                    label: s.label,
                    last_active: s.last_active,
                }],
                None => vec![err(format!("no such session: {session_id}"))],
            })
        }
        WorkspaceClientMessage::SetCwd { session_id, path } => {
            DispatchOutcome::just(if manager.update_session_cwd(&session_id, path) {
                // Respawn-in-cwd policy (locked decision #3): PTYs never
                // migrate. We update the durable tab cwd here and drop
                // any stale PTY link, so the next attach respawns a fresh
                // shell in the new cwd rather than `cd`-ing a live one.
                //
                // The link drop is the load-bearing half: a tab's
                // recorded cwd is now ahead of the shell that was bound
                // to it, so that shell no longer represents the tab.
                // `SessionRegistry` is owned by the `/session` PTY socket
                // task (sibling on `AppState`, reachable from this
                // dispatcher only via `server.rs` wiring that belongs to
                // wave 1B), so this dispatcher cannot kill the old shell
                // itself; clearing the link is the in-scope, correct
                // hand-off. When the client next attaches the tab, the
                // PTY owner sees no link, spawns in `cwd`, and calls
                // `WorkspaceManager::link_pty_session` to re-bind.
                if let Some(stale) = manager.unlink_pty_session(&session_id) {
                    tracing::debug!(
                        %session_id,
                        stale_pty = %stale,
                        "SetCwd dropped stale PTY link; tab will respawn in new cwd"
                    );
                }
                Vec::new()
            } else {
                vec![err(format!("no such session: {session_id}"))]
            })
        }
        WorkspaceClientMessage::RenameSession { session_id, label } => {
            DispatchOutcome::just(if manager.rename_session(&session_id, label) {
                Vec::new()
            } else {
                vec![err(format!("no such session: {session_id}"))]
            })
        }
        WorkspaceClientMessage::BindEditorSurface {
            surface_id,
            session_id,
            path,
        } => DispatchOutcome::just(bind_editor_surface(
            manager, conn, surface_id, session_id, path,
        )),
        WorkspaceClientMessage::ListEditorSurfaces => {
            DispatchOutcome::just(list_editor_surfaces(manager, conn))
        }
        WorkspaceClientMessage::CloseEditorSurface { surface_id } => {
            DispatchOutcome::just(if manager.remove_editor_surface(&surface_id) {
                vec![WorkspaceServerMessage::EditorSurfaceClosed { surface_id }]
            } else {
                vec![err(format!("no such editor surface: {surface_id}"))]
            })
        }
        WorkspaceClientMessage::RequestOpenWindow {
            workspace_id,
            title,
        } => DispatchOutcome::just(open_logical_window(
            manager,
            conn,
            WorkspaceWindowKind::Terminal,
            workspace_id,
            None,
            title,
        )),
        WorkspaceClientMessage::RequestOpenNativeTab {
            workspace_id,
            parent_window_id,
            title,
        } => DispatchOutcome::just(open_logical_window(
            manager,
            conn,
            WorkspaceWindowKind::NativeTab,
            workspace_id,
            parent_window_id,
            title,
        )),
        WorkspaceClientMessage::RequestOpenConfigEditor { workspace_id } => {
            DispatchOutcome::just(open_logical_window(
                manager,
                conn,
                WorkspaceWindowKind::ConfigEditor,
                workspace_id,
                None,
                Some("Rio Settings".into()),
            ))
        }
        WorkspaceClientMessage::RequestCloseWindow { window_id } => {
            DispatchOutcome::just(if manager.remove_window(&window_id) {
                vec![WorkspaceServerMessage::WindowClosed { window_id }]
            } else {
                vec![err(format!("no such window: {window_id}"))]
            })
        }
        WorkspaceClientMessage::ListWindows => {
            DispatchOutcome::just(vec![WorkspaceServerMessage::WindowList {
                windows: manager.list_windows(),
            }])
        }
        WorkspaceClientMessage::RunWorkspaceAction { action } => {
            DispatchOutcome::just(run_workspace_action(manager, conn, action))
        }
        WorkspaceClientMessage::StoreClipboard { payload } => {
            conn.clipboard_payload = Some(payload.clone());
            DispatchOutcome::just(vec![WorkspaceServerMessage::ClipboardPayload {
                payload: Some(payload),
            }])
        }
        WorkspaceClientMessage::LoadClipboard => {
            DispatchOutcome::just(vec![WorkspaceServerMessage::ClipboardPayload {
                payload: conn.clipboard_payload.clone(),
            }])
        }
        WorkspaceClientMessage::MaterializeClipboardImage {
            payload,
            request_id,
        } => DispatchOutcome::just(materialize_clipboard_image(payload, request_id)),
        WorkspaceClientMessage::PaneLayoutOp {
            pane_external_id,
            op,
        } => DispatchOutcome::just(handle_pane_layout_op(
            manager,
            conn,
            pane_external_id,
            op,
        )),
        WorkspaceClientMessage::PublishPaneLayout { layout } => {
            DispatchOutcome::just(handle_publish_pane_layout(manager, layout))
        }
        WorkspaceClientMessage::Hello {
            token,
            client_name,
            client_id,
        } => handle_hello(
            manager,
            conn,
            pairing_tokens,
            preauthenticated_reason,
            peer_ip,
            token,
            client_name,
            client_id,
        ),
        WorkspaceClientMessage::RequestFullSnapshot { since_offset } => {
            DispatchOutcome::just(request_full_snapshot(manager, conn, since_offset))
        }
        WorkspaceClientMessage::GetWorkplacePreferences { workspace_id } => {
            let prefs = manager.get_preferences(&workspace_id);
            DispatchOutcome::just(vec![WorkspaceServerMessage::WorkplacePreferences {
                workspace_id,
                prefs,
            }])
        }
        WorkspaceClientMessage::SetWorkplacePreferences {
            workspace_id,
            prefs,
        } => {
            // Persist + fan out. The submitter does NOT get a synchronous
            // reply on this path — every connected websocket (the
            // submitter included) will see the
            // `WorkplacePreferencesChanged` broadcast on its prefs
            // subscription, which keeps the reply shape identical for
            // local + paired surfaces.
            manager.set_preferences(workspace_id, prefs);
            DispatchOutcome::default()
        }
        WorkspaceClientMessage::ListPairings => {
            // Reflect the daemon's current pairing-token set. When
            // `pairing_tokens` is `None` (in-process tests / legacy
            // callers that pre-resolve the handshake) we still
            // succeed with an empty list so the settings panel
            // renders "no devices" instead of an error.
            DispatchOutcome::just(vec![WorkspaceServerMessage::PairingList {
                pairings: pairing_tokens.map(|store| store.list()).unwrap_or_default(),
            }])
        }
        WorkspaceClientMessage::RevokePairing { fingerprint_prefix } => {
            // Revoke is best-effort: an unknown prefix simply reports
            // `removed = false`. The settings panel uses that flag to
            // distinguish "row already gone" from a synchronous error.
            let removed = pairing_tokens
                .map(|store| store.revoke(&fingerprint_prefix))
                .unwrap_or(false);
            tracing::info!(
                fingerprint_prefix = %fingerprint_prefix,
                removed,
                "pairing-token revocation requested"
            );
            DispatchOutcome::just(vec![WorkspaceServerMessage::PairingRevoked {
                fingerprint_prefix,
                removed,
            }])
        }
    };
    if !outcome.disconnect {
        manager.remember_client_state(conn);
    }
    outcome
}

fn create_workspace_vault(
    manager: &WorkspaceManager,
    conn: &ConnectionWorkspace,
    workspace_id: &str,
) -> Vec<WorkspaceServerMessage> {
    let Some(workspace) = manager
        .list_host_workspaces(None)
        .into_iter()
        .find(|workspace| workspace.id == workspace_id)
    else {
        return vec![err(format!("no such host workspace: {workspace_id}"))];
    };
    let Some(root) = workspace.root_dir.clone() else {
        return vec![err(format!(
            "workspace {workspace_id} has no directory to link"
        ))];
    };

    // Treat retries (including a double-click before the guest receives the
    // first upsert) as success. Refreshing through the manager also repairs
    // an older in-memory summary whose on-disk link already exists.
    if resolve_linked_vault_dir(&root).is_some() {
        let refreshed = manager
            .set_host_workspace_root(workspace_id, root)
            .unwrap_or(workspace);
        return vec![WorkspaceServerMessage::HostWorkspaceUpserted {
            workspace: refreshed,
        }];
    }

    let base_name = root
        .file_name()
        .and_then(|name| name.to_str())
        .filter(|name| !name.trim().is_empty())
        .unwrap_or(workspace.title.as_str());
    let base_name = sanitize_vault_name(base_name);
    let base_name = if base_name.is_empty() {
        "Workspace".to_string()
    } else {
        base_name
    };
    let vaults_root = neoism_workspace_index::notes_vaults_dir();
    let mut vault_name = base_name.clone();
    let mut suffix = 2usize;
    while vaults_root.join(&vault_name).exists() {
        vault_name = format!("{base_name} {suffix}");
        suffix += 1;
    }

    let result = (|| -> std::io::Result<std::path::PathBuf> {
        let mut notes_workspace = match neoism_workspace_index::load_workspace(&root)? {
            Some(workspace) => workspace,
            None => neoism_workspace_index::init_workspace(&root)?,
        };
        notes_workspace.config.notes.workspace = vault_name;
        neoism_workspace_index::ensure_notes_workspace(&notes_workspace)?;
        neoism_workspace_index::link_workspace_to_vault_project(
            &mut notes_workspace,
            &root,
        )
    })();

    match result {
        Ok(_) => {
            manager.broadcast_tree_changed(Some(conn.client_id));
            // Re-resolve and persist `linked_vault_dir` in the authoritative
            // summary before replying; merely reading the manager would keep
            // returning the pre-link `None` cached on this workspace.
            let refreshed = manager
                .set_host_workspace_root(workspace_id, root)
                .unwrap_or(workspace);
            vec![WorkspaceServerMessage::HostWorkspaceUpserted {
                workspace: refreshed,
            }]
        }
        Err(error) => vec![err(format!(
            "could not create notes vault for workspace {workspace_id}: {error}"
        ))],
    }
}

fn sanitize_vault_name(name: &str) -> String {
    name.trim()
        .chars()
        .map(|ch| {
            if matches!(ch, '/' | '\\' | ':' | '*' | '?' | '"' | '<' | '>' | '|') {
                '-'
            } else {
                ch
            }
        })
        .collect::<String>()
        .trim_matches('-')
        .trim()
        .to_string()
}

/// Evaluate an inbound `Hello { token, client_name }` against the
/// daemon's pairing-token store + `NEOISM_REQUIRE_AUTH` gate.
///
/// On accept, returns `HelloAck { accepted: true, peer_identity }`
/// with the optional best-effort `tailscale whois` lookup attached.
/// On reject, returns `HelloAck { accepted: false, reason }` plus
/// `disconnect = true` so the websocket task drops the connection
/// after writing the ack frame.
///
/// `pairing_tokens = None` is a test/legacy path that behaves as if
/// auth is not required — useful for in-process dispatchers that
/// don't carry a token store. `peer_ip` is best-effort; it's only
/// used to populate the optional `peer_identity` field on accept.
fn handle_hello(
    manager: &WorkspaceManager,
    conn: &mut ConnectionWorkspace,
    pairing_tokens: Option<&PairingTokenStore>,
    preauthenticated_reason: Option<&'static str>,
    peer_ip: Option<&str>,
    token: Option<String>,
    client_name: Option<String>,
    client_id: Uuid,
) -> DispatchOutcome {
    let outcome = if let Some(reason) = preauthenticated_reason {
        HandshakeOutcome::Accepted { reason }
    } else {
        match pairing_tokens {
            Some(store) => handshake::evaluate_hello(token.as_deref(), store),
            // No store wired: behave like "trust local". Matches the
            // env-off branch of `evaluate_hello` so test harnesses and
            // legacy callers don't have to construct a `PairingTokenStore`
            // just to dispatch a `Hello`.
            None => HandshakeOutcome::Accepted {
                reason: "trust-local (no pairing-token store)",
            },
        }
    };
    match outcome {
        HandshakeOutcome::Accepted { reason } => {
            let assigned_client_id = if client_id.is_nil() {
                Uuid::new_v4()
            } else {
                client_id
            };
            conn.client_id = assigned_client_id;
            if let Some(state) = manager.resume_client_state(assigned_client_id) {
                conn.active_workspace = state.active_workspace;
                conn.active_session = state.active_session;
            }
            // Stamp `last_seen` (and capture the first `client_name`
            // as the device label) on the matched token so the
            // revocation UI can render a useful row. No-op when there's
            // no token or no store — `touch` short-circuits on empty
            // input.
            if let (Some(store), Some(tok)) = (pairing_tokens, token.as_deref()) {
                store.touch(tok, client_name.as_deref());
            }
            let peer_identity = peer_ip
                .filter(|ip| !ip.is_empty())
                .and_then(handshake::tailscale_whois_blocking);
            tracing::info!(
                client_name = ?client_name,
                client_id = %assigned_client_id,
                peer_ip = ?peer_ip,
                peer_identity = ?peer_identity,
                outcome = reason,
                "Hello accepted"
            );
            DispatchOutcome::just(vec![WorkspaceServerMessage::HelloAck {
                accepted: true,
                reason: Some(reason.to_string()),
                peer_identity,
            }])
        }
        HandshakeOutcome::Rejected { reason } => {
            tracing::warn!(
                client_name = ?client_name,
                client_id = %client_id,
                peer_ip = ?peer_ip,
                outcome = reason,
                "Hello rejected — closing socket after ack"
            );
            DispatchOutcome {
                replies: vec![WorkspaceServerMessage::HelloAck {
                    accepted: false,
                    reason: Some(reason.to_string()),
                    peer_identity: None,
                }],
                disconnect: true,
            }
        }
    }
}

fn ensure_client_id(conn: &mut ConnectionWorkspace) -> Uuid {
    if conn.client_id.is_nil() {
        conn.client_id = Uuid::new_v4();
    }
    conn.client_id
}

fn request_full_snapshot(
    manager: &WorkspaceManager,
    conn: &mut ConnectionWorkspace,
    since_offset: Option<u64>,
) -> Vec<WorkspaceServerMessage> {
    let client_id = ensure_client_id(conn);
    let (sessions, layout, pty_offsets) = match conn.active_workspace.as_deref() {
        Some(ws_id) => {
            let sessions = manager.sessions_for_workspace(ws_id);
            let surfaces = manager.editor_surfaces_for_workspace(ws_id);
            let layout = manager.pane_layout_for_workspace(ws_id);
            let pty_offsets = surfaces
                .into_iter()
                .filter_map(|surface| surface.route_id.map(|route_id| (route_id, 0)))
                .collect();
            (sessions, layout, pty_offsets)
        }
        None => (Vec::new(), None, HashMap::new()),
    };

    let mut replies = vec![WorkspaceServerMessage::FullSnapshot {
        client_id,
        sessions,
        layout,
        prefs: manager.preferences_snapshot(),
        pty_offsets: pty_offsets.clone(),
    }];

    if let Some(from_offset) = since_offset {
        // The current daemon keeps live PTY readers per websocket, and
        // their byte stream history is not yet available to the
        // workspace manager. Emit explicit empty cursors for known
        // routes so reconnecting clients can still converge on "no
        // backlog available from this architecture" without guessing.
        for route_id in pty_offsets.keys().copied() {
            replies.push(WorkspaceServerMessage::PtyBacklog {
                route_id,
                bytes: Vec::new(),
                from_offset,
            });
        }
    }

    replies
}
