//! Independent, bounded project paging. Tokens are never reused, including across
//! discovery refreshes and scope changes; global catalog generations are unrelated.
use super::*;

#[cfg(test)]
mod tests;

#[cfg(test)]
mod loading_query_tests {
    use super::*;

    #[test]
    fn failed_pages_retry_with_backoff_then_require_explicit_action() {
        let mut panel = NeoismAgentSidePanel::default();
        panel.set_session_projects(vec![("/pending".into(), 1)]);
        for failure in 1..=3 {
            let requests = panel.begin_project_session_requests();
            assert_eq!(requests.len(), 1);
            let (directory, cursor, token) = &requests[0];
            assert!(cursor.is_none());
            panel.settle_project_session_error(directory, *token, "offline".into());
            assert!(panel.begin_project_session_requests().is_empty());
            let page = panel.project_pages.get_mut("/pending").unwrap();
            page.last_failure = Some(Instant::now() - std::time::Duration::from_secs(10));
            assert_eq!(page.retry_due(Instant::now()), failure < 3);
        }
        assert!(panel.begin_project_session_requests().is_empty());
        panel.selected = panel
            .sessions
            .iter()
            .position(|e| e.is_project_page())
            .unwrap();
        assert!(panel.activate_selected_project_page());
        let requests = panel.begin_project_session_requests();
        assert_eq!(requests.len(), 1);
        panel.set_project_session_page(
            "/pending",
            None,
            vec![],
            Some("next".into()),
            requests[0].2,
        );
        let page = panel.project_pages.get("/pending").unwrap();
        assert_eq!(page.failures, 0);
        assert!(page.last_failure.is_none());
        // Successful Show more pages must not be automatically drained.
        assert!(panel.begin_project_session_requests().is_empty());
    }

    #[test]
    fn project_loading_query_tracks_state_not_placeholder_title() {
        let mut panel = NeoismAgentSidePanel::default();
        panel.set_session_projects(vec![("/pending".into(), 1)]);
        assert!(panel.project_page_loading("/pending"));
        assert!(!panel.project_page_loading("/unknown"));
        for loaded in [false, true] {
            for queued in [false, true] {
                for requested in [false, true] {
                    for failed in [false, true] {
                        let page = panel.project_pages.get_mut("/pending").unwrap();
                        page.loaded = loaded;
                        page.queued = queued;
                        page.request = requested.then(|| (Some("cursor".into()), 1));
                        page.error = failed.then(|| "offline".into());
                        page.next_cursor = Some("next".into());
                        assert_eq!(
                            panel.project_page_loading("/pending"),
                            requested || queued || (!loaded && !failed),
                            "loaded={loaded}, queued={queued}, requested={requested}, failed={failed}"
                        );
                    }
                }
            }
        }
        let page = panel.project_pages.get_mut("/pending").unwrap();
        page.loaded = true;
        page.queued = false;
        page.request = None;
        page.error = None;
        assert!(!panel.project_page_loading("/pending"));
        assert_eq!(
            panel.project_page_row("/pending").unwrap().title,
            "Show more"
        );
        panel.project_pages.get_mut("/pending").unwrap().error = Some("offline".into());
        assert!(!panel.project_page_loading("/pending"));
        assert!(panel
            .project_page_row("/pending")
            .unwrap()
            .title
            .starts_with("Retry"));
        let page = panel.project_pages.get_mut("/pending").unwrap();
        page.error = None;
        page.next_cursor = None;
        assert!(!panel.project_page_loading("/pending"));
        assert!(panel.project_page_row("/pending").is_none());
    }

    #[test]
    fn pending_page_hit_is_not_an_action_even_with_a_loaded_cursor() {
        let mut panel = NeoismAgentSidePanel::default();
        panel.set_session_projects(vec![("/pending".into(), 1)]);
        // Hit the actual placeholder with the renderer's unchanged row stride.
        let row = panel
            .sessions
            .iter()
            .position(|row| row.is_project_page())
            .unwrap();
        panel.set_last_panel_rect([0.0, 0.0, 300.0, 1000.0]);
        panel.set_row_hit_rect([0.0, 0.0, 300.0, 1000.0], 40.0);
        let y = row as f32 * 40.0 + 20.0;
        assert!(!panel.project_page_action_at(20.0, y));
        let page = panel.project_pages.get_mut("/pending").unwrap();
        page.loaded = true;
        page.next_cursor = Some("next".into());
        assert!(panel.project_page_action_at(20.0, y));
        panel.project_pages.get_mut("/pending").unwrap().queued = true;
        assert!(!panel.project_page_action_at(20.0, y));
        let page = panel.project_pages.get_mut("/pending").unwrap();
        page.queued = false;
        page.request = Some((Some("next".into()), 1));
        assert!(!panel.project_page_action_at(20.0, y));
        let page = panel.project_pages.get_mut("/pending").unwrap();
        page.request = None;
        page.error = Some("offline".into());
        assert!(panel.project_page_action_at(20.0, y));
    }
}

