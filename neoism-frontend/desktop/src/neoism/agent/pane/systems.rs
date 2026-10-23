use super::*;
use crate::neoism::agent::system::{decode_source_key, AgentSystemSource};

#[derive(Default)]
pub(crate) struct PaneSystems {
    pub(crate) available: Vec<AgentSystemSource>,
    pub(crate) root: bool,
    pub(crate) explicit: bool,
    pub(crate) selected: Option<AgentSystemSource>,
    pub(crate) summaries: HashMap<String, super::super::api::SessionProjects>,
    pub(crate) streams: HashMap<String, AgentSessionCatalogStream>,
}

pub(crate) fn qualify_entry(
    source: &AgentSystemSource,
    mut entry: NeoismAgentSessionEntry,
) -> NeoismAgentSessionEntry {
    entry.id = source.key(&entry.id);
    entry.source_key = entry.source_key.map(|key| source.key(&key));
    entry.directory = entry.directory.map(|path| source.key(&path));
    entry.branch_root_id = entry.branch_root_id.map(|id| source.key(&id));
    entry
}

impl NeoismAgentPane {
    pub(crate) fn set_system_sources(
        &mut self,
        sources: Vec<AgentSystemSource>,
        root: bool,
    ) {
        let changed = self.systems.available != sources || self.systems.root != root;
        let was_authorized = self.execution_authorized();
        let reconnected = self.systems.selected.as_ref().and_then(|selected| {
            sources
                .iter()
                .find(|source| {
                    source.authorized()
                        && source.id == selected.id
                        && source.system_id == selected.system_id
                        && (source.generation != selected.generation
                            || source.server != selected.server
                            || !was_authorized)
                })
                .cloned()
        });
        if let Some(source) = reconnected {
            self.rebind_system_transport(source);
        }
        self.systems.available = sources;
        self.systems.root = root;
        if !self.execution_authorized() {
            self.event_stream = None;
            self.session_catalog_stream = None;
            self.systems.streams.clear();
        }
        if changed {
            self.catalog_generation = self.catalog_generation.wrapping_add(1);
            self.systems.summaries.clear();
            self.systems.streams.clear();
            if self.side_panel.catalog_directory().is_none() {
                self.side_panel.set_session_projects(Vec::new());
                self.side_panel.set_pinned_sessions(Vec::new());
                if self.execution_authorized() {
                    self.request_side_panel_session_page(None);
                }
            }
        }
    }

    /// A verified reconnect may replace the forward but not the machine identity.
    /// Replace channels/streams rather than admitting replies from the old transport.
    fn rebind_system_transport(&mut self, source: AgentSystemSource) {
        let mut previous = std::mem::replace(self, Self::default());
        previous.event_stream = None;
        previous.session_catalog_stream = None;
        previous.systems.streams.clear();
        self.server = source.server.clone();
        self.catalog_generation = previous.catalog_generation.wrapping_add(1);
        self.directory = previous.directory.take();
        self.session_id = previous.session_id.take();
        self.session_tree_root_id = previous.session_tree_root_id.take();
        self.messages = std::mem::take(&mut previous.messages);
        self.new_chat_source = previous.new_chat_source;
        self.default_chat_source = previous.default_chat_source;
        self.draft_source_explicit = previous.draft_source_explicit;
        self.session_sources = std::mem::take(&mut previous.session_sources);
        self.input = std::mem::take(&mut previous.input);
        self.input_attachments = std::mem::take(&mut previous.input_attachments);
        self.cursor_byte = previous.cursor_byte;
        self.prompt_draft_id = previous.prompt_draft_id;
        // Submission outcomes can be ambiguous across a disconnect. Retain them
        // for explicit review, but never resume the old dispatch queue.
        self.uncertain_prompts = std::mem::take(&mut previous.uncertain_prompts);
        self.uncertain_prompts
            .append(&mut previous.pending_prompt_dispatches);
        let had_in_flight_prompt = previous.prompt_dispatch_in_flight;
        self.systems.root = previous.systems.root;
        self.systems.explicit = true;
        self.systems.selected = Some(source);
        if let Some(wake) = previous.event_wake.take() {
            self.set_event_wake(wake);
        }
        self.apply_config_defaults();
        if had_in_flight_prompt || !self.uncertain_prompts.is_empty() {
            self.push_notice("Machine reconnected. Previous submitted prompts were not replayed; review the conversation before sending again.", NeoismAgentNoticeLevel::Warn);
        }
        if let Some(session) = self.session_id.clone() {
            self.hydrate_runtime_status_for_session(&session);
            let root = self.session_tree_root_id.clone().unwrap_or(session);
            self.start_session_updates(&root);
        }
    }

