//! Desktop routing for the shared modal. Joined browsing is exclusively files-plane I/O.
use crate::screen::{
    DesktopBrowserPurpose, DesktopBrowserRequestKind as RequestKind,
    DesktopBrowserTarget, RemoteBrowserRequest, Screen,
};
use neoism_protocol::files::{FilesClientMessage, FilesServerMessage};
use neoism_ui::panels::file_browser::{
    FileBrowserEntry, FileBrowserLocation, FileBrowserMode, FileBrowserRequest,
};
use std::path::PathBuf;

const MAX_BROWSER_IMAGE_BYTES: u64 = 20 * 1024 * 1024;

fn request_applies(
    pending: &RemoteBrowserRequest,
    generation: u64,
    target_current: bool,
    active: bool,
    current_path: &str,
) -> bool {
    if pending.generation != generation || !target_current {
        return false;
    }
    match pending.kind {
        RequestKind::Locations => active,
        RequestKind::Listing => active && pending.path.as_deref() == Some(current_path),
        // Selection closes the modal, but its captured target remains live until commit.
        _ => true,
    }
}

fn request_owns_connection(pending: &RemoteBrowserRequest, key: Option<usize>) -> bool {
    pending.connection_key == key
}

fn valid_image_size(size: Option<u64>) -> bool {
    size.is_some_and(|size| size > 0 && size <= MAX_BROWSER_IMAGE_BYTES)
}

