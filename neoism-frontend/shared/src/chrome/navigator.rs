//! Host-neutral routing. IO and workspace activation remain with the host.
use super::Chrome;
use crate::event::{KeyState, LogicalKey, Modifiers, NamedKey, PointerButton, UiEvent};
use crate::panels::agent_pane::state::side_panel::CatalogHeaderAction;
use crate::widgets::conversation_navigator::ConversationNavigatorAction as Action;

impl<A: Send + Copy + 'static> Chrome<A> {
    /// Bind the dashboard permission to the authorized agent/catalog identity.
    /// Re-rooting and reconnecting the same identity must not discard it.
    pub fn bind_conversation_dashboard_endpoint(&mut self, endpoint: &str) {
        if self.conversation_dashboard_endpoint.as_deref() != Some(endpoint) {
            self.conversation_dashboard_endpoint = Some(endpoint.to_owned());
            self.conversation_dashboard_root = None;
            self.refresh_conversation_catalog_scope();
        }
    }

    /// Only hosts with an authorized FilesLocations response may supply home.
    pub fn set_conversation_dashboard_root(&mut self, root: Option<String>) {
        let root = root
            .filter(|root| !root.trim().is_empty())
            .map(std::path::PathBuf::from);
        self.conversation_dashboard_root = root;
        // Replay for a lazily installed/replaced pane too. The pane's scope
        // setter itself is idempotent and does not refetch unchanged scopes.
        self.refresh_conversation_catalog_scope();
    }

    pub fn conversation_catalog_directory(
        &self,
        root: &std::path::Path,
    ) -> Option<String> {
        let directory = root.to_string_lossy();
        let bytes = directory.as_bytes();
        let filesystem_root = directory == "/"
            || (bytes.len() == 3
                && bytes[0].is_ascii_alphabetic()
                && bytes[1] == b':'
                && matches!(bytes[2], b'/' | b'\\'));
        let dashboard = self.conversation_dashboard_root.as_deref() == Some(root);
        (!filesystem_root && !dashboard).then(|| directory.into_owned())
    }

    /// Used by WASM's real set_workspace_root path, not the chat execution cwd.
    pub fn set_conversation_catalog_workspace_root(&mut self, root: &std::path::Path) {
        let directory = self.conversation_catalog_directory(root);
        if let Some(pane) = self.agent_pane.as_mut() {
            pane.side_panel_mut().set_dashboard_directory(
                self.conversation_dashboard_root
                    .as_ref()
                    .map(|root| root.to_string_lossy().into_owned()),
            );
            pane.set_catalog_directory(directory);
        }
    }

    fn refresh_conversation_catalog_scope(&mut self) {
        if let Some(pane) = self.agent_pane.as_mut() {
            pane.side_panel_mut().set_dashboard_directory(
                self.conversation_dashboard_root
                    .as_ref()
                    .map(|root| root.to_string_lossy().into_owned()),
            );
        }
        if let Some(root) = self.workspace_root_path.clone() {
            self.set_conversation_catalog_workspace_root(&root);
        }
    }

    pub fn take_catalog_header_action(&mut self) -> Option<CatalogHeaderAction> {
        self.pending_catalog_header.take().filter(|action| {
            *action != CatalogHeaderAction::AddWorkspace
                || self
                    .agent_pane
                    .as_ref()
                    .is_some_and(|pane| pane.side_panel().header_action_enabled(*action))
        })
    }

    pub fn take_navigator_action(&mut self) -> Option<Action> {
        self.pending_navigator_action.take()
    }

    pub fn apply_navigator_action(&mut self, action: Action) {
        // Activation stays live until the host validates its query and endpoint.
        // Closing here would erase the query before the browser can check it.
        if matches!(action, Action::Dismiss) {
            self.conversation_navigator.close();
        }
        self.pending_navigator_action = Some(action);
    }

    pub(crate) fn handle_conversation_navigator_event(
        &mut self,
        event: &UiEvent,
    ) -> bool {
        if !self.conversation_navigator.is_active()
            || !matches!(
                event,
                UiEvent::Key(_)
                    | UiEvent::Text(_)
                    | UiEvent::Composition(_)
                    | UiEvent::PointerDown { .. }
                    | UiEvent::PointerUp { .. }
                    | UiEvent::PointerMove { .. }
                    | UiEvent::PointerLeave
                    | UiEvent::Wheel { .. }
            )
        {
            return false;
        }
        let mut action = None;
        match event {
            UiEvent::PointerDown {
                x,
                y,
                button: PointerButton::Left,
                ..
            } => {
                action = self.conversation_navigator.pointer_down(*x, *y);
            }
            UiEvent::PointerMove { x, y, .. } => {
                self.conversation_navigator.pointer_move(*x, *y);
            }
            UiEvent::Wheel { dy, mode, .. } => {
                let delta = if matches!(mode, crate::event::WheelMode::Line) {
                    *dy * 32.0
                } else {
                    *dy
                };
                self.conversation_navigator.scroll(delta);
            }
            UiEvent::Text(text) => self.conversation_navigator.push_text(text),
            UiEvent::Key(key) if key.state == KeyState::Pressed => match &key.logical {
                LogicalKey::Named(NamedKey::Escape) => action = Some(Action::Dismiss),
                LogicalKey::Named(NamedKey::Enter) => {
                    action = self
                        .conversation_navigator
                        .selected_value()
                        .map(|value| Action::Activate(value.to_owned()));
                }
                LogicalKey::Named(NamedKey::ArrowDown) => {
                    self.conversation_navigator.select_next()
                }
                LogicalKey::Named(NamedKey::ArrowUp) => {
                    self.conversation_navigator.select_previous()
                }
                LogicalKey::Named(NamedKey::Backspace) => {
                    self.conversation_navigator.pop_char()
                }
                LogicalKey::Character(text)
                    if !key.modifiers.intersects(
                        Modifiers::CTRL | Modifiers::ALT | Modifiers::META,
                    ) =>
                {
                    self.conversation_navigator.push_text(text);
                }
                _ => {}
            },
            _ => {}
        }
        if let Some(action) = action {
            self.apply_navigator_action(action);
        }
        // Even unhandled modal input must not reach the terminal/composer.
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::event::{KeyDescriptor, PhysicalKey};
    use crate::widgets::conversation_navigator::{
        ConversationNavigatorMode as Mode, ConversationNavigatorRow as Row,
    };

    fn key(named: NamedKey) -> UiEvent {
        UiEvent::Key(KeyDescriptor {
            physical: PhysicalKey(0),
            logical: LogicalKey::Named(named),
            state: KeyState::Pressed,
            modifiers: Modifiers::empty(),
            repeat: false,
        })
    }

    #[test]
    fn modal_keyboard_preserves_query_until_host_validates_activation() {
        let mut chrome = Chrome::<()>::new();
        chrome.conversation_navigator.open(
            Mode::Search,
            vec![Row {
                value: "outside-catalog".into(),
                title: "Result".into(),
                ..Default::default()
            }],
        );
        chrome.conversation_navigator.set_query("needle".into());
        assert!(chrome.handle_conversation_navigator_event(&key(NamedKey::Enter)));
        assert!(chrome.conversation_navigator.is_active());
        assert_eq!(chrome.conversation_navigator.query(), "needle");
        assert_eq!(
            chrome.take_navigator_action(),
            Some(Action::Activate("outside-catalog".into()))
        );
        assert_eq!(chrome.take_navigator_action(), None);
    }

    #[test]
    fn dashboard_metadata_reaches_sorter_and_endpoint_reset_clears_priority() {
        use crate::panels::agent_pane::state::{
            side_panel::NeoismAgentSessionEntry, NeoismAgentPane,
        };
        let mut chrome = Chrome::<()>::new();
        chrome.bind_conversation_dashboard_endpoint("authorized-a");
        chrome.set_conversation_dashboard_root(Some("/host/home".into()));
        // Metadata can precede pane installation and workspaceRoot synchronization.
        chrome.install_agent_pane(NeoismAgentPane::default());
        chrome.set_workspace_root_path(Some("/".into()));
        chrome.set_conversation_catalog_workspace_root(std::path::Path::new("/"));
        let rows = || {
            vec![
                NeoismAgentSessionEntry::new("old", "old", "")
                    .with_directory(Some("/host/home".into()))
                    .with_updated_ms(1),
                NeoismAgentSessionEntry::new("new", "new", "")
                    .with_directory(Some("/new".into()))
                    .with_updated_ms(10),
            ]
        };
        chrome
            .agent_pane_mut()
            .unwrap()
            .side_panel_mut()
            .set_sessions(rows());
        assert_eq!(
            chrome.agent_pane().unwrap().side_panel().sessions()[0]
                .directory
                .as_deref(),
            Some("/host/home")
        );
        chrome.bind_conversation_dashboard_endpoint("authorized-a");
        assert_eq!(
            chrome.agent_pane().unwrap().side_panel().sessions()[0].title,
            "~"
        );
        chrome.bind_conversation_dashboard_endpoint("authorized-b");
        assert_eq!(
            chrome.agent_pane().unwrap().side_panel().sessions()[0]
                .directory
                .as_deref(),
            Some("/new")
        );
        chrome.set_conversation_dashboard_root(Some("/new".into()));
        assert_eq!(
            chrome.agent_pane().unwrap().side_panel().sessions()[0].title,
            "~"
        );
        chrome.set_conversation_dashboard_root(None);
        assert_ne!(
            chrome.agent_pane().unwrap().side_panel().sessions()[0].title,
            "~"
        );
    }

    #[test]
    fn advertised_home_is_global_on_the_workspace_root_setter_scope_path() {
        use crate::panels::agent_pane::state::NeoismAgentPane;
        use std::path::Path;
        let mut chrome = Chrome::<()>::new();
        chrome.install_agent_pane(NeoismAgentPane::default());
        chrome.bind_conversation_dashboard_endpoint("authorized-a");
        chrome.set_workspace_root_path(Some("/home/alice".into()));
        chrome.set_conversation_catalog_workspace_root(Path::new("/home/alice"));
        assert_eq!(
            chrome
                .agent_pane()
                .unwrap()
                .side_panel()
                .catalog_directory(),
            Some("/home/alice")
        );
        // Late authorized FilesLocations arrival fixes the INITIAL home view.
        chrome.set_conversation_dashboard_root(Some("/home/alice".into()));
        assert_eq!(
            chrome
                .agent_pane()
                .unwrap()
                .side_panel()
                .catalog_directory(),
            None
        );
        // This is the scope method called by WASM's actual set_workspace_root.
        chrome.set_conversation_catalog_workspace_root(Path::new("/work/project-a"));
        assert_eq!(
            chrome
                .agent_pane()
                .unwrap()
                .side_panel()
                .catalog_directory(),
            Some("/work/project-a")
        );
        chrome.set_conversation_catalog_workspace_root(Path::new("/home/alice"));
        assert_eq!(
            chrome
                .agent_pane()
                .unwrap()
                .side_panel()
                .catalog_directory(),
            None
        );
    }

    #[test]
    fn dashboard_permission_survives_same_endpoint_but_not_auth_changes() {
        use std::path::Path;
        let mut chrome = Chrome::<()>::new();
        chrome.bind_conversation_dashboard_endpoint("authorized-a");
        chrome.set_conversation_dashboard_root(Some("/home/alice".into()));
        chrome.bind_conversation_dashboard_endpoint("authorized-a");
        assert_eq!(
            chrome.conversation_catalog_directory(Path::new("/home/alice")),
            None
        );
        chrome.bind_conversation_dashboard_endpoint("authorized-b");
        assert_eq!(
            chrome.conversation_catalog_directory(Path::new("/home/alice")),
            Some("/home/alice".into())
        );
        assert_eq!(chrome.conversation_catalog_directory(Path::new("/")), None);
        assert_eq!(
            chrome.conversation_catalog_directory(Path::new("C:\\")),
            None
        );
    }

    #[test]
    fn queued_add_workspace_is_rejected_after_leaving_dashboard() {
        use crate::panels::agent_pane::state::NeoismAgentPane;
        let mut chrome = Chrome::<()>::new();
        let mut pane = NeoismAgentPane::default();
        pane.side_panel_mut().set_navigation_header_enabled(true);
        chrome.install_agent_pane(pane);
        chrome.pending_catalog_header = Some(CatalogHeaderAction::AddWorkspace);
        chrome
            .agent_pane_mut()
            .unwrap()
            .set_catalog_directory(Some("/work/project".into()));
        assert_eq!(chrome.take_catalog_header_action(), None);
        chrome.pending_catalog_header = Some(CatalogHeaderAction::Projects);
        assert_eq!(
            chrome.take_catalog_header_action(),
            Some(CatalogHeaderAction::Projects)
        );
        chrome.agent_pane_mut().unwrap().set_catalog_directory(None);
        chrome.pending_catalog_header = Some(CatalogHeaderAction::AddWorkspace);
        assert_eq!(
            chrome.take_catalog_header_action(),
            Some(CatalogHeaderAction::AddWorkspace)
        );
    }

    #[test]
    fn sidebar_enter_on_projects_queues_the_same_picker_action_as_click() {
        use crate::panels::agent_pane::state::NeoismAgentPane;
        for scope in [None, Some("/work/project".to_owned())] {
            let mut chrome = Chrome::<()>::new();
            let mut pane = NeoismAgentPane::default();
            pane.side_panel_mut().set_navigation_header_enabled(true);
            pane.set_catalog_directory(scope);
            pane.side_panel_mut().set_focused(true);
            pane.side_panel_mut().select_new_chat();
            chrome.install_agent_pane(pane);
            chrome.conversations_visible = true;
            assert!(chrome.handle_chrome_key_shortcut(&key(NamedKey::ArrowDown)));
            assert_eq!(chrome.agent_pane().unwrap().side_panel().selected_header_action(), Some(CatalogHeaderAction::Search));
            assert!(!chrome.agent_pane().unwrap().side_panel().search_focused());
            assert!(chrome.handle_chrome_key_shortcut(&key(NamedKey::Enter)));
            assert_eq!(chrome.take_catalog_header_action(), Some(CatalogHeaderAction::Search));
            assert!(chrome.pending_conversation_open.is_none());
            assert!(chrome.handle_chrome_key_shortcut(&key(NamedKey::ArrowDown)));
            assert!(chrome
                .agent_pane()
                .unwrap()
                .side_panel()
                .projects_selected());
            assert!(chrome.handle_chrome_key_shortcut(&key(NamedKey::Enter)));
            assert_eq!(
                chrome.take_catalog_header_action(),
                Some(CatalogHeaderAction::Projects)
            );
            assert!(chrome.pending_conversation_open.is_none());
            assert!(chrome.handle_chrome_key_shortcut(&key(NamedKey::ArrowUp)));
            assert_eq!(chrome.agent_pane().unwrap().side_panel().selected_header_action(), Some(CatalogHeaderAction::Search));
            assert!(chrome.handle_chrome_key_shortcut(&key(NamedKey::ArrowUp)));
            assert!(chrome
                .agent_pane()
                .unwrap()
                .side_panel()
                .new_chat_selected());
        }
    }

    #[test]
    fn active_navigator_owns_animation_redraw() {
        let mut chrome = Chrome::<()>::new();
        chrome.conversation_navigator.open(Mode::Search, vec![]);
        assert!(chrome.animations_active());
    }

    #[test]
    fn escape_cancels_and_non_input_events_still_flow() {
        let mut chrome = Chrome::<()>::new();
        chrome.conversation_navigator.open(Mode::Projects, vec![]);
        assert!(chrome.generic_keyboard_overlay_active());
        assert!(!chrome.handle_conversation_navigator_event(&UiEvent::Focus(true)));
        assert!(chrome.handle_conversation_navigator_event(&key(NamedKey::Escape)));
        assert_eq!(chrome.take_navigator_action(), Some(Action::Dismiss));
        assert!(!chrome.handle_conversation_navigator_event(&key(NamedKey::Enter)));
    }
}