    pub(crate) fn system_source(&self, server: &str) -> Option<&AgentSystemSource> {
        self.systems
            .available
            .iter()
            .find(|s| s.authorized() && s.server == server)
    }

    pub(crate) fn resolve_catalog_key(&self, key: &str) -> Option<(String, String)> {
        if let Some((id, generation, value)) = decode_source_key(key) {
            let source =
                self.systems.available.iter().find(|s| {
                    s.id == id && s.generation == generation && s.authorized()
                })?;
            Some((source.server.clone(), value))
        } else {
            Some((self.server.clone(), key.to_owned()))
        }
    }

    pub(crate) fn apply_system_summary(
        &mut self,
        server: String,
        discovery: super::super::api::SessionProjects,
    ) {
        self.systems.summaries.insert(server, discovery);
        let mut projects = Vec::new();
        let mut labels = HashMap::new();
        let mut pinned = Vec::new();
        for (server, summary) in &self.systems.summaries {
            if server == &self.server {
                projects.extend(summary.projects.clone());
                pinned.extend(summary.pinned_sessions.clone());
            } else if let Some(source) = self.system_source(server) {
                for (path, _) in &summary.projects {
                    let name = path
                        .trim_end_matches(['/', '\\'])
                        .rsplit(['/', '\\'])
                        .next()
                        .unwrap_or(path);
                    labels.insert(source.key(path), format!("{name} · {}", source.name));
                }
                projects.extend(
                    summary
                        .projects
                        .iter()
                        .map(|(path, time)| (source.key(path), *time)),
                );
                pinned.extend(
                    summary
                        .pinned_sessions
                        .iter()
                        .cloned()
                        .map(|entry| qualify_entry(source, entry)),
                );
            }
        }
        self.side_panel.set_project_labels(labels);
        self.side_panel.set_session_projects(projects);
        self.side_panel.set_pinned_sessions(pinned);
    }

    pub(crate) fn sync_workspace_server(&mut self, server: String) {
        if !self.systems.explicit {
            self.switch_server(server);
        }
    }

    pub(crate) fn is_system_execution_backend(&self) -> bool {
        self.systems.selected.is_some()
            || super::super::system::is_system_agent_server(&self.server)
    }

    pub(crate) fn apply_session_move(
        &mut self,
        directory: String,
        switch_workspace: bool,
    ) {
        self.directory = Some(directory.clone());
        self.refresh_provider_readiness();
        self.invalidate_skill_options();
        self.side_panel.invalidate_goal_refresh();
        if self.is_system_execution_backend() {
            // Host-local movement only changes this pane's execution cwd. Never
            // interpret a remote path as a directory on the GUI's machine.
            self.systems.explicit = true;
        } else if switch_workspace {
            self.ui_events
                .push(NeoismAgentUiEvent::SwitchWorkspace { directory });
        }
    }

    pub(crate) fn selected_system_source(&self) -> Option<&AgentSystemSource> {
        self.systems.selected.as_ref()
    }

    pub(crate) fn execution_authorized(&self) -> bool {
        self.systems.selected.as_ref().is_none_or(|selected| {
            selected.authorized()
                && self
                    .systems
                    .available
                    .iter()
                    .any(|s| s == selected && s.authorized())
        })
    }

    pub(crate) fn pin_system_backend(
        &mut self,
        source: AgentSystemSource,
        directory: Option<String>,
    ) {
        let available = self.systems.available.clone();
        let root = self.systems.root;
        self.switch_server(source.server.clone());
        self.systems.available = available;
        if !self.systems.available.iter().any(|s| s == &source) {
            self.systems.available.push(source.clone());
        }
        self.systems.root = root;
        self.directory = directory.or_else(|| Some(source.home_directory.clone()));
        self.systems.selected = Some(source);
        self.systems.explicit = true;
    }

