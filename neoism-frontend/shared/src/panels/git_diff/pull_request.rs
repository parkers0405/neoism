use super::{types::Rect, GitDiffPanel};
use neoism_protocol::pull_request::*;
use std::path::PathBuf;
use web_time::Instant;
#[cfg(test)]
#[path = "pull_request_tests.rs"]
mod tests;
#[derive(Debug, Clone)]
pub enum PullRequestOpen {
    Inbox {
        root: PathBuf,
    },
    Create {
        root: PathBuf,
        branch: String,
        head: String,
        default_branch: String,
    },
    Detail {
        root: PathBuf,
        detail: PullRequestDetail,
    },
}
#[derive(Default)]
pub(super) struct SidebarPullRequest {
    identity: Option<(PathBuf, Option<String>)>,
    generation: u64,
    last: Option<Instant>,
    pub(super) pending: bool,
    dirty: bool,
    discovering: bool,
    open_read: bool,
    error: Option<String>,
    open_create: bool,
    notices: Vec<String>,
    pub(super) discovered: Option<PullRequestReply>,
    replies: Vec<PullRequestReply>,
    opens: Vec<PullRequestOpen>,
    lookup: Option<String>,
    pub rect: Option<Rect>,
    hovered: bool,
}
#[cfg(any(not(target_arch = "wasm32"), test))]
fn completion_is_current(
    d: &super::types::PanelData,
    root: &std::path::Path,
    branch: &Option<String>,
    generation: u64,
) -> bool {
    d.repo_root.as_deref() == Some(root)
        && &d.branch == branch
        && d.pr.generation == generation
}
impl GitDiffPanel {
    pub(super) fn pull_request_needs_redraw(&self) -> bool {
        self.data.lock().is_ok_and(|d| {
            d.pr.pending
                || (self.visible && d.pr.dirty)
                || !d.pr.replies.is_empty()
                || !d.pr.opens.is_empty()
                || !d.pr.notices.is_empty()
        })
    }
    pub fn begin_external_pull_request(&mut self) -> bool {
        let Ok(mut d) = self.data.lock() else {
            return false;
        };
        if d.pr.pending {
            return false;
        }
        d.pr.pending = true;
        d.pr.identity = d.repo_root.clone().map(|root| (root, d.branch.clone()));
        d.pr.discovering = false;
        true
    }
    pub fn request_pull_request(&mut self, request: PullRequestRequest) -> bool {
        let Some(io) = self.io.clone() else {
            return false;
        };
        if !io.pull_requests_ready() {
            return false;
        }
        let (root, generation, branch) = {
            let Ok(mut d) = self.data.lock() else {
                return false;
            };
            let Some(root) = d.repo_root.clone() else {
                return false;
            };
            if d.pr.pending {
                return false;
            }
            d.pr.pending = true;
            d.pr.identity = Some((root.clone(), d.branch.clone()));
            d.pr.discovering = matches!(&request, PullRequestRequest::Discover { .. });
            (root, d.pr.generation, d.branch.clone())
        };
        if io.is_host_driven() {
            io.request_pull_request(request);
            return true;
        }
        #[cfg(not(target_arch = "wasm32"))]
        {
            let data = self.data.clone();
            let discovering = matches!(&request, PullRequestRequest::Discover { .. });
            std::thread::spawn(move || {
                let reply = io.pull_request(&root, request);
                if let Ok(mut d) = data.lock() {
                    if d.repo_root.as_deref() != Some(root.as_path())
                        || d.pr.generation != generation
                        || (discovering
                            && !completion_is_current(&d, &root, &branch, generation))
                    {
                        return;
                    }
                    d.pr.pending = false;
                    Self::record_pr(&mut d, reply);
                }
            });
            true
        }
        #[cfg(target_arch = "wasm32")]
        {
            let _ = (root, generation, branch);
            self.host_pull_request_reply(PullRequestReply::Error {
                kind: PullRequestErrorKind::Unavailable,
                message: "Pull requests require a connected repository host.".into(),
            });
            false
        }
    }
    fn record_pr(d: &mut super::types::PanelData, reply: PullRequestReply) {
        d.pr.dirty = true;
        let discovering = std::mem::take(&mut d.pr.discovering);
        match &reply {
            PullRequestReply::Discovered {
                branch,
                head,
                default_branch,
                pull_request,
            } => {
                if std::mem::take(&mut d.pr.open_create) {
                    if let Some(pr) = pull_request {
                        d.pr.notices.push(format!(
                            "This branch already has pull request #{}.",
                            pr.number
                        ));
                    } else if let Some(root) = &d.repo_root {
                        d.pr.opens.push(PullRequestOpen::Create {
                            root: root.clone(),
                            branch: branch.clone(),
                            head: head.clone(),
                            default_branch: default_branch.clone(),
                        });
                    }
                }
                d.pr.discovered = Some(reply);
                d.pr.error = None;
                return;
            }
            PullRequestReply::Created { pull_request }
                if d.branch.as_deref() == Some(pull_request.head_branch.as_str()) =>
            {
                let default_branch = match &d.pr.discovered {
                    Some(PullRequestReply::Discovered { default_branch, .. }) => {
                        default_branch.clone()
                    }
                    _ => pull_request.base_branch.clone(),
                };
                d.pr.discovered = Some(PullRequestReply::Discovered {
                    branch: pull_request.head_branch.clone(),
                    head: pull_request.head_sha.clone(),
                    default_branch,
                    pull_request: Some(pull_request.clone()),
                });
            }
            PullRequestReply::Detail { detail } => {
                if d.pr.open_read {
                    if let Some(root) = &d.repo_root {
                        d.pr.opens.push(PullRequestOpen::Detail {
                            root: root.clone(),
                            detail: detail.clone(),
                        });
                    }
                }
            }
            PullRequestReply::Error { message, .. } => {
                d.pr.error = Some(message.clone());
                if d.pr.open_create || d.pr.open_read {
                    d.pr.notices.push(message.clone());
                }
                d.pr.open_create = false;
            }
            _ => {}
        }
        let opening = d.pr.open_read;
        d.pr.open_read = false;
        if !discovering && !opening {
            d.pr.replies.push(reply);
        }
    }
    pub fn host_pull_request_reply(&mut self, reply: PullRequestReply) {
        if let Ok(mut d) = self.data.lock() {
            d.pr.pending = false;
            Self::record_pr(&mut d, reply);
        }
    }
    pub fn take_pull_request_replies(&mut self) -> Vec<PullRequestReply> {
        self.data
            .lock()
            .map(|mut d| std::mem::take(&mut d.pr.replies))
            .unwrap_or_default()
    }
    pub fn take_pull_request_notices(&mut self) -> Vec<String> {
        self.data
            .lock()
            .map(|mut d| std::mem::take(&mut d.pr.notices))
            .unwrap_or_default()
    }