#[derive(Default)]
pub(super) struct ProjectSessionPage {
    pub updated_ms: u64,
    pub loaded: bool,
    pub next_cursor: Option<String>,
    pub request: Option<(Option<String>, u64)>,
    pub error: Option<String>,
    pub queued: bool,
    failures: u8,
    last_failure: Option<Instant>,
}

impl ProjectSessionPage {
    fn retry_due(&self, now: Instant) -> bool {
        self.error.is_some()
            && self.failures < 3
            && self.last_failure.is_some_and(|last| {
                now.saturating_duration_since(last).as_secs_f32()
                    >= 0.75 * 2.0_f32.powi(i32::from(self.failures.saturating_sub(1)))
            })
    }
}

impl NeoismAgentSidePanel {
    /// Display names are separate from opaque backend-qualified project identity.
    pub fn set_project_labels(&mut self, labels: HashMap<String, String>) {
        self.project_labels = labels;
    }

    pub fn set_pinned_sessions(&mut self, sessions: Vec<NeoismAgentSessionEntry>) {
        if self.catalog_directory.is_some() {
            return;
        }
        let sessions: Vec<_> = sessions
            .into_iter()
            .filter(|entry| {
                entry.pinned
                    && !entry.is_header
                    && !entry.is_excerpt
                    && !self.deleted_session_ids.contains(&entry.id)
            })
            .collect();
        let mut versions: HashMap<_, _> = sessions
            .iter()
            .map(|entry| (entry.id.clone(), entry.updated_ms))
            .collect();
        let selected = self
            .selected_row()
            .map(|row| row.stable_identity().to_owned());
        let anchor = self.session_viewport_anchor();
        self.all_sessions.retain(|entry| {
            let Some(previous) = self.discovered_pinned_versions.get(&entry.id) else {
                return true;
            };
            if versions.contains_key(&entry.id) || !entry.pinned {
                return true;
            }
            // A live pin after the prior snapshot must survive a delayed reply.
            if entry.updated_ms > *previous {
                versions.insert(entry.id.clone(), entry.updated_ms);
                return true;
            }
            false
        });
        self.discovered_pinned_versions = versions;
        let sessions: Vec<_> = sessions
            .into_iter()
            .filter_map(|entry| {
                match self.all_sessions.iter().find(|old| old.id == entry.id) {
                    Some(old) if old.updated_ms > entry.updated_ms => {
                        old.pinned.then(|| old.clone())
                    }
                    _ => Some(entry),
                }
            })
            .collect();
        if sessions.is_empty() {
            self.rebuild_session_display();
            self.restore_selected_session(selected.as_deref());
            self.restore_session_viewport_anchor(anchor);
        } else {
            self.upsert_sessions(sessions);
        }
    }

    /// A page placeholder is pending before its first claim as well as while
    /// queued/in flight. Errors and idle loaded cursors remain actionable rows.
    pub fn project_page_loading(&self, directory: &str) -> bool {
        self.project_pages.get(directory).is_some_and(|page| {
            page.request.is_some()
                || page.queued
                || (!page.loaded && page.error.is_none())
        })
    }

    pub fn project_pagination_enabled(&self) -> bool {
        self.project_pagination && self.catalog_directory.is_none()
    }

