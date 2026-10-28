//! Original Git panel with a host-driven daemon request queue; no local git IO.
use neoism_protocol::git::{
    GitChangeBlock, GitChangeStatus, GitFileChange, GitServerMessage,
};
use neoism_ui::{
    panels::git_diff::{
        FileChange, FileStatus, GitDiffIo, GitDiffPanel, PanelHit, PullRequestOpen,
        ScrollbarKind,
    },
    primitives::{paint_record::RecordingPainter, IdeTheme},
};
use serde_json::{json, Value};
use std::{
    cell::RefCell,
    collections::HashMap,
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
};
#[derive(Default)]
struct Queue(Mutex<Vec<Value>>);
impl Queue {
    fn send(&self, message: Value) {
        if let Ok(mut queue) = self.0.lock() {
            queue.push(message);
        }
    }
    fn take(&self) -> Vec<Value> {
        self.0
            .lock()
            .map(|mut queue| std::mem::take(&mut *queue))
            .unwrap_or_default()
    }
}
impl GitDiffIo for Queue {
    fn is_host_driven(&self) -> bool {
        true
    }
    fn collect_files(&self, _: &Path) -> Vec<FileChange> {
        self.send(json!("ChangedFiles"));
        vec![]
    }
    fn request_latest_commit(&self) {
        self.send(json!({"Log":{"max_count":1}}));
    }
    fn request_commit(&self, revision: &str) {
        self.send(json!({"ReadCommit":{"revision":revision}}));
    }
    fn request_diff(&self, path: &str) {
        self.request_diffs(&[path.to_owned()]);
    }
    fn request_diffs(&self, paths: &[String]) {
        if !paths.is_empty() {
            self.send(json!({"DiffFiles":{"paths":paths}}));
        }
    }
    fn request_base(&self, path: &str) {
        self.send(json!({"ReadBase":{"path":path}}));
    }
    fn stage(&self, _: &Path, path: &str) -> Result<(), String> {
        self.send(json!({"Stage":{"path":path}}));
        Ok(())
    }
    fn unstage(&self, _: &Path, path: &str) -> Result<(), String> {
        self.send(json!({"Unstage":{"path":path}}));
        Ok(())
    }
    fn restore(&self, _: &Path, path: &str) -> Result<(), String> {
        self.send(json!({"Restore":{"path":path}}));
        Ok(())
    }
    fn apply_block(
        &self,
        _: &Path,
        path: &str,
        block: &GitChangeBlock,
        restore: bool,
    ) -> Result<(), String> {
        self.send(json!({"ApplyBlock":{"path":path,"block":block,"restore":restore}}));
        Ok(())
    }
    fn commit(&self, _: &Path, message: &str) -> Result<(), String> {
        self.send(json!({"Commit":{"message":message}}));
        Ok(())
    }
    fn list_branches(&self, _: &Path) -> Vec<String> {
        self.send(json!("Branches"));
        vec![]
    }
    fn checkout(&self, _: &Path, branch: &str) -> Result<(), String> {
        self.send(json!({"Checkout":{"branch":branch}}));
        Ok(())
    }
    fn fetch(&self, _: &Path) -> Result<(), String> {
        self.send(json!("Fetch"));
        Ok(())
    }
    fn pull(&self, _: &Path) -> Result<(), String> {
        self.send(json!("Pull"));
        Ok(())
    }
    fn push(&self, _: &Path) -> Result<(), String> {
        self.send(json!("Push"));
        Ok(())
    }
    fn publish(&self, _: &Path) -> Result<(), String> {
        self.send(json!("Publish"));
        Ok(())
    }
    fn request_pull_request(
        &self,
        request: neoism_protocol::pull_request::PullRequestRequest,
    ) {
        self.send(json!({"PullRequest":{"request":request}}));
    }
}
pub(crate) fn file_change(file: GitFileChange) -> FileChange {
    FileChange {
        path: file.path,
        additions: file.additions,
        deletions: file.deletions,
        staged: file.staged,
        status: match file.status {
            GitChangeStatus::Modified => FileStatus::Modified,
            GitChangeStatus::Staged => FileStatus::Staged,
            GitChangeStatus::Mixed => FileStatus::Mixed,
            GitChangeStatus::Added => FileStatus::Added,
            GitChangeStatus::Deleted => FileStatus::Deleted,
            GitChangeStatus::Renamed => FileStatus::Renamed,
            GitChangeStatus::Untracked => FileStatus::Untracked,
            GitChangeStatus::Conflict => FileStatus::Conflict,
        },
    }
}
struct Host {
    panel: GitDiffPanel,
    paint: RecordingPainter,
    queue: Arc<Queue>,
    width: f32,
    height: f32,
    scale: f32,
    theme: String,
    effects: Vec<Value>,
    drag: Option<ScrollbarKind>,
    resize: Option<(f32, f32)>,
}
thread_local! {static HOSTS:RefCell<HashMap<u64,Host>>=RefCell::new(HashMap::new());static NEXT:RefCell<u64>=RefCell::new(0xd000_0000);}
fn text<'a>(v: &'a Value, key: &str) -> &'a str {
    v[key].as_str().unwrap_or("")
}
fn n(v: &Value, key: &str, default: f32) -> f32 {
    v[key].as_f64().map(|n| n as f32).unwrap_or(default)
}
impl Host {
    fn frame(&mut self) -> Value {
        self.paint.begin_frame();
        self.panel.render(
            &mut self.paint,
            self.width,
            0.0,
            self.height,
            &IdeTheme::by_name(&self.theme),
        );
        if let Some(copy) = self.panel.take_latest_commit_copy() {
            self.effects.push(json!({"kind":"copy","text":copy}));
        }
        for (root, detail) in self.panel.take_commit_details() {
            self.effects
                .push(json!({"kind":"commit-detail","root":root,"detail":detail}));
        }
        for open in self.panel.take_pull_request_opens() {
            self.effects.push(match open{PullRequestOpen::Inbox{root}=>json!({"kind":"pr-inbox","root":root}),PullRequestOpen::Create{root,branch,head,default_branch}=>json!({"kind":"pr-create","root":root,"branch":branch,"head":head,"defaultBranch":default_branch}),PullRequestOpen::Detail{root,detail}=>json!({"kind":"pr-detail","root":root,"detail":detail})});
        }
        for reply in self.panel.take_pull_request_replies() {
            self.effects.push(json!({"kind":"pr-reply","reply":reply}));
        }
        for notice in self.panel.take_pull_request_notices() {
            self.effects.push(json!({"kind":"notice","text":notice}));
        }
        let mut frame = self.panel.host_frame();
        frame["commands"] = json!(self.paint.commands());
        frame["requests"] = json!(self.queue.take());
        frame["effects"] = json!(std::mem::take(&mut self.effects));
        frame
    }
    fn open_review(&mut self) {
        if let Some((path, root)) = self.panel.selected_file_target() {
            self.effects
                .push(json!({"kind":"review","root":root,"path":path}));
        }
    }
    fn reply(&mut self, v: &Value) -> Result<(), String> {
        let reply: GitServerMessage =
            serde_json::from_value(v.clone()).map_err(|error| error.to_string())?;
        let snapshot = matches!(
            &reply,
            GitServerMessage::RepoStatus { .. } | GitServerMessage::ChangedFiles { .. }
        );
        match reply {
            GitServerMessage::RepoStatus { snapshot, .. } => {
                self.panel.host_set_repo(
                    snapshot.repo_root.map(PathBuf::from),
                    snapshot.branch,
                );
                self.panel.set_upstream(snapshot.upstream);
                self.panel.host_set_files(
                    snapshot.files.into_iter().map(file_change).collect(),
                );
                if let Some(error) = snapshot.error {
                    self.panel.host_set_error(error);
                }
            }
            GitServerMessage::ChangedFiles {
                files,
                branch,
                upstream,
                error,
            } => {
                self.panel.host_set_branch(branch);
                self.panel.set_upstream(upstream);
                self.panel
                    .host_set_files(files.into_iter().map(file_change).collect());
                if let Some(error) = error {
                    self.panel.host_set_error(error);
                }
            }
            GitServerMessage::FileDiffs { diffs } => {
                for diff in diffs {
                    self.panel.host_set_diff_text(&diff.path, &diff.patch);
                }
            }
            GitServerMessage::BaseFile { path, text } => {
                self.panel.host_set_base_text(&path, text)
            }
            GitServerMessage::Branches { branches } => {
                self.panel.host_set_branches(branches)
            }
            GitServerMessage::Branch { name } => self.panel.host_set_branch(name),
            GitServerMessage::Log { commits } => self
                .panel
                .host_set_latest_commit(commits.into_iter().next()),
            GitServerMessage::CommitDetail { detail } => {
                self.panel.host_commit_detail(detail)
            }
            GitServerMessage::PullRequest { reply } => {
                self.panel.host_pull_request_reply(reply)
            }
            GitServerMessage::Error { message } => self.panel.host_set_error(message),
            _ => {}
        }
        if snapshot && self.panel.is_visible() {
            if let Some(selected) = self.panel.selected_file_index() {
                self.panel.select_file(selected);
            }
        }
        Ok(())
    }
    fn click(&mut self, x: f32, y: f32) {
        let hit = self.panel.hit_test(x, y);
        if self.panel.actions_menu_is_open() && self.panel.actions_menu_contains(x, y) {
            if let PanelHit::ActionMenuRow(slot) = hit {
                self.panel.activate_action_row(slot);
            }
            return;
        }
        if self.panel.actions_menu_is_open() && !matches!(hit, PanelHit::ActionsButton) {
            self.panel.close_actions_menu();
        }
        if self.panel.branch_menu_is_open()
            && !matches!(
                hit,
                PanelHit::BranchMenuRow(_)
                    | PanelHit::BranchFilterBox
                    | PanelHit::BranchButton
            )
        {
            self.panel.close_branch_menu();
        }
        match hit {
            PanelHit::Outside => self.panel.set_focused(false),
            PanelHit::Close => self.panel.close(),
            PanelHit::PullRequest => {
                self.panel.activate_pull_request();
            }
            PanelHit::FileRow(index) => {
                self.panel.set_focused(true);
                self.panel.focus_files_section();
                self.panel.select_file(index);
                self.open_review();
            }
            PanelHit::FileCheckbox(index) => {
                self.panel.set_focused(true);
                self.panel.focus_files_section();
                self.panel.toggle_stage(index);
            }
            PanelHit::FolderToggle(index) => {
                self.panel.set_focused(true);
                self.panel.focus_files_section();
                self.panel.toggle_folder(index);
            }
            PanelHit::FolderCheckbox(index) => self.panel.toggle_folder_stage(index),
            PanelHit::BranchButton => self.panel.toggle_branch_menu(),
            PanelHit::BranchMenuRow(index) => self.panel.activate_branch_row(index),
            PanelHit::CommitBox => self.panel.focus_commit_box(true),
            PanelHit::CommitButton => self.panel.commit(),
            PanelHit::PushButton => self.panel.activate_push_button(),
            PanelHit::ActionsButton => self.panel.toggle_actions_menu(),
            PanelHit::ActionMenuRow(index) => self.panel.activate_action_row(index),
            PanelHit::LatestCommit => self.panel.open_latest_commit(),
            PanelHit::LatestCommitCopy => self.panel.copy_latest_commit(),
            _ => {}
        }
    }
    fn key(&mut self, v: &Value) {
        let key = text(v, "key");
        let ch = if text(v, "char").is_empty() {
            if key == "space" {
                " "
            } else {
                key
            }
        } else {
            text(v, "char")
        };
        let ctrl =
            v["ctrl"].as_bool().unwrap_or(false) || v["cmd"].as_bool().unwrap_or(false);
        let alt = v["alt"].as_bool().unwrap_or(false);
        let shift = v["shift"].as_bool().unwrap_or(false);
        if self.panel.pull_request_lookup_active() {
            self.panel.pull_request_lookup_key(
                match key {
                    "enter" => "Enter",
                    "escape" => "Escape",
                    "backspace" => "Backspace",
                    _ => ch,
                },
                Some(ch),
            );
            return;
        }
        if self.panel.actions_menu_is_open() {
            match key {
                "down" | "right" => self.panel.actions_menu_move(1),
                "up" | "left" => self.panel.actions_menu_move(-1),
                "tab" => self.panel.actions_menu_move(if shift { -1 } else { 1 }),
                "pageup" => self.panel.actions_menu_move(-5),
                "pagedown" => self.panel.actions_menu_move(5),
                "enter" | "space" => self.panel.actions_menu_activate(),
                "escape" => {
                    self.panel.close_actions_menu();
                    self.panel.focus_commit_box(true);
                }
                _ => {}
            }
            return;
        }
        if key == "f4" || alt && key == "down" && self.panel.commit_box_focused() {
            self.panel.open_actions_menu();
            return;
        }
        if self.panel.branch_menu_is_open() {
            match key {
                "enter" => self.panel.branch_menu_activate(),
                "escape" => self.panel.close_branch_menu(),
                "down" => self.panel.branch_menu_move(1),
                "up" => self.panel.branch_menu_move(-1),
                "backspace" => self.panel.branch_filter_backspace(),
                _ if !ctrl && !alt => self.panel.branch_filter_insert(ch),
                _ => {}
            }
            return;
        }
        if alt && !ctrl {
            match key {
                "up" => self.panel.section_focus_prev(),
                "down" => self.panel.section_focus_next(),
                "left" => {
                    self.panel.section_move_left();
                }
                "right" => {
                    self.panel.section_move_right();
                }
                _ => {}
            }
            return;
        }
        if self.panel.commit_box_focused() {
            match key {
                "enter" if shift => self.panel.commit_input_insert("\n"),
                "enter" => self.panel.commit(),
                "escape" => self.panel.focus_commit_box(false),
                "backspace" => self.panel.commit_input_backspace(),
                _ if !ctrl && !alt && ch.chars().count() == 1 => {
                    self.panel.commit_input_insert(ch)
                }
                _ => {}
            }
            return;
        }
        match key {
            "enter" if ctrl => self.panel.commit(),
            "d" if ctrl => self.panel.select_half_page_down(),
            "u" if ctrl => self.panel.select_half_page_up(),
            "enter" | "space" if self.panel.branch_section_focused() => {
                self.panel.toggle_branch_menu()
            }
            "space" => self.panel.toggle_stage_selected(),
            "c" if !ctrl => self.panel.focus_commit_box(true),
            "p" if !ctrl => {
                self.panel.activate_pull_request();
            }
            "down" | "j" if !ctrl => {
                if self.panel.diff_section_focused() {
                    self.panel.scroll_diff_keys(true);
                } else {
                    let count = self.panel.take_count();
                    self.panel.select_next_by(count);
                }
            }
            "up" | "k" if !ctrl => {
                if self.panel.diff_section_focused() {
                    self.panel.scroll_diff_keys(false);
                } else {
                    let count = self.panel.take_count();
                    self.panel.select_prev_by(count);
                }
            }
            "g" => {
                if self.panel.note_g() {
                    self.panel.select_first_file();
                }
            }
            "G" => {
                if let Some(count) = self.panel.pending_count() {
                    self.panel.goto_file(count);
                } else {
                    self.panel.select_last_file();
                }
            }
            "$" => self.panel.select_last_file(),
            "enter" => {
                if self.panel.checkbox_column_focused() {
                    self.panel.toggle_stage_selected();
                } else {
                    self.open_review();
                }
            }
            "escape" => self.panel.close(),
            _ if !ctrl && !alt => {
                if let Some(digit) = ch
                    .chars()
                    .next()
                    .filter(|_| ch.len() == 1)
                    .and_then(|ch| ch.to_digit(10))
                {
                    self.panel.push_count_digit(digit);
                }
            }
            _ => {}
        }
    }
}
pub fn dispatch(v: &Value) -> Option<Result<Value, String>> {
    let op = text(v, "op");
    if !op.starts_with("gitpanel.") {
        return None;
    }
    Some(HOSTS.with(|hosts| {
        let mut hosts = hosts.borrow_mut();
        if op == "gitpanel.new" {
            let id = NEXT.with(|next| {
                let mut next = next.borrow_mut();
                *next += 1;
                *next
            });
            let queue = Arc::new(Queue::default());
            let mut panel = GitDiffPanel::new();
            panel.set_io_provider(queue.clone());
            hosts.insert(
                id,
                Host {
                    panel,
                    paint: RecordingPainter::new(
                        v["fontFamily"].as_str().unwrap_or("Cascadia Code NF"),
                        v["fontWeight"].as_u64().unwrap_or(400) as u16,
                        vec!["Symbols Nerd Font Mono".into()],
                    ),
                    queue,
                    width: 1440.0,
                    height: 800.0,
                    scale: 1.0,
                    theme: "pastel_dark".into(),
                    effects: vec![],
                    drag: None,
                    resize: None,
                },
            );
            return Ok(json!({"id":id}));
        }
        let id = v["id"].as_u64().ok_or("Missing Git panel id")?;
        if op == "gitpanel.dispose" {
            hosts.remove(&id);
            return Ok(Value::Null);
        }
        let host = hosts.get_mut(&id).ok_or("Unknown Git panel")?;
        match op {
            "gitpanel.open" => host.panel.open(
                Some(PathBuf::from(text(v, "root"))),
                v["branch"].as_str().map(str::to_owned),
            ),
            "gitpanel.close" => host.panel.close(),
            "gitpanel.focus" => host
                .panel
                .set_focused(v["focused"].as_bool().unwrap_or(false)),
            "gitpanel.configure" => {
                host.paint = RecordingPainter::new(
                    v["fontFamily"].as_str().unwrap_or("Cascadia Code NF"),
                    v["fontWeight"].as_u64().unwrap_or(400) as u16,
                    vec!["Symbols Nerd Font Mono".into()],
                );
            }
            "gitpanel.layout" => {
                host.width = n(v, "width", host.width);
                host.height = n(v, "height", host.height);
                let scale = n(v, "scale", host.scale);
                if scale != host.scale {
                    host.scale = scale;
                    host.panel.set_scale(scale);
                }
                if let Some(theme) = v["theme"].as_str() {
                    host.theme = theme.into();
                }
            }
            "gitpanel.reply" => host.reply(&v["message"])?,
            "gitpanel.key" => host.key(v),
            "gitpanel.commit-text" => {
                host.panel.host_set_commit_text(text(v, "text").into())
            }
            "gitpanel.pointer" => {
                let x = n(v, "x", -1.0);
                let y = n(v, "y", -1.0);
                match text(v, "kind") {
                    "hover" => {
                        host.panel.update_latest_commit_hover(x, y);
                    }
                    "up" => {
                        host.drag = None;
                        host.resize = None;
                        host.panel.end_divider_drag();
                    }
                    "move" => {
                        if let Some((start, width)) = host.resize {
                            let target = width - (x - start);
                            let current = host.panel.width();
                            host.panel.resize(target - current);
                        } else if let Some(kind) = host.drag {
                            host.panel.drag_scrollbar(kind, y);
                        } else {
                            host.panel.drag_divider(y);
                        }
                    }
                    _ => {
                        if host.panel.is_hovering_resize_edge(x, y) {
                            host.resize = Some((x, host.panel.width()));
                        } else if let Some(kind) = host.panel.scrollbar_hit(x, y) {
                            host.drag = Some(kind);
                            host.panel.drag_scrollbar(kind, y);
                        } else if !host.panel.begin_divider_drag(x, y) {
                            host.click(x, y);
                        }
                    }
                }
            }
            "gitpanel.scroll" => {
                let x = n(v, "x", 0.0);
                let y = n(v, "y", 0.0);
                let delta = n(v, "delta", 0.0);
                if v["precise"].as_bool().unwrap_or(false) {
                    host.panel.scroll_touch_at(x, y, delta);
                } else {
                    host.panel.scroll_at(x, y, delta);
                }
            }
            "gitpanel.resize" => host.panel.resize(n(v, "delta", 0.0)),
            "gitpanel.refresh" => {
                let root = host.panel.repo_root();
                let branch = host.panel.branch();
                host.panel.refresh(root, branch);
            }
            "gitpanel.commit-succeeded" => {
                host.panel.host_commit_succeeded(text(v, "message"))
            }
            "gitpanel.review" => host.open_review(),
            "gitpanel.request-review" => host.panel.request_review_diffs(),
            "gitpanel.stage" => {
                if let Some(index) = v["index"].as_u64() {
                    host.panel.toggle_stage(index as usize);
                } else {
                    host.panel.stage_all_toggle();
                }
            }
            "gitpanel.restore" => {
                let root = PathBuf::from(text(v, "root"));
                host.panel.restore_file(&root, text(v, "path"));
            }
            "gitpanel.apply-block" => {
                let block = serde_json::from_value(v["block"].clone())
                    .map_err(|e| e.to_string())?;
                host.panel.apply_block(
                    &PathBuf::from(text(v, "root")),
                    text(v, "path"),
                    block,
                    v["restore"].as_bool().unwrap_or(false),
                );
            }
            "gitpanel.focus-commit" => host.panel.focus_commit_box(true),
            "gitpanel.frame" => {}
            _ => return Err(format!("Unknown Git panel operation {op}")),
        }
        Ok(host.frame())
    }))
}