    pub fn open_pull_request_inbox(&mut self) -> bool {
        self.close_actions_menu();
        self.close_branch_menu();
        let Ok(mut data) = self.data.lock() else {
            return false;
        };
        let Some(root) = data.repo_root.clone() else {
            return false;
        };
        data.pr.opens.push(PullRequestOpen::Inbox { root });
        data.pr.dirty = true;
        true
    }

    pub fn start_pull_request_creation(&mut self) -> bool {
        self.close_actions_menu();
        let request = {
            let Ok(mut data) = self.data.lock() else {
                return false;
            };
            if data.pr.pending {
                if data.pr.discovering {
                    data.pr.open_create = true;
                    return true;
                }
                data.pr
                    .notices
                    .push("Another pull request operation is still running.".into());
                return false;
            }
            data.pr.open_create = true;
            PullRequestRequest::Discover {
                branch: data.branch.clone(),
            }
        };
        if self.request_pull_request(request) {
            return true;
        }
        if let Ok(mut data) = self.data.lock() {
            data.pr.open_create = false;
            data.pr
                .notices
                .push("Connect the workspace before creating a pull request.".into());
        }
        false
    }

    pub fn take_pull_request_opens(&mut self) -> Vec<PullRequestOpen> {
        self.data
            .lock()
            .map(|mut d| {
                if d.pr.pending {
                    Vec::new()
                } else {
                    std::mem::take(&mut d.pr.opens)
                }
            })
            .unwrap_or_default()
    }
    pub fn start_pull_request_lookup(&mut self) {
        self.close_actions_menu();
        self.close_branch_menu();
        self.focused = true;
        self.commit_focused = false;
        if let Ok(mut d) = self.data.lock() {
            if d.pr.pending {
                return;
            }
            d.pr.identity = d.repo_root.clone().map(|root| (root, d.branch.clone()));
            d.pr.lookup = Some(String::new());
            d.pr.error = None;
            d.pr.dirty = true;
        }
    }

    pub fn pull_request_lookup_active(&self) -> bool {
        self.data.lock().is_ok_and(|d| d.pr.lookup.is_some())
    }

    pub fn pull_request_lookup_key(&mut self, name: &str, text: Option<&str>) -> bool {
        if !self.pull_request_lookup_active() {
            return false;
        }
        if name == "Enter" {
            return self.activate_pull_request();
        }
        if let Ok(mut d) = self.data.lock() {
            if name == "Escape" {
                d.pr.lookup = None;
            } else if let Some(input) = &mut d.pr.lookup {
                if name == "Backspace" {
                    input.pop();
                } else if let Some(text) = text {
                    let capacity = 21usize.saturating_sub(input.len());
                    let text = text.trim();
                    if text.chars().all(|c| c.is_ascii_digit() || c == '#') {
                        input.extend(text.chars().take(capacity));
                    }
                }
            }
            d.pr.dirty = true;
        }
        true
    }

