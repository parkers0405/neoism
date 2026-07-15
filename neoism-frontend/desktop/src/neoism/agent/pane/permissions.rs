use super::*;

impl NeoismAgentPane {
    pub fn pending_permission(&self) -> Option<&NeoismAgentPendingPermission> {
        self.pending_permission.as_ref()
    }

    pub fn register_permission_choice_rect(
        &mut self,
        choice: NeoismAgentPermissionChoice,
        rect: [f32; 4],
    ) {
        self.permission_choice_hit_rects.push((choice, rect));
    }

    pub fn clear_permission_choice_hit_rects(&mut self) {
        self.permission_choice_hit_rects.clear();
    }

    pub fn respond_permission_at(&mut self, x: f32, y: f32) -> bool {
        let Some(choice) = permission_policy::choice_at(
            self.permission_choice_hit_rects.iter().copied(),
            x,
            y,
        ) else {
            return false;
        };
        self.respond_pending_permission(choice)
    }

    pub fn move_permission_selection(&mut self, delta: isize) -> bool {
        let Some(permission) = self.pending_permission.as_mut() else {
            return false;
        };
        if permission.responding {
            return true;
        }
        permission.selected =
            permission_policy::move_selected_index(permission.selected, delta);
        true
    }

    pub(crate) fn enqueue_pending_permission(
        &mut self,
        permission: NeoismAgentPendingPermission,
    ) {
        self.note_permission_branch_status(&permission, BranchStatus::WaitingPermission);
        permission_policy::enqueue_permission(
            &mut self.pending_permission,
            &mut self.pending_permission_queue,
            permission,
            |permission| permission.id.as_str(),
        );
        self.sync_subagent_waiting_clock();
        self.maybe_auto_respond_permission();
    }

    /// `/yolo` toggle. Turning it on immediately answers the current
    /// pending request; the reply-succeeded promotion path keeps
    /// draining the queue after that.
    pub fn toggle_skip_permissions(&mut self) {
        self.skip_permissions = !self.skip_permissions;
        if self.skip_permissions {
            self.push_notice(
                "Permissions: skipping ALL requests (dangerous) — /yolo to turn off"
                    .to_string(),
                NeoismAgentNoticeLevel::Warn,
            );
            self.maybe_auto_respond_permission();
        } else {
            self.push_notice(
                "Permissions: prompts re-enabled".to_string(),
                NeoismAgentNoticeLevel::Info,
            );
        }
    }

    pub fn skip_permissions_enabled(&self) -> bool {
        self.skip_permissions
    }

    pub(crate) fn maybe_auto_respond_permission(&mut self) {
        if self.skip_permissions && self.pending_permission.is_some() {
            self.respond_pending_permission(NeoismAgentPermissionChoice::Once);
        }
    }

    pub(crate) fn remove_pending_permission(&mut self, request_id: &str) -> bool {
        permission_policy::remove_permission(
            &mut self.pending_permission,
            &mut self.pending_permission_queue,
            request_id,
            |permission| permission.id.as_str(),
        )
    }

    pub(crate) fn clear_pending_permission_current(&mut self) {
        permission_policy::clear_current_permission(
            &mut self.pending_permission,
            &mut self.pending_permission_queue,
        );
    }

    pub(crate) fn note_permission_branch_status(
        &mut self,
        permission: &NeoismAgentPendingPermission,
        status: BranchStatus,
    ) {
        if permission.session_id.is_empty()
            || Some(permission.session_id.as_str()) == self.session_id.as_deref()
        {
            return;
        }
        self.side_panel
            .set_branch_activity_status(permission.session_id.clone(), status);
    }

    pub fn submit_pending_permission(&mut self) -> bool {
        let Some(permission) = self.pending_permission.as_ref() else {
            return false;
        };
        let choice = match permission_policy::selected_reply(permission.selected) {
            "always" => NeoismAgentPermissionChoice::Always,
            "reject" => NeoismAgentPermissionChoice::Reject,
            _ => NeoismAgentPermissionChoice::Once,
        };
        self.respond_pending_permission(choice)
    }

    pub fn respond_pending_permission(
        &mut self,
        choice: NeoismAgentPermissionChoice,
    ) -> bool {
        match permission_policy::start_reply(
            &mut self.pending_permission,
            |permission| permission.id.as_str(),
            |permission| permission.responding,
            |permission, responding| permission.responding = responding,
        ) {
            PermissionReplyStart::NoCurrent => false,
            PermissionReplyStart::AlreadyResponding => true,
            PermissionReplyStart::MissingId => {
                self.system_message("Permission", "pending permission has no id");
                true
            }
            PermissionReplyStart::Ready { id } => {
                self.push_outbound(OutboundAgentCommand::ReplyPermission {
                    id,
                    reply: choice.reply().to_string(),
                });
                true
            }
        }
    }
}
