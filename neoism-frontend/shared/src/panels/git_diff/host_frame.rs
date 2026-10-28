//! Data-only original panel geometry for another native GPU host.
use super::{types::Rect, GitDiffPanel};
use serde_json::{json, Value};
fn rect(value: Rect) -> [f32; 4] {
    [value.x, value.y, value.w, value.h]
}
impl GitDiffPanel {
    pub fn host_frame(&self) -> Value {
        let mut hits = Vec::new();
        if self.visible && self.panel_rect.w > 0.0 {
            hits.push(json!({"id":"resize","rect":[self.panel_rect.x-super::RESIZE_HIT_HALF,self.panel_rect.y,super::RESIZE_HIT_HALF*2.0,self.panel_rect.h]}));
        }
        for (id, bounds) in [
            ("close", self.close_rect),
            ("commit-input", self.commit_box_rect),
            ("commit", self.commit_button_rect),
            ("branch", self.branch_button_rect),
            ("branch-input", self.branch_filter_rect),
            ("actions", self.actions_button_rect),
            ("push", self.push_button_rect),
            ("latest-commit", self.latest_commit_rect),
            ("copy-commit", self.latest_commit_copy_rect),
        ] {
            if bounds.w > 0.0 && bounds.h > 0.0 {
                hits.push(json!({"id":id,"rect":rect(bounds)}));
            }
        }
        for (index, bounds) in &self.file_row_rects {
            hits.push(json!({"id":format!("file:{index}"),"rect":rect(*bounds)}));
        }
        for (index, bounds) in &self.file_checkbox_rects {
            hits.push(json!({"id":format!("stage:{index}"),"rect":rect(*bounds)}));
        }
        for (index, bounds) in &self.folder_row_rects {
            hits.push(json!({"id":format!("folder:{index}"),"rect":rect(*bounds)}));
        }
        for (index, bounds) in &self.folder_checkbox_rects {
            hits.push(json!({"id":format!("stage-folder:{index}"),"rect":rect(*bounds)}));
        }
        for (index, bounds) in &self.actions_menu_row_rects {
            hits.push(json!({"id":format!("action:{index}"),"rect":rect(*bounds)}));
        }
        for (index, (branch, bounds)) in self.branch_menu_row_rects.iter().enumerate() {
            hits.push(json!({"id":format!("branch:{index}"),"branch":branch,"rect":rect(*bounds)}));
        }
        json!({"rect":rect(self.panel_rect),"visible":self.visible,"focused":self.focused,"width":self.panel_rect.w,
   "repoRoot":self.repo_root(),"loading":self.loading(),"error":self.error(),"branch":self.branch(),"upstream":self.upstream(),"selected":self.selected_file_index(),
   "files":self.files().into_iter().map(|file|json!({"path":file.path,"status":file.status.as_str(),"additions":file.additions,"deletions":file.deletions,"staged":file.staged})).collect::<Vec<_>>(),
   "cursorRect":self.selected_cursor_rect(),"commitText":self.commit_input_text(),"commitFocused":self.commit_box_focused(),"branchQuery":self.branch_filter.text(),"branchMenu":self.branch_menu_is_open(),"actionsMenu":self.actions_menu_is_open(),
   "animating":self.needs_redraw(),"hits":hits,"filesScrollbar":rect(self.files_scrollbar_thumb_rect),"diffScrollbar":rect(self.diff_scrollbar_thumb_rect)})
    }
    pub fn host_set_commit_text(&mut self, text: String) {
        self.commit_input.set_text(text);
    }
}
