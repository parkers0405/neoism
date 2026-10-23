use super::*;
#[cfg(test)]
#[path = "git_diff_view_pr_tests.rs"]
mod pull_request_tests;
use crate::panels::git_pull_request::{Intent, PullRequestView, Tab};
use neoism_protocol::pull_request::*;
impl FileSection {
    pub(super) fn pr_anchor_for_visual_row(
        &self,
        visual: usize,
        split: bool,
        column: usize,
    ) -> Option<PullRequestInlineComment> {
        let (line, side) = if split {
            let rows = if column == 0 { &self.left } else { &self.right };
            (
                rows.get(visual)?,
                if column == 0 {
                    PullRequestSide::Left
                } else {
                    PullRequestSide::Right
                },
            )
        } else {
            if visual >= *self.wrapped_offsets.last()? {
                return None;
            }
            let index = self
                .wrapped_offsets
                .partition_point(|row| *row <= visual)
                .saturating_sub(1);
            let line = self.lines.as_ref()?.get(index)?;
            (
                line,
                if line.kind == DiffLineKind::Remove {
                    PullRequestSide::Left
                } else {
                    PullRequestSide::Right
                },
            )
        };
        let line = if side == PullRequestSide::Left {
            line.old_line_number
        } else {
            line.new_line_number
        }?;
        Some(PullRequestInlineComment {
            path: self.change.path.clone(),
            side,
            line,
            start_line: None,
            body: String::new(),
        })
    }
}
impl GitDiffView {
    pub fn is_read_only(&self) -> bool {
        self.commit_id.is_some() || self.pull_request.is_some() || self.inbox.is_some()
    }
    pub fn pull_request_inbox(root: PathBuf) -> Self {
        let mut view = Self::new(root.clone(), PathBuf::new());
        view.inbox = Some(Box::new(super::inbox::PullRequestInbox::new(root)));
        view
    }
    pub fn is_pull_request_inbox(&self) -> bool {
        self.inbox.is_some()
    }
    pub fn is_pull_request(&self) -> bool {
        self.pull_request.is_some() || self.inbox.is_some()
    }
    pub fn pull_request_number(&self) -> Option<u64> {
        if self.inbox.is_some() {
            return None;
        }
        self.pull_request.as_ref().and_then(|p| p.number())
    }
    pub fn pull_request_title(&self) -> Option<&str> {
        if self.inbox.is_some() {
            return Some("Pull Requests");
        }
        self.pull_request.as_ref().map(|p| {
            p.detail
                .as_ref()
                .map(|d| d.summary.title.as_str())
                .unwrap_or(&p.fields[0])
        })
    }
    pub fn has_pull_request_drafts(&self) -> bool {
        if let Some(inbox) = &self.inbox {
            return inbox.has_drafts();
        }
        self.pull_request
            .as_ref()
            .is_some_and(|p| p.has_drafts() || p.pending)
    }
    pub fn from_pull_request(root: PathBuf, detail: PullRequestDetail) -> Self {
        let mut view = Self::new(root, PathBuf::new());
        let mut pr = PullRequestView::create(
            detail.summary.head_branch.clone(),
            detail.summary.head_sha.clone(),
            detail.summary.base_branch.clone(),
        );
        pr.focus = None;
        pr.detail = Some(detail.clone());
        view.pull_request = Some(pr);
        view.load_pr_files(&detail);
        view
    }
    pub fn create_pull_request(
        root: PathBuf,
        branch: String,
        head: String,
        default_branch: String,
    ) -> Self {
        let mut v = Self::new(root, PathBuf::new());
        v.pull_request = Some(PullRequestView::create(branch, head, default_branch));
        v
    }
    fn load_pr_files(&mut self, detail: &PullRequestDetail) {
        use neoism_protocol::git::GitChangeStatus as S;
        self.set_files(
            detail
                .files
                .iter()
                .map(|f| FileChange {
                    path: f.path.clone(),
                    additions: f.additions,
                    deletions: f.deletions,
                    staged: false,
                    status: match f.status {
                        S::Added => FileStatus::Added,
                        S::Deleted => FileStatus::Deleted,
                        S::Renamed => FileStatus::Renamed,
                        _ => FileStatus::Modified,
                    },
                })
                .collect(),
        );
        for d in &detail.diffs {
            let mut lines = Vec::new();
            crate::panels::git_diff::parse_diff_into_with_headers(
                d.patch.as_bytes(),
                &mut lines,
            );
            self.set_file_lines(Path::new(&d.path), lines);
        }
        self.pending_reveal = None;
    }
    pub fn apply_pull_request_reply(&mut self, reply: &PullRequestReply) {
        if let Some(inbox) = &mut self.inbox {
            inbox.apply(reply);
            return;
        }
        if let PullRequestReply::Detail { detail } = reply {
            if self.pull_request_number() == Some(detail.summary.number) {
                self.load_pr_files(detail);
            }
        }
        if let Some(p) = &mut self.pull_request {
            p.apply(reply);
        }
        let refresh = match reply {
            PullRequestReply::Created { pull_request } => Some(pull_request.number),
            PullRequestReply::ThreadUpdated { number }
            | PullRequestReply::ReviewSubmitted { number }
            | PullRequestReply::Merged { number }
                if self.pull_request_number() == Some(*number) =>
            {
                Some(*number)
            }
            _ => None,
        };
        if let Some(number) = refresh {
            self.pending_actions.push(GitDiffViewAction::PullRequest {
                root: self.root.clone(),
                request: PullRequestRequest::Read { number },
            });
        }
    }
    pub fn pull_request_text_input(&mut self, text: &str) -> bool {
        if let Some(inbox) = &mut self.inbox {
            return inbox.text_input(text);
        }
        self.pull_request
            .as_mut()
            .is_some_and(|p| p.insert_text(text))
    }
    pub fn pull_request_key(&mut self, name: &str, shift: bool, primary: bool) -> bool {
        if let Some(inbox) = &mut self.inbox {
            return inbox.key(name, shift, primary);
        }
        if !self.is_pull_request() {
            return false;
        }
        if primary && name == "Enter" {
            self.pr_intent(
                if self
                    .pull_request
                    .as_ref()
                    .is_some_and(|p| p.anchor.is_some())
                {
                    Intent::Queue
                } else {
                    Intent::Submit
                },
            );
            return true;
        }
        if name == "Tab" {
            let p = self.pull_request.as_mut().unwrap();
            if p.detail.is_some() && !p.sheet {
                let tabs = [Tab::Changes, Tab::Conversation, Tab::Checks];
                let i = tabs.iter().position(|t| *t == p.tab).unwrap_or(0);
                p.tab = tabs[(i + if shift { 2 } else { 1 }) % 3];
                p.scroll = 0.0;
                p.focus = None;
                return true;
            }
            if shift {
                p.focus_field(if p.detail.is_none() {
                    (p.focus.unwrap_or(0) + 2) % 3
                } else {
                    3
                });
                return true;
            }
            if p.focus.is_none() {
                p.focus_field(if p.detail.is_none() { 0 } else { 3 });
                return true;
            }
        }
        self.pull_request.as_mut().unwrap().key(name)
    }
    pub(super) fn pr_intent(&mut self, intent: Intent) {
        let root = self.root.clone();
        let p = self.pull_request.as_mut().unwrap();
        if p.pending
            && !matches!(
                &intent,
                Intent::Tab(_) | Intent::Browser(_) | Intent::Cancel
            )
        {
            return;
        }
        let number = p.number();
        let mut req = None;
        match intent {
            Intent::Tab(t) => {
                p.tab = t;
                p.scroll = 0.0;
                p.focus = None;
            }
            Intent::Field(i) => p.focus_field(i),
            Intent::Draft => p.draft = !p.draft,
            Intent::Browser(url) => {
                if url.starts_with("https://") || url.starts_with("http://") {
                    self.pending_actions
                        .push(GitDiffViewAction::OpenUrl { url });
                }
            }
            Intent::Refresh => {
                if !p.pending {
                    req = number.map(|number| PullRequestRequest::Read { number });
                    p.pending = req.is_some();
                }
            }
            Intent::Checkout(new_worktree) => {
                if p.has_drafts() {
                    p.message = Some(
                        "Finish or discard drafts before checkout or worktree navigation"
                            .into(),
                    );
                    return;
                }
                if !p.pending {
                    req = number.map(|number| PullRequestRequest::Checkout {
                        number,
                        expected_head: p.head.clone(),
                        new_worktree,
                    });
                    p.pending = req.is_some();
                }
            }
            Intent::Review => {
                p.sheet = true;
                p.scroll = 0.0;
                if p.fields[3].is_empty() {
                    p.reply_thread = None;
                }
                p.focus_field(3);
            }
            Intent::Event(e) => p.event = e,
            Intent::Submit => req = p.request(),
            Intent::Queue => p.queue(),
            Intent::Reply(id) => {
                p.sheet = true;
                p.scroll = 0.0;
                if p.fields[3].is_empty() && p.anchor.is_none() {
                    p.reply_thread = Some(id);
                } else {
                    p.message = Some("Finish or discard the current draft first".into());
                }
                p.focus_field(3);
            }
            Intent::Resolve(thread_id, resolved) => {
                if !p.pending {
                    req = number.map(|number| PullRequestRequest::ResolveThread {
                        number,
                        thread_id,
                        resolved,
                    });
                    p.pending = req.is_some();
                }
            }
            Intent::Discard => {
                if !p.pending {
                    p.comments.clear();
                    p.anchor = None;
                    p.reply_thread = None;
                    p.fields[3].clear();
                    if p.detail.is_none() {
                        p.fields[0].clear();
                        p.fields[1].clear();
                        p.fields[2] = p.initial_base.clone();
                        p.draft = false;
                    }
                    p.stale = false;
                    p.sheet = false;
                    p.focus = None;
                }
            }
            Intent::Cancel => {
                p.sheet = false;
                p.focus = None;
            }
        }
        if let Some(request) = req {
            self.pending_actions
                .push(GitDiffViewAction::PullRequest { root, request });
        }
    }
    pub(super) fn pr_pointer_down(&mut self, x: f32, y: f32) -> bool {
        let Some(p) = &self.pull_request else {
            return false;
        };
        if let Some((_, intent)) = p.hits.iter().find(|(r, _)| r.contains(x, y)) {
            let intent = intent.clone();
            self.pr_intent(intent);
            return true;
        }
        if p.pending || p.sheet || p.detail.is_none() || p.tab != Tab::Changes {
            return true;
        }
        if let Some(anchor) = self.pr_anchor_at(x, y) {
            let p = self.pull_request.as_mut().unwrap();
            if p.anchor.is_none() {
                if p.fields[3].is_empty() {
                    p.anchor = Some(anchor);
                    p.reply_thread = None;
                } else {
                    p.message = Some("Finish or discard the current draft first".into());
                }
            }
            p.sheet = true;
            p.scroll = 0.0;
            p.focus_field(3);
            return true;
        }
        false
    }
    /// Resolve actual source coordinates, never visual/wrapped row numbers.
    fn pr_anchor_at(&self, x: f32, y: f32) -> Option<PullRequestInlineComment> {
        if !self.is_pull_request() {
            return None;
        }
        self.pr_line_hits
            .iter()
            .find(|(r, _)| r.contains(x, y))
            .map(|(_, a)| a.clone())
    }
}
