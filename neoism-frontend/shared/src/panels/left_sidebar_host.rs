#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum LeftSidebarView {
    Files,
    Notes,
    Conversations,
}

impl LeftSidebarView {
    pub const ALL: [Self; 3] = [Self::Files, Self::Notes, Self::Conversations];

    const fn index(self) -> usize {
        match self {
            Self::Files => 0,
            Self::Notes => 1,
            Self::Conversations => 2,
        }
    }
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum SidebarPlacement {
    #[default]
    Unified,
    Independent,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SidebarTransition {
    Show,
    Focus,
    Hide,
    Independent,
}

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct SidebarRequests {
    pub files: bool,
    pub notes: bool,
    pub conversations: bool,
}

impl SidebarRequests {
    pub const fn visible(self, view: LeftSidebarView) -> bool {
        match view {
            LeftSidebarView::Files => self.files,
            LeftSidebarView::Notes => self.notes,
            LeftSidebarView::Conversations => self.conversations,
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct LeftSidebarHost {
    active_unified: Option<LeftSidebarView>,
    focused: Option<LeftSidebarView>,
    unified_width: f32,
    placements: [SidebarPlacement; 3],
}

impl Default for LeftSidebarHost {
    fn default() -> Self {
        Self {
            active_unified: None,
            focused: None,
            unified_width: 300.0,
            placements: [SidebarPlacement::Unified; 3],
        }
    }
}

impl LeftSidebarHost {
    pub fn active_unified(&self) -> Option<LeftSidebarView> {
        self.active_unified
    }

    pub fn focused(&self) -> Option<LeftSidebarView> {
        self.focused
    }

    pub fn placement(&self, view: LeftSidebarView) -> SidebarPlacement {
        self.placements[view.index()]
    }

    pub fn set_placements(
        &mut self,
        files: SidebarPlacement,
        notes: SidebarPlacement,
        conversations: SidebarPlacement,
    ) {
        self.placements = [files, notes, conversations];
        if self
            .active_unified
            .is_some_and(|view| self.placement(view) != SidebarPlacement::Unified)
        {
            self.active_unified = None;
        }
    }

    pub fn set_placement(&mut self, view: LeftSidebarView, placement: SidebarPlacement) {
        self.placements[view.index()] = placement;
        if self.active_unified == Some(view) && placement != SidebarPlacement::Unified {
            self.active_unified = None;
        }
    }

    pub fn set_unified_width(&mut self, width: f32) {
        self.unified_width = width.clamp(120.0, 600.0);
    }

    pub fn unified_width(&self) -> f32 {
        self.unified_width
    }

    pub fn reconcile(&mut self, requests: SidebarRequests) {
        if self.active_unified.is_some_and(|view| {
            !requests.visible(view) || self.placement(view) != SidebarPlacement::Unified
        }) {
            self.active_unified = None;
        }
        if self.active_unified.is_none() {
            self.active_unified = LeftSidebarView::ALL.into_iter().find(|view| {
                requests.visible(*view)
                    && self.placement(*view) == SidebarPlacement::Unified
            });
        }
        if self
            .focused
            .is_some_and(|view| !self.is_resolved_visible(view, requests))
        {
            self.focused = None;
        }
    }

    pub fn toggle(
        &mut self,
        view: LeftSidebarView,
        currently_focused: bool,
    ) -> SidebarTransition {
        if self.placement(view) == SidebarPlacement::Independent {
            return SidebarTransition::Independent;
        }
        if self.active_unified == Some(view) {
            if currently_focused {
                self.active_unified = None;
                self.focused = None;
                SidebarTransition::Hide
            } else {
                self.focused = Some(view);
                SidebarTransition::Focus
            }
        } else {
            self.active_unified = Some(view);
            self.focused = Some(view);
            SidebarTransition::Show
        }
    }

    pub fn show(&mut self, view: LeftSidebarView, focus: bool) -> SidebarTransition {
        if self.placement(view) == SidebarPlacement::Independent {
            return SidebarTransition::Independent;
        }
        self.active_unified = Some(view);
        if focus {
            self.focused = Some(view);
        }
        SidebarTransition::Show
    }

    pub fn hide(&mut self, view: LeftSidebarView) {
        if self.active_unified == Some(view) {
            self.active_unified = None;
        }
        if self.focused == Some(view) {
            self.focused = None;
        }
    }

    pub fn set_focused(&mut self, view: Option<LeftSidebarView>) {
        self.focused = view;
    }

    pub fn is_resolved_visible(
        &self,
        view: LeftSidebarView,
        requests: SidebarRequests,
    ) -> bool {
        if self.placement(view) == SidebarPlacement::Unified {
            self.active_unified == Some(view) && requests.visible(view)
        } else {
            requests.visible(view)
        }
    }

    pub fn resolved_views(&self, requests: SidebarRequests) -> Vec<LeftSidebarView> {
        let mut resolved = Vec::with_capacity(4);
        if let Some(view) = self.active_unified.filter(|view| requests.visible(*view)) {
            resolved.push(view);
        }
        resolved.extend(LeftSidebarView::ALL.into_iter().filter(|view| {
            self.placement(*view) == SidebarPlacement::Independent
                && requests.visible(*view)
        }));
        resolved
    }

    pub fn resolved_width(&self, view: LeftSidebarView, natural_width: f32) -> f32 {
        if self.placement(view) == SidebarPlacement::Unified {
            self.unified_width
        } else {
            natural_width
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unified_views_switch_in_one_constant_width_slot() {
        let mut host = LeftSidebarHost::default();
        assert_eq!(
            host.toggle(LeftSidebarView::Files, false),
            SidebarTransition::Show
        );
        assert_eq!(host.active_unified(), Some(LeftSidebarView::Files));
        assert_eq!(
            host.toggle(LeftSidebarView::Notes, false),
            SidebarTransition::Show
        );
        assert_eq!(host.active_unified(), Some(LeftSidebarView::Notes));
        assert_eq!(host.resolved_width(LeftSidebarView::Notes, 440.0), 300.0);
    }

    #[test]
    fn focused_active_toggle_closes_but_unfocused_toggle_refocuses() {
        let mut host = LeftSidebarHost::default();
        host.show(LeftSidebarView::Files, false);
        assert_eq!(
            host.toggle(LeftSidebarView::Files, false),
            SidebarTransition::Focus
        );
        assert_eq!(
            host.toggle(LeftSidebarView::Files, true),
            SidebarTransition::Hide
        );
        assert_eq!(host.active_unified(), None);
    }

    #[test]
    fn mixed_placements_resolve_unified_then_independent_columns() {
        let mut host = LeftSidebarHost::default();
        host.set_placements(
            SidebarPlacement::Unified,
            SidebarPlacement::Independent,
            SidebarPlacement::Independent,
        );
        host.show(LeftSidebarView::Files, true);
        assert_eq!(
            host.resolved_views(SidebarRequests {
                files: true,
                notes: true,
                conversations: true
            }),
            vec![
                LeftSidebarView::Files,
                LeftSidebarView::Notes,
                LeftSidebarView::Conversations
            ]
        );
    }

    #[test]
    fn reconcile_collapses_legacy_multi_visible_state_to_one_unified_view() {
        let mut host = LeftSidebarHost::default();
        host.reconcile(SidebarRequests {
            files: true,
            notes: true,
            conversations: true,
        });
        assert_eq!(host.active_unified(), Some(LeftSidebarView::Files));
        assert_eq!(
            host.resolved_views(SidebarRequests {
                files: true,
                notes: true,
                conversations: true
            }),
            vec![LeftSidebarView::Files]
        );
    }
}