    /// Discovery is authoritative, not a count of the currently loaded chats.
    /// Rediscovery preserves same-project claims, history, cursors, selection,
    /// collapse preferences, and the viewport. Removed-project workers remain
    /// counted until they settle, but their replies can no longer admit chats.
    pub fn set_session_projects(&mut self, projects: Vec<(String, u64)>) {
        if self.catalog_directory.is_some() {
            return;
        }
        let selected = self
            .selected_row()
            .filter(|_| {
                matches!(self.mode, SidePanelMode::Sessions)
                    && !self.search_focused()
                    && !self.new_chat_selected()
                    && self.selected_header_action().is_none()
            })
            .map(|e| e.stable_identity().to_owned());
        let anchor = if matches!(self.mode, SidePanelMode::Sessions) {
            self.session_viewport_anchor()
        } else {
            None
        };
        let mut pages: HashMap<String, ProjectSessionPage> = HashMap::new();
        for (directory, updated_ms) in projects {
            let mut page = pages
                .remove(&directory)
                .or_else(|| self.project_pages.remove(&directory))
                .unwrap_or_default();
            page.updated_ms = updated_ms;
            pages.insert(directory, page);
        }
        for page in self.project_pages.values() {
            if let Some((_, token)) = &page.request {
                self.retired_project_requests.insert(*token);
            }
        }
        self.all_sessions.retain(|entry| {
            pages.contains_key(entry.directory.as_deref().unwrap_or_default())
        });
        self.project_pages = pages;
        self.project_pagination = true;
        self.session_page_loading = false;
        self.session_requested_cursor = None;
        self.session_next_cursor = None;
        self.session_catalog_state = SessionCatalogState::Ready;
        self.rebuild_session_display();
        self.restore_selected_session(selected.as_deref());
        self.restore_session_viewport_anchor(anchor);
    }

    /// Claim at most three requests in total. Automatic first pages are limited
    /// to expanded groups intersecting the viewport (+ two rows of overscan).
    /// Explicit Show more/retry actions are queued until a slot is available.
    /// Hosts must request five entries for each returned tuple.
    pub fn begin_project_session_requests(
        &mut self,
    ) -> Vec<(String, Option<String>, u64)> {
        if !self.project_pagination_enabled()
            || self.user_hidden
            || !matches!(self.mode, SidePanelMode::Sessions)
            || !self.session_query.trim().is_empty()
        {
            return Vec::new();
        }
        let busy = self
            .project_pages
            .values()
            .filter(|p| p.request.is_some())
            .count();
        let mut slots = 3usize.saturating_sub(busy + self.retired_project_requests.len());
        let start = self.scroll_top.saturating_sub(2);
        let end = self
            .scroll_top
            .saturating_add(self.last_panel_height_rows.max(1) + 2);
        let mut candidates = Vec::new();
        // Visible groups first, so old offscreen work cannot starve new ones.
        for (index, entry) in self.sessions.iter().enumerate() {
            if !entry.is_project() {
                continue;
            }
            let directory = entry.directory.as_ref().unwrap();
            if !self.project_expanded(directory) {
                continue;
            }
            let group_end = self.sessions[index + 1..]
                .iter()
                .position(|e| e.is_project())
                .map_or(self.sessions.len(), |n| index + 1 + n);
            if index <= end && group_end > start {
                candidates.push(directory.clone());
            }
        }
        let mut queued: Vec<_> = self
            .project_pages
            .iter()
            .filter(|(dir, p)| p.queued && self.project_expanded(dir))
            .map(|(dir, _)| dir.clone())
            .collect();
        queued.sort();
        for directory in queued {
            if !candidates.contains(&directory) {
                candidates.push(directory);
            }
        }
        let now = Instant::now();
        let mut requests = Vec::new();
        for directory in candidates {
            if slots == 0 {
                break;
            }
            let Some(page) = self.project_pages.get_mut(&directory) else {
                continue;
            };
            if page.request.is_some()
                || (!page.queued
                    && !page.retry_due(now)
                    && (page.loaded || page.error.is_some()))
            {
                continue;
            }
            self.project_generation = self
                .project_generation
                .checked_add(1)
                .expect("project request generation exhausted");
            let cursor = if page.loaded {
                page.next_cursor.clone()
            } else {
                None
            };
            page.request = Some((cursor.clone(), self.project_generation));
            page.error = None;
            page.queued = false;
            requests.push((directory, cursor, self.project_generation));
            slots -= 1;
        }
        if !requests.is_empty() {
            self.rebuild_session_display();
        }
        requests
    }