    fn can_select_execution_system(&self) -> bool {
        self.systems.root
            && self.session_id.is_none()
            && !self.is_streaming()
            && !self.prompt_dispatch_in_flight
            && self.pending_prompt_dispatches.is_empty()
            && self.uncertain_prompts.is_empty()
    }

    pub(crate) fn composer_system_control(
        &self,
    ) -> Option<neoism_ui::panels::agent_pane::composer_interactions::ComposerSystemControl>
    {
        if !self.systems.root && self.systems.selected.is_none() {
            return None;
        }
        Some(
            neoism_ui::panels::agent_pane::composer_interactions::ComposerSystemControl {
                label: self
                    .systems
                    .selected
                    .as_ref()
                    .map(|s| s.name.clone())
                    .unwrap_or_else(|| "This computer".into()),
                selectable: self.can_select_execution_system(),
            },
        )
    }

    pub(crate) fn open_system_picker(&mut self) {
        if !self.can_select_execution_system() {
            return;
        }
        let mut options = vec![NeoismAgentPickerOption::new(
            "This computer",
            "",
            "",
            "local",
        )];
        options.extend(
            self.systems
                .available
                .iter()
                .filter(|s| s.authorized())
                .map(|s| {
                    NeoismAgentPickerOption::new(&s.name, "", &s.home_directory, &s.id)
                }),
        );
        let selected = self
            .systems
            .selected
            .as_ref()
            .and_then(|s| options.iter().position(|o| o.value == s.id))
            .unwrap_or(0);
        self.picker = Some(NeoismAgentPicker::new(
            NeoismAgentPickerKind::System,
            "Run on",
            options,
            selected,
        ));
    }