impl Screen<'_> {
    fn capture_browser_target(
        &self,
        purpose: DesktopBrowserPurpose,
    ) -> Option<DesktopBrowserTarget> {
        let joined = self.context_manager.current_workspace_is_remote_joined();
        // A missing host root is an error, never permission to inspect guest files.
        let root = self
            .active_workspace_root
            .clone()
            .or_else(|| self.active_pane_workspace_root())
            .or_else(|| {
                if joined {
                    None
                } else {
                    std::env::current_dir().ok()
                }
            })?;
        let agent_purpose = matches!(purpose, DesktopBrowserPurpose::AgentDirectory | DesktopBrowserPurpose::AgentImage);
        let agent = if agent_purpose {
            Some(
                self.context_manager
                    .current()
                    .neoism_agent
                    .as_ref()?
                    .file_browser_identity(),
            )
        } else {
            None
        };
        Some(DesktopBrowserTarget {
            purpose,
            workspace_id: self.current_workspace_id(),
            root,
            scope: format!(
                "{}:{:?}",
                self.terminal_completion_scope(),
                self.context_manager.daemon_connection_key()
            ),
            joined,
            route: agent_purpose.then_some(self.context_manager.current().route_id),
            agent,
        })
    }

    fn browser_target_is_current(&self) -> bool {
        self.desktop_browser_target.as_ref().is_some_and(|target| {
            self.capture_browser_target(target.purpose.clone()).as_ref() == Some(target)
        })
    }

    pub(crate) fn open_agent_image_browser(&mut self) {
        self.open_desktop_file_browser(DesktopBrowserPurpose::AgentImage);
    }

    pub(crate) fn open_agent_directory_browser(&mut self) {
        self.open_desktop_file_browser(DesktopBrowserPurpose::AgentDirectory);
    }

    pub(crate) fn open_workspace_directory_browser(&mut self) {
        self.open_desktop_file_browser(DesktopBrowserPurpose::Workspace);
    }

    pub(crate) fn open_add_conversation_workspace_browser(&mut self) {
        self.capture_conversation_home_anchor();
        self.open_desktop_file_browser(DesktopBrowserPurpose::AddWorkspace);
    }

    fn open_desktop_file_browser(&mut self, purpose: DesktopBrowserPurpose) {
        let Some(target) = self.capture_browser_target(purpose) else {
            self.file_tree_notify(
                "No directory target is available".to_string(),
                neoism_ui::panels::notifications::NotificationLevel::Warn,
            );
            return;
        };
        self.desktop_browser_generation = self.desktop_browser_generation.wrapping_add(1);
        let mode = if target.purpose == DesktopBrowserPurpose::AgentImage {
            FileBrowserMode::AttachImage
        } else {
            FileBrowserMode::ChooseDirectory
        };
        // Host recents must never leak into local browsing or a different host.
        let recents = if target.joined {
            Vec::new()
        } else {
            std::fs::read(
                neoism_backend::config::config_dir_path()
                    .join("file-browser-recents.json"),
            )
            .ok()
            .and_then(|bytes| serde_json::from_slice::<Vec<String>>(&bytes).ok())
            .unwrap_or_default()
        };
        let start = target
            .agent
            .as_ref()
            .and_then(|a| a.3.as_deref())
            .unwrap_or_else(|| target.root.to_str().unwrap_or(""));
        self.renderer.file_browser.open(mode, start, recents);
        if !target.joined {
            let candidates = [
                ("workspace", "Workspace", Some(target.root.clone())),
                ("home", "Home", dirs::home_dir()),
                ("documents", "Documents", dirs::document_dir()),
                ("downloads", "Downloads", dirs::download_dir()),
                ("pictures", "Pictures", dirs::picture_dir()),
            ];
            let locations = candidates
                .into_iter()
                .filter_map(|(kind, label, path)| {
                    let path = path?.canonicalize().ok()?;
                    path.is_dir().then(|| FileBrowserLocation {
                        kind: kind.into(),
                        label: label.into(),
                        path: path.to_string_lossy().into_owned(),
                    })
                })
                .collect();
            self.renderer.file_browser.set_locations(locations);
        }
        self.desktop_browser_target = Some(target);
        self.pump_agent_image_browser();
        self.mark_dirty();
    }

    fn browser_failure(&mut self, message: String, selection: bool) {
        self.renderer.file_browser.set_error(message.clone());
        if selection {
            self.file_tree_notify(
                message,
                neoism_ui::panels::notifications::NotificationLevel::Error,
            );
            self.desktop_browser_target = None;
        }
        self.mark_dirty();
    }

    fn dispatch_browser_request(
        &mut self,
        message: FilesClientMessage,
        path: Option<String>,
        kind: RequestKind,
    ) {
        let Some(target) = self.desktop_browser_target.as_ref() else {
            return;
        };
        let Some((handle, runtime)) =
            self.context_manager.daemon_link_handle_and_runtime()
        else {
            self.browser_failure(
                "Host connection unavailable. Reopen the picker after reconnecting."
                    .into(),
                !matches!(kind, RequestKind::Locations | RequestKind::Listing),
            );
            return;
        };
        let root = target.root.clone();
        let request_id = handle.allocate_request_id();
        self.pending_remote_browser.insert(
            request_id,
            RemoteBrowserRequest {
                connection_key: self.context_manager.daemon_connection_key(),
                generation: self.desktop_browser_generation,
                path,
                kind,
            },
        );
        let generation = self.desktop_browser_generation;
        let errors = self.desktop_browser_errors.0.clone();
        let events = self.context_manager.event_proxy();
        let window = self.context_manager.window_id();
        let route = self.context_manager.current().route_id;
        runtime.spawn(async move {
            let error = match handle
                .send_files_with_request_id(request_id, message, Some(root))
                .await
            {
                Err(error) => format!("Could not reach host: {error}"),
                Ok(()) => {
                    tokio::time::sleep(std::time::Duration::from_secs(15)).await;
                    "Host file-browser request timed out. Reopen the picker to retry."
                        .to_string()
                }
            };
            // Successful replies remove the request before this deadline is drained.
            let _ = errors.send((request_id, generation, error));
            events.send_event(
                neoism_backend::event::RioEvent::RenderRoute(route).into(),
                window,
            );
        });
    }

    fn commit_browser_directory(&mut self, path: PathBuf) {
        if !self.browser_target_is_current() {
            return;
        }
        let Some(target) = self.desktop_browser_target.clone() else {
            return;
        };
        if !target.joined && !path.is_dir() {
            self.browser_failure(
                "The selected location is no longer a folder".into(),
                true,
            );
            return;
        }
        match target.purpose {
            DesktopBrowserPurpose::AddWorkspace => {
                if let Err(error) = self.add_conversation_workspace(path) {
                    self.browser_failure(error, true);
                    return;
                }
            }
            DesktopBrowserPurpose::Workspace => {
                if let Err(error) = self.commit_workspace_directory(path) {
                    self.browser_failure(error, true);
                    return;
                }
            }
            DesktopBrowserPurpose::AgentDirectory => {
                if let Some(agent) =
                    self.context_manager.current_mut().neoism_agent.as_mut()
                {
                    agent.choose_session_directory(path.to_string_lossy().into_owned());
                }
            }
            DesktopBrowserPurpose::AgentImage => return,
        }
        self.desktop_browser_target = None;
        self.mark_dirty();
    }

    /// Kept under the original name for existing modal input callers; also run before rendering.
    pub(crate) fn pump_agent_image_browser(&mut self) {
        let errors: Vec<_> = self.desktop_browser_errors.1.try_iter().collect();
        for (id, generation, message) in errors {
            if self
                .pending_remote_browser
                .get(&id)
                .is_some_and(|pending| pending.generation == generation)
            {
                self.apply_desktop_browser_message(
                    id,
                    &FilesServerMessage::Error { message },
                );
            }
        }
        if self.desktop_browser_target.is_none() {
            return;
        }
        if !self.browser_target_is_current() {
            self.renderer.file_browser.close();
            self.renderer.file_browser.take_selection();
            self.desktop_browser_target = None;
            return;
        }
        let target = self.desktop_browser_target.clone().unwrap();
        if target.joined {
            for request in self.renderer.file_browser.drain_requests() {
                match request {
                    FileBrowserRequest::LoadLocations => self.dispatch_browser_request(
                        FilesClientMessage::ListBrowserLocations,
                        None,
                        RequestKind::Locations,
                    ),
                    FileBrowserRequest::ListDirectory { path } => self
                        .dispatch_browser_request(
                            FilesClientMessage::BrowserListDir { path: path.clone() },
                            Some(path),
                            RequestKind::Listing,
                        ),
                }
            }
        } else {
            self.renderer
                .file_browser
                .fulfill_requests_native(&target.root);
        }
        let Some(selection) = self.renderer.file_browser.take_selection() else {
            return;
        };
        // Modes alone do not distinguish workspace and agent cd.
        let expected_mode = if target.purpose == DesktopBrowserPurpose::AgentImage {
            FileBrowserMode::AttachImage
        } else {
            FileBrowserMode::ChooseDirectory
        };
        if selection.mode != expected_mode {
            return;
        }
        if target.joined {
            // Revalidate on the host after selection, not against an earlier listing.
            let kind = if target.purpose == DesktopBrowserPurpose::AgentImage {
                RequestKind::ImageStat
            } else {
                RequestKind::DirectoryStat
            };
            self.dispatch_browser_request(
                FilesClientMessage::BrowserStat {
                    path: selection.path.clone(),
                },
                Some(selection.path),
                kind,
            );
            return;
        }
        let path = match self
            .renderer
            .file_browser
            .resolve_native_selection(&selection.path)
        {
            Ok(path) => path,
            Err(error) => {
                self.browser_failure(error, true);
                return;
            }
        };
        let state_dir = neoism_backend::config::config_dir_path();
        if let Ok(bytes) = serde_json::to_vec(self.renderer.file_browser.recents()) {
            let _ = std::fs::create_dir_all(&state_dir);
            let _ = std::fs::write(state_dir.join("file-browser-recents.json"), bytes);
        }
        match target.purpose {
            DesktopBrowserPurpose::Workspace | DesktopBrowserPurpose::AddWorkspace | DesktopBrowserPurpose::AgentDirectory => {
                self.commit_browser_directory(path)
            }
            DesktopBrowserPurpose::AgentImage => {
                if !valid_image_size(std::fs::metadata(&path).ok().map(|meta| meta.len()))
                {
                    self.browser_failure(
                        "Choose an image smaller than 20 MB".into(),
                        true,
                    );
                    return;
                }
                if let Some(agent) =
                    self.context_manager.current_mut().neoism_agent.as_mut()
                {
                    agent.attach_path(&path);
                }
                self.desktop_browser_target = None;
            }
        }
        self.mark_dirty();
    }

    /// Consume correlated replies even when stale so they cannot feed another files-plane consumer.
    pub(crate) fn apply_desktop_browser_message(
        &mut self,
        id: u64,
        message: &FilesServerMessage,
    ) -> bool {
        if self.pending_remote_browser.get(&id).is_some_and(|pending| {
            !request_owns_connection(pending, self.context_manager.daemon_connection_key())
        }) {
            // IDs can be reused by another daemon. Do not swallow its tree/editor reply.
            self.pending_remote_browser.remove(&id);
            return false;
        }
        let Some(pending) = self.pending_remote_browser.remove(&id) else {
            return false;
        };
        if !request_applies(
            &pending,
            self.desktop_browser_generation,
            self.browser_target_is_current(),
            self.renderer.file_browser.is_active(),
            self.renderer.file_browser.current_path(),
        ) {
            return true;
        }
        let selection =
            !matches!(pending.kind, RequestKind::Locations | RequestKind::Listing);
        match message {
            FilesServerMessage::BrowserLocations { locations }
                if pending.kind == RequestKind::Locations =>
            {
                self.renderer.file_browser.set_locations(
                    locations
                        .iter()
                        .map(|location| FileBrowserLocation {
                            kind: match location.kind {
                                neoism_protocol::files::FileLocationKind::Workspace => {
                                    "workspace"
                                }
                                neoism_protocol::files::FileLocationKind::Home => "home",
                                neoism_protocol::files::FileLocationKind::Documents => {
                                    "documents"
                                }
                                neoism_protocol::files::FileLocationKind::Downloads => {
                                    "downloads"
                                }
                                neoism_protocol::files::FileLocationKind::Pictures => {
                                    "pictures"
                                }
                            }
                            .into(),
                            label: location.label.clone(),
                            path: location.path.clone(),
                        })
                        .collect(),
                );
            }
            FilesServerMessage::Stat { path, entry }
                if pending.path.as_ref() == Some(path)
                    && pending.kind == RequestKind::DirectoryStat =>
            {
                if entry.is_dir {
                    let canonical =
                        entry.host_path.clone().unwrap_or_else(|| path.clone());
                    self.dispatch_browser_request(
                        FilesClientMessage::BrowserListDir {
                            path: canonical.clone(),
                        },
                        Some(canonical),
                        RequestKind::DirectoryListing,
                    );
                } else {
                    self.browser_failure(
                        "The selected host location is not a folder".into(),
                        true,
                    );
                }
            }
            FilesServerMessage::Stat { path, entry }
                if pending.path.as_ref() == Some(path)
                    && pending.kind == RequestKind::ImageStat =>
            {
                if !entry.is_dir && valid_image_size(entry.size) {
                    let canonical =
                        entry.host_path.clone().unwrap_or_else(|| path.clone());
                    self.dispatch_browser_request(
                        FilesClientMessage::BrowserReadFile {
                            path: canonical.clone(),
                        },
                        Some(canonical),
                        RequestKind::ImageRead,
                    );
                } else {
                    self.browser_failure(
                        "Choose an image smaller than 20 MB".into(),
                        true,
                    );
                }
            }
            FilesServerMessage::DirListing { path, .. }
                if pending.kind == RequestKind::DirectoryListing
                    && pending.path.as_ref() == Some(path) =>
            {
                self.commit_browser_directory(PathBuf::from(path));
            }
            FilesServerMessage::DirListing { path, entries }
                if pending.kind == RequestKind::Listing
                    && pending.path.as_ref() == Some(path) =>
            {
                self.renderer.file_browser.set_listing(
                    path,
                    entries
                        .iter()
                        .map(|entry| FileBrowserEntry {
                            name: entry.name.clone(),
                            is_dir: entry.is_dir,
                            size: entry.size,
                        })
                        .collect(),
                );
            }
            FilesServerMessage::FileContent { path, bytes }
                if pending.kind == RequestKind::ImageRead
                    && pending.path.as_ref() == Some(path) =>
            {
                if !valid_image_size(Some(bytes.len() as u64)) {
                    self.browser_failure(
                        "Choose an image smaller than 20 MB".into(),
                        true,
                    );
                    return true;
                }
                let mime = neoism_ui::panels::agent_pane::input_controller::mime_for_path(
                    std::path::Path::new(path),
                );
                if !mime.starts_with("image/") {
                    self.browser_failure(
                        "That host file is not a supported image".into(),
                        true,
                    );
                    return true;
                }
                if let Some(agent) =
                    self.context_manager.current_mut().neoism_agent.as_mut()
                {
                    agent.attach_clipboard_image(
                        neoism_backend::clipboard::ClipboardImage {
                            bytes: bytes.clone(),
                            mime: mime.into(),
                            filename: if neoism_protocol::host_path::HostPath::new(path)
                                .is_windows()
                            {
                                path.rsplit(['/', '\\'])
                                    .next()
                                    .unwrap_or("image")
                                    .to_string()
                            } else {
                                path.rsplit('/').next().unwrap_or("image").to_string()
                            },
                        },
                    );
                }
                self.desktop_browser_target = None;
            }
            FilesServerMessage::Error { message } => {
                self.browser_failure(message.clone(), selection)
            }
            _ => self.browser_failure(
                "Unexpected host file-browser response".into(),
                selection,
            ),
        }
        self.pump_agent_image_browser();
        self.mark_dirty();
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn old_navigation_success_and_error_are_both_rejected() {
        let pending = RemoteBrowserRequest {
            connection_key: None,
            generation: 3,
            path: Some("/host/old".into()),
            kind: RequestKind::Listing,
        };
        assert!(!request_applies(&pending, 3, true, true, "/host/new"));
        assert!(request_applies(&pending, 3, true, true, "/host/old"));
        assert!(!request_applies(&pending, 3, true, false, "/host/old"));
    }

    #[test]
    fn closed_modal_selection_requires_original_generation_and_identity() {
        for kind in [
            RequestKind::DirectoryStat,
            RequestKind::DirectoryListing,
            RequestKind::ImageStat,
            RequestKind::ImageRead,
        ] {
            let pending = RemoteBrowserRequest {
                connection_key: None,
                generation: 3,
                path: Some("/host/selected".into()),
                kind,
            };
            assert!(request_applies(&pending, 3, true, false, "/host"));
            assert!(!request_applies(&pending, 4, true, false, "/host"));
            assert!(!request_applies(&pending, 3, false, false, "/host"));
        }
    }

    #[test]
    fn another_connection_cannot_steal_a_reused_request_id() {
        let pending = RemoteBrowserRequest {
            connection_key: Some(11), generation: 1, path: None, kind: RequestKind::Locations,
        };
        assert!(request_owns_connection(&pending, Some(11)));
        assert!(!request_owns_connection(&pending, Some(22)));
        assert!(!request_owns_connection(&pending, None));
    }

    #[test]
    fn image_size_is_checked_before_and_after_remote_read() {
        assert!(!valid_image_size(None));
        assert!(!valid_image_size(Some(0)));
        assert!(valid_image_size(Some(MAX_BROWSER_IMAGE_BYTES)));
        assert!(!valid_image_size(Some(MAX_BROWSER_IMAGE_BYTES + 1)));
    }
}