    pub fn activate_pull_request(&mut self) -> bool {
        let request = {
            let Ok(mut d) = self.data.lock() else {
                return false;
            };
            if d.pr.pending {
                return true;
            }
            if d.repo_root.is_none() {
                return false;
            }
            if let Some(input) = d.pr.lookup.take() {
                if let Some(number) = input
                    .trim()
                    .trim_start_matches('#')
                    .parse::<u64>()
                    .ok()
                    .filter(|n| *n > 0)
                {
                    d.pr.open_read = true;
                    Some(PullRequestRequest::Read { number })
                } else {
                    d.pr.lookup = Some(input);
                    d.pr.error = Some("Enter a pull request number".into());
                    return true;
                }
            } else {
                match d.pr.discovered.clone() {
                    Some(PullRequestReply::Discovered {
                        pull_request: Some(pr),
                        ..
                    }) => {
                        d.pr.open_read = true;
                        Some(PullRequestRequest::Read { number: pr.number })
                    }
                    _ => return false,
                }
            }
        };
        if let Some(request) = request {
            if self.request_pull_request(request) {
                return true;
            }
            if let Ok(mut d) = self.data.lock() {
                if d.pr.open_read {
                    d.pr.notices.push(
                        "Connect the workspace before opening a pull request.".into(),
                    );
                }
                d.pr.open_read = false;
            }
            false
        } else {
            true
        }
    }
    pub(super) fn discover_pull_request(&mut self) {
        if !self.visible || self.io.as_ref().is_some_and(|io| !io.pull_requests_ready()) {
            return;
        }
        let request = {
            let Ok(mut d) = self.data.lock() else {
                return;
            };
            let Some(root) = d.repo_root.clone() else {
                return;
            };
            let identity = (root, d.branch.clone());
            if d.pr.identity.as_ref() != Some(&identity) {
                let root_changed =
                    d.pr.identity
                        .as_ref()
                        .is_none_or(|(root, _)| root != &identity.0);
                let action_pending = d.pr.pending && !d.pr.discovering;
                if root_changed {
                    d.pr.generation = d.pr.generation.wrapping_add(1);
                    d.pr.pending = false;
                    d.pr.open_read = false;
                    d.pr.open_create = false;
                    d.pr.notices.clear();
                    d.pr.opens.retain(|open| {
                        matches!(open, PullRequestOpen::Inbox { root } if root == &identity.0)
                    });
                    d.pr.replies.clear();
                } else if !action_pending {
                    d.pr.pending = false;
                }
                // Checkout changes the branch before its acknowledgement arrives.
                // Invalidate discovery, not an explicit action on the same repository.
                d.pr.identity = Some(identity);
                d.pr.last = None;
                d.pr.discovered = None;
                d.pr.lookup = None;
                d.pr.error = None;
            }
            if d.pr.lookup.is_some()
                || d.pr.pending
                || !d.pr.replies.is_empty()
                || !d.pr.opens.is_empty()
                || d.pr.last.is_some_and(|t| t.elapsed().as_secs() < 60)
            {
                return;
            }
            d.pr.last = Some(Instant::now());
            PullRequestRequest::Discover {
                branch: d.branch.clone(),
            }
        };
        self.request_pull_request(request);
    }
    pub(super) fn pull_request_row_visible(&self) -> bool {
        self.data.lock().is_ok_and(|mut data| {
            data.pr.dirty = false;
            let visible = data.pr.lookup.is_some()
                || matches!(
                    &data.pr.discovered,
                    Some(PullRequestReply::Discovered {
                        pull_request: Some(_),
                        ..
                    })
                );
            if !visible {
                data.pr.rect = None;
                data.pr.hovered = false;
            }
            visible
        })
    }

    pub(super) fn pull_request_label(&self) -> String {
        self.data
            .lock()
            .map(|mut d| {
                d.pr.dirty = false;
                // The row represents an actual linked PR, never discovery state.
                if let Some(input) = &d.pr.lookup {
                    return format!("Open PR #{}", input.trim_start_matches('#'));
                }
                match &d.pr.discovered {
                    Some(PullRequestReply::Discovered {
                        pull_request: Some(p),
                        ..
                    }) => format!("#{}  {}", p.number, p.title),
                    _ => String::new(),
                }
            })
            .unwrap_or_default()
    }
    pub(super) fn pull_request_hovered(&self) -> bool {
        self.visible
            && !self.branch_menu_open
            && !self.actions_menu_is_open()
            && self.data.lock().is_ok_and(|d| d.pr.hovered)
    }

    pub(super) fn update_pull_request_hover(&mut self, x: f32, y: f32) -> bool {
        let hovered = self.visible
            && !self.branch_menu_open
            && !self.actions_menu_is_open()
            && self.pull_request_hit(x, y);
        if let Ok(mut data) = self.data.lock() {
            let changed = data.pr.hovered != hovered;
            data.pr.hovered = hovered;
            return changed;
        }
        false
    }

    pub(super) fn pull_request_hit(&self, x: f32, y: f32) -> bool {
        self.visible
            && self
                .data
                .lock()
                .ok()
                .and_then(|d| d.pr.rect)
                .is_some_and(|r| r.contains(x, y))
    }
}