    pub fn set_project_session_page(
        &mut self,
        directory: &str,
        requested_cursor: Option<&str>,
        mut sessions: Vec<NeoismAgentSessionEntry>,
        next_cursor: Option<String>,
        generation: u64,
    ) {
        if self.retired_project_requests.remove(&generation) {
            return;
        }
        if !self.project_pagination_enabled() {
            return;
        }
        let Some(page) = self.project_pages.get(directory) else {
            return;
        };
        if !page.request.as_ref().is_some_and(|(cursor, token)| {
            *token == generation && cursor.as_deref() == requested_cursor
        }) {
            return;
        }
        let selected = self
            .selected_row()
            .filter(|_| {
                matches!(self.mode, SidePanelMode::Sessions)
                    && !self.search_focused()
                    && !self.new_chat_selected()
                    && self.selected_header_action().is_none()
            })
            .map(|e| e.stable_identity().to_owned());
        let anchor = if matches!(self.mode, SidePanelMode::Sessions) {
            self.session_viewport_anchor()
        } else {
            None
        };
        sessions.retain(|e| {
            !e.is_header
                && e.directory.as_deref().unwrap_or_default() == directory
                && !self.deleted_session_ids.contains(&e.id)
        });
        for entry in &mut sessions {
            self.search_only_session_ids.remove(&entry.id);
            self.apply_session_activity_override(entry);
        }
        // Merge by identity, preserving unrelated projects and live events.
        for mut entry in sessions {
            if let Some(old) = self.all_sessions.iter_mut().find(|e| e.id == entry.id) {
                if entry.updated_ms >= old.updated_ms {
                    if entry.runtime_status.is_none() {
                        entry.runtime_status.clone_from(&old.runtime_status);
                    }
                    if entry.catalog_activity.is_none() {
                        entry.catalog_activity = old.catalog_activity;
                    }
                    *old = entry;
                }
            } else {
                self.all_sessions.push(entry);
            }
        }
        let page = self.project_pages.get_mut(directory).unwrap();
        page.loaded = true;
        page.failures = 0;
        page.last_failure = None;
        // A broken server must not leave Show more repeatedly claiming the same cursor.
        page.next_cursor =
            next_cursor.filter(|cursor| Some(cursor.as_str()) != requested_cursor);
        page.request = None;
        page.error = None;
        self.rebuild_session_display();
        self.restore_selected_session(selected.as_deref());
        self.restore_session_viewport_anchor(anchor);
    }

    pub fn settle_project_session_error(
        &mut self,
        directory: &str,
        generation: u64,
        message: String,
    ) {
        if self.retired_project_requests.remove(&generation) {
            return;
        }
        if !self.project_pagination_enabled() {
            return;
        }
        let Some(page) = self.project_pages.get_mut(directory) else {
            return;
        };
        if !page
            .request
            .as_ref()
            .is_some_and(|(_, token)| *token == generation)
        {
            return;
        }
        page.request = None;
        page.error = Some(message);
        page.failures = page.failures.saturating_add(1);
        page.last_failure = Some(Instant::now());
        page.queued = false;
        self.rebuild_session_display();
    }

    pub fn project_page_action_at(&self, x: f32, y: f32) -> bool {
        if !matches!(self.mode, SidePanelMode::Sessions) {
            return false;
        }
        self.last_panel_rect()
            .and_then(|rect| self.hit_test_row(x, y, rect))
            .and_then(|row| self.sessions.get(row))
            .filter(|entry| entry.is_project_page())
            .and_then(|entry| entry.directory.as_ref())
            .and_then(|directory| self.project_pages.get(directory))
            .is_some_and(|page| {
                page.request.is_none()
                    && !page.queued
                    && (page.error.is_some() || page.next_cursor.is_some())
            })
    }

    /// Called by toggle_selected_project, so every existing mouse/keyboard
    /// activation intercept handles paging without opening a pseudo-session.
    pub fn activate_selected_project_page(&mut self) -> bool {
        if !self.project_pagination_enabled()
            || self.search_focused()
            || self.new_chat_selected()
            || self.selected_header_action().is_some()
        {
            return false;
        }
        let Some(entry) = self
            .sessions
            .get(self.selected)
            .filter(|e| e.is_project_page())
        else {
            return false;
        };
        let directory = entry.directory.clone().unwrap();
        if let Some(page) = self.project_pages.get_mut(&directory) {
            if page.request.is_none()
                && (page.error.is_some() || page.next_cursor.is_some())
            {
                page.failures = 0;
                page.last_failure = None;
                page.queued = true;
            }
        }
        self.rebuild_session_display();
        true
    }

    pub(super) fn project_page_row(
        &self,
        directory: &str,
    ) -> Option<NeoismAgentSessionEntry> {
        let page = self.project_pages.get(directory)?;
        let title = if page.request.is_some() || page.queued {
            "Loading…".to_string()
        } else if let Some(error) = &page.error {
            format!("Retry — {error}")
        } else if !page.loaded {
            "Loading…".to_string()
        } else if page.next_cursor.is_some() {
            "Show more".to_string()
        } else {
            return None;
        };
        let mut row = NeoismAgentSessionEntry::header(title)
            .with_directory(Some(directory.to_owned()))
            .with_depth(1);
        row.directory = Some(directory.to_owned());
        row.id = format!("project-page:{directory}");
        Some(row)
    }
}