    pub(crate) fn select_system(&mut self, id: &str) -> bool {
        if !self.can_select_execution_system() {
            return false;
        }
        let source = if id == "local" {
            None
        } else {
            let Some(source) = self
                .systems
                .available
                .iter()
                .find(|s| s.id == id && s.authorized())
                .cloned()
            else {
                return false;
            };
            Some(source)
        };
        let server = source
            .as_ref()
            .map(|s| s.server.clone())
            .unwrap_or_else(neoism_agent_server);
        // Endpoint changes reset endpoint-owned caches and pending work, but not
        // the user's draft text, images, cursor, or draft identity.
        let input = std::mem::take(&mut self.input);
        let attachments = std::mem::take(&mut self.input_attachments);
        let cursor = self.cursor_byte;
        let draft = self.prompt_draft_id;
        let available = self.systems.available.clone();
        let wake = self.event_wake.clone();
        self.switch_server(server);
        self.directory = source
            .as_ref()
            .map(|s| s.home_directory.clone())
            .or_else(|| dirs::home_dir().map(|p| p.to_string_lossy().into_owned()));
        self.systems.available = available;
        self.systems.root = true;
        self.systems.explicit = true;
        self.systems.selected = source;
        self.input = input;
        self.input_attachments = attachments;
        self.cursor_byte = cursor;
        self.prompt_draft_id = draft;
        if let Some(wake) = wake {
            self.set_event_wake(wake);
        }
        self.refresh_provider_readiness();
        self.invalidate_skill_options();
        self.close_picker();
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{atomic::AtomicBool, Arc};

    fn source(id: &str, generation: u64) -> AgentSystemSource {
        AgentSystemSource {
            id: id.into(),
            name: format!("Machine {id}"),
            generation,
            system_id: id.into(),
            server: format!(
                "http://127.0.0.1:{}/agent/system",
                if id == "b" { 2 } else { 1 }
            ),
            home_directory: format!("/remote/{id}"),
            authorized: Arc::new(AtomicBool::new(true)),
        }
    }

    #[test]
    fn same_path_and_id_are_distinct_in_the_real_catalog() {
        let a = source("a", 1);
        let b = source("b", 2);
        let mut pane = NeoismAgentPane::default();
        pane.systems.root = true;
        pane.systems.available = vec![a.clone(), b.clone()];
        for source in [&a, &b] {
            pane.apply_system_summary(
                source.server.clone(),
                super::super::super::api::SessionProjects {
                    projects: vec![("/work/same".into(), 1)],
                    pinned_sessions: vec![NeoismAgentSessionEntry::new(
                        "same-id", "Chat", "",
                    )
                    .with_directory(Some("/work/same".into()))
                    .with_pinned(true)],
                },
            );
        }
        assert!(pane.side_panel.catalog_session(&a.key("same-id")).is_some());
        assert!(pane.side_panel.catalog_session(&b.key("same-id")).is_some());
        assert!(pane.resolve_catalog_key(&a.key("same-id")).is_some());
        assert_ne!(a.key("same-id"), b.key("same-id"));
        assert_ne!(a.key("/work/same"), b.key("/work/same"));
        pane.side_panel.set_last_panel_height_rows(50);
        let requests = pane.side_panel.begin_project_session_requests();
        assert_eq!(
            requests.len(),
            2,
            "both source-qualified project pages must be independently requestable"
        );
        for (directory, cursor, generation) in requests {
            let source = if directory == a.key("/work/same") {
                &a
            } else {
                &b
            };
            assert_eq!(directory, source.key("/work/same"));
            assert!(cursor.is_none());
            pane.side_panel.set_project_session_page(
                &directory,
                None,
                vec![qualify_entry(
                    source,
                    NeoismAgentSessionEntry::new("paged-id", "Paged", "")
                        .with_directory(Some("/work/same".into())),
                )],
                Some("same-cursor".into()),
                generation,
            );
        }
        for source in [&a, &b] {
            let entry = pane
                .side_panel
                .catalog_session(&source.key("paged-id"))
                .expect("both paginated rows survive the merged projection");
            assert_eq!(
                entry.directory.as_deref(),
                Some(source.key("/work/same").as_str())
            );
            assert_eq!(
                pane.resolve_catalog_key(&entry.id),
                Some((source.server.clone(), "paged-id".into()))
            );
        }
    }

    #[test]
    fn revoked_source_rejects_keys_callbacks_and_pane_execution() {
        let source = source("revoked", 1);
        let mut pane = NeoismAgentPane::default();
        pane.systems.root = true;
        pane.systems.available = vec![source.clone()];
        pane.systems.selected = Some(source.clone());
        let generation = pane.catalog_generation;
        assert!(pane.catalog_callback_is_current(&source.server, generation, None));
        source.revoke();
        assert!(pane.resolve_catalog_key(&source.key("session")).is_none());
        assert!(!pane.catalog_callback_is_current(&source.server, generation, None));
        assert!(!pane.execution_authorized());
    }

    #[test]
    fn verified_same_machine_reconnect_rebinds_transport_without_replaying_prompts() {
        let original = source("reconnected", 1);
        let mut replacement = source("reconnected", 2);
        replacement.server = "http://127.0.0.1:2/agent/system".into();
        let mut pane = NeoismAgentPane::default();
        pane.pin_system_backend(original.clone(), Some("/remote/chosen".into()));
        pane.session_id = Some("existing-chat".into());
        pane.session_tree_root_id = Some("family-root".into());
        pane.input = "unfinished draft".into();
        pane.cursor_byte = 5;
        pane.prompt_draft_id = 42;
        pane.input_attachments
            .push(NeoismAgentInputAttachment::File {
                token: "image".into(),
                filename: "draft.png".into(),
                url: "data:image/png;base64,aW1hZ2U=".into(),
                mime: "image/png".into(),
            });
        let image = pane.input_attachments[0].clone();
        for message_id in ["uncertain", "queued"] {
            pane.pending_prompt_dispatches
                .push_back(PendingPromptDispatch {
                    reconcile_before_send: true,
                    origin_session_id: pane.session_id.clone(),
                    origin_draft_id: 42,
                    source: pane.new_chat_source,
                    draft_external_selections: HashMap::new(),
                    server: pane.server.clone(),
                    directory: pane.directory.clone(),
                    message_id: message_id.into(),
                    parts: vec![serde_json::json!({"type":"text", "text":message_id})],
                    system: None,
                    agent: None,
                    model: "test/model".into(),
                    connection_id: None,
                    thinking: None,
                    delivery: neoism_protocol::agent::PromptDelivery::Steer,
                    author: None,
                    transcript_echo: None,
                    event_wake: None,
                });
        }
        pane.uncertain_prompts
            .push_back(pane.pending_prompt_dispatches.pop_front().unwrap());
        pane.prompt_dispatch_in_flight = true;
        let stale_sender = pane.background_sender();
        let generation = pane.catalog_generation;
        original.revoke();
        pane.set_system_sources(Vec::new(), true);
        assert!(!pane.execution_authorized());
        pane.set_system_sources(vec![replacement.clone()], true);
        assert!(pane.execution_authorized());
        assert_eq!(pane.server, replacement.server);
        assert_eq!(pane.systems.selected.as_ref(), Some(&replacement));
        assert_eq!(pane.session_id.as_deref(), Some("existing-chat"));
        assert_eq!(pane.session_tree_root_id.as_deref(), Some("family-root"));
        assert_eq!(pane.directory.as_deref(), Some("/remote/chosen"));
        assert_eq!(pane.input, "unfinished draft");
        assert_eq!(pane.input_attachments, vec![image]);
        assert_eq!(pane.cursor_byte, 5);
        assert_eq!(pane.prompt_draft_id, 42);
        assert!(pane.catalog_generation > generation);
        assert!(pane.event_stream.is_some());
        assert!(pane.pending_prompt_dispatches.is_empty());
        assert!(!pane.prompt_dispatch_in_flight);
        assert_eq!(pane.uncertain_prompts.len(), 2);
        assert!(!pane
            .ui_events
            .iter()
            .any(|event| matches!(event, NeoismAgentUiEvent::SwitchWorkspace { .. })));
        assert!(stale_sender
            .send(NeoismAgentBackgroundUpdate::SessionProjectsRefreshed {
                server: original.server,
                catalog_generation: generation,
                generation: 0,
                result: Err("stale transport reply".into()),
            })
            .is_err());
    }

    #[test]
    fn unavailable_system_pauses_ingestion_and_recovers_the_same_transport_epoch() {
        let source = source("temporary", 1);
        let mut pane = NeoismAgentPane::default();
        pane.pin_system_backend(source.clone(), Some("/remote/chosen".into()));
        pane.session_id = Some("existing-chat".into());
        pane.event_stream = Some(AgentSessionEventStream::with_updates_for_test(
            "existing-chat",
            [AgentSessionUpdate::System {
                title: "Stale".into(),
                body: "must not be ingested".into(),
            }],
        ));
        pane.set_system_sources(Vec::new(), true);
        assert!(!pane.execution_authorized());
        assert!(pane.event_stream.is_none());
        assert!(!pane.drain_server_updates());
        assert!(!pane.drain_background_updates());
        assert!(pane.messages.is_empty());
        pane.set_system_sources(vec![source.clone()], true);
        assert!(pane.execution_authorized());
        assert_eq!(pane.server, source.server);
        assert_eq!(pane.session_id.as_deref(), Some("existing-chat"));
        assert_eq!(pane.directory.as_deref(), Some("/remote/chosen"));
        assert!(pane.event_stream.is_some());
    }

    #[test]
    fn reconnect_to_different_machine_under_same_saved_id_stays_blocked() {
        let original = source("reused-id", 1);
        let mut different = source("reused-id", 2);
        different.system_id = "different-descriptor-machine".into();
        different.server = "http://127.0.0.1:2/agent/system".into();
        let mut pane = NeoismAgentPane::default();
        pane.pin_system_backend(original.clone(), Some("/remote/project".into()));
        pane.session_id = Some("same-session-id".into());
        pane.input = "preserve".into();
        original.revoke();
        pane.set_system_sources(vec![different], true);
        assert!(!pane.execution_authorized());
        assert_eq!(pane.systems.selected.as_ref(), Some(&original));
        assert_eq!(pane.server, original.server);
        assert_eq!(pane.session_id.as_deref(), Some("same-session-id"));
        assert_eq!(pane.directory.as_deref(), Some("/remote/project"));
        assert_eq!(pane.input, "preserve");
        assert!(pane.is_system_execution_backend());
    }

    #[test]
    fn explicit_remote_pane_survives_workspace_backend_sync() {
        let source = source("pinned", 1);
        let mut pane = NeoismAgentPane::default();
        pane.pin_system_backend(source.clone(), Some("/remote/project".into()));
        pane.session_id = Some("chat".into());
        pane.sync_workspace_server("http://local/agent".into());
        assert_eq!(pane.server, source.server);
        assert_eq!(pane.directory.as_deref(), Some("/remote/project"));
        assert_eq!(pane.session_id.as_deref(), Some("chat"));
    }

    #[test]
    fn system_session_move_changes_remote_cwd_without_local_workspace_intent() {
        let source = source("moved", 1);
        let mut pane = NeoismAgentPane::default();
        assert!(!pane.is_system_execution_backend());
        pane.pin_system_backend(source.clone(), Some("/remote/old".into()));
        pane.session_id = Some("chat".into());
        assert!(pane.is_system_execution_backend(), "all file links, including existing local same-path files, must reject local resolution");
        pane.apply_session_move("/remote/destination".into(), true);
        assert_eq!(pane.directory.as_deref(), Some("/remote/destination"));
        assert_eq!(pane.server, source.server);
        assert!(pane.systems.explicit);
        assert!(!pane
            .ui_events
            .iter()
            .any(|event| matches!(event, NeoismAgentUiEvent::SwitchWorkspace { .. })));
        pane.sync_workspace_server("http://local/agent".into());
        assert_eq!(pane.server, source.server);
        source.revoke();
        assert!(
            pane.is_system_execution_backend(),
            "revocation must never allow local file fallback"
        );
    }

    #[test]
    fn rendered_composer_exposes_system_control_only_in_supported_contexts() {
        use neoism_ui::panels::agent_pane::view::user_input::AgentUserInputPane;
        let mut pane = NeoismAgentPane::default();
        assert!(AgentUserInputPane::composer_system(&pane).is_none());
        pane.systems.root = true;
        let local = AgentUserInputPane::composer_system(&pane).unwrap();
        assert_eq!(local.label, "This computer");
        assert!(local.selectable);
        pane.systems.selected = Some(source("selected", 1));
        let remote = AgentUserInputPane::composer_system(&pane).unwrap();
        assert_eq!(remote.label, "Machine selected");
        assert!(remote.selectable);
        pane.session_id = Some("existing-chat".into());
        assert!(
            !AgentUserInputPane::composer_system(&pane)
                .unwrap()
                .selectable
        );
        pane.systems.root = false;
        assert!(
            !AgentUserInputPane::composer_system(&pane)
                .unwrap()
                .selectable
        );
        pane.systems.selected = None;
        assert!(AgentUserInputPane::composer_system(&pane).is_none());
    }

    #[test]
    fn root_target_change_preserves_draft_and_uses_owning_home() {
        let source = source("draft", 1);
        let mut pane = NeoismAgentPane::default();
        pane.systems.root = true;
        pane.systems.available = vec![source.clone()];
        pane.input = "keep typed prompt".into();
        pane.input_attachments
            .push(NeoismAgentInputAttachment::Text {
                token: "paste".into(),
                text: "attachment".into(),
            });
        pane.directory = Some("/local/only".into());
        let draft = pane.prompt_draft_id;
        assert!(pane.select_system("draft"));
        assert_eq!(pane.input, "keep typed prompt");
        assert_eq!(pane.input_attachments.len(), 1);
        assert_eq!(pane.input_attachments[0].token(), "paste");
        assert_eq!(pane.prompt_draft_id, draft);
        assert_eq!(
            pane.directory.as_deref(),
            Some(source.home_directory.as_str())
        );
        assert!(pane.session_id.is_none());
        pane.systems.root = false;
        assert!(!pane.select_system("local"));
        assert_eq!(pane.server, source.server);
    }
}
