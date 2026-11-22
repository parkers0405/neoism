//! What the Git panel does next: the one smart action (`resolve`, after t3code's
//! `resolveQuickAction`), the chain of daemon steps behind it, and the branch picker.

use super::*;
use neoism_protocol::pull_request::{PullRequestReply, PullRequestRequest, PullRequestSummary};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum Tab { #[default] Changes, Pulls, Issues }

/// The branch against its upstream and the repository's default branch.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Sync {
    pub ahead: u32,
    pub behind: u32,
    pub default_branch: Option<String>,
    pub has_remote: bool,
    pub head: Option<String>,
}

#[derive(Clone, Debug, PartialEq)]
pub enum Step { StageAll, Commit(String), Push, Publish, Pull, Fetch, Sync, CreatePr, Discover }

impl Step {
    /// What the panel says while the step runs.
    pub fn status(&self) -> &'static str {
        match self {
            Step::StageAll => "Staging changes\u{2026}",
            Step::Commit(_) => "Committing\u{2026}",
            Step::Push => "Pushing\u{2026}",
            Step::Publish => "Publishing branch\u{2026}",
            Step::Pull => "Pulling latest changes\u{2026}",
            Step::Fetch => "Fetching\u{2026}",
            Step::Sync | Step::Discover => "Checking the remote\u{2026}",
            Step::CreatePr => "Creating pull request\u{2026}",
        }
    }
}

/// The main button: its label, the steps it runs (none when it cannot run) and why not.
#[derive(Debug, PartialEq)]
pub struct Quick { pub label: &'static str, pub steps: Vec<Step>, pub hint: Option<&'static str> }

impl Quick {
    fn run(label: &'static str, steps: Vec<Step>) -> Quick { Quick { label, steps, hint: None } }
    fn off(label: &'static str, hint: &'static str) -> Quick { Quick { label, steps: vec![], hint: Some(hint) } }
}

impl State {
    pub(super) fn on_default_branch(&self) -> bool {
        self.sync.as_ref().and_then(|s| s.default_branch.as_deref()).is_some_and(|d| Some(d) == self.branch.as_deref())
    }

    /// The commit message to use: what was typed, or one written from the changed files.
    pub(super) fn commit_message(&self) -> String {
        let typed = self.message.text.trim();
        if !typed.is_empty() { return typed.to_owned(); }
        let staged: Vec<&GitFileChange> = self.files.iter().filter(|f| f.staged).collect();
        let files: Vec<&GitFileChange> = if staged.is_empty() { self.files.iter().collect() } else { staged };
        let name = |f: &GitFileChange| f.path.rsplit('/').next().unwrap_or(&f.path).to_owned();
        let verb = |f: &GitFileChange| match f.status {
            GitChangeStatus::Added | GitChangeStatus::Untracked => "Add",
            GitChangeStatus::Deleted => "Remove",
            _ => "Update",
        };
        match files.as_slice() {
            [] => "Update".into(),
            [one] => format!("{} {}", verb(one), name(one)),
            many if many.iter().all(|f| verb(f) == verb(many[0])) => format!("{} {} files", verb(many[0]), many.len()),
            many => format!("Update {} files", many.len()),
        }
    }

    fn commit_steps(&self) -> Vec<Step> {
        let mut steps = Vec::new();
        if self.totals.2 == 0 { steps.push(Step::StageAll); }
        steps.push(Step::Commit(self.commit_message()));
        steps
    }

    fn push_step(&self) -> Step { if self.upstream.is_some() { Step::Push } else { Step::Publish } }

    /// The next sensible thing, as t3code's `resolveQuickAction` decides it.
    pub fn quick(&self) -> Quick {
        if !self.loaded { return Quick::off("Commit", "Loading repository status\u{2026}"); }
        if self.branch.is_none() { return Quick::off("Commit", "Not on a branch. Check out a branch to commit."); }
        let sync = self.sync.clone().unwrap_or_default();
        let known = self.sync.is_some();
        let pr_open = self.pr.as_ref().is_some_and(|pr| pr.state.eq_ignore_ascii_case("open"));
        let settled = pr_open || self.on_default_branch();
        if !self.files.is_empty() {
            let mut steps = self.commit_steps();
            if !known || !sync.has_remote || sync.behind > 0 { return Quick::run("Commit", steps); }
            steps.extend([self.push_step(), Step::Sync]);
            if settled { return Quick::run("Commit & push", steps); }
            steps.extend([Step::CreatePr]);
            return Quick::run("Commit, push & PR", steps);
        }
        if !known { return Quick::off("Commit", "Nothing to commit."); }
        if !sync.has_remote { return Quick::off("Commit", "Nothing to commit, and this repository has no remote."); }
        if self.upstream.is_none() { return Quick::run("Publish branch", vec![Step::Publish, Step::Sync]); }
        if sync.ahead > 0 && sync.behind > 0 { return Quick::off("Sync", "Branch has diverged from upstream. Rebase or merge first."); }
        if sync.ahead > 0 {
            return if settled { Quick::run("Push", vec![Step::Push, Step::Sync]) }
                else { Quick::run("Push & create PR", vec![Step::Push, Step::Sync, Step::CreatePr]) };
        }
        if sync.behind > 0 { return Quick::run("Pull", vec![Step::Pull, Step::Sync]); }
        if !settled && self.discovered { return Quick::run("Create PR", vec![Step::Sync, Step::CreatePr]); }
        Quick::off("Commit", "Branch is up to date. No action needed.")
    }

    /// The chevron's menu: every action, with the reason a disabled one cannot run.
    pub fn menu(&self) -> Vec<(&'static str, Vec<Step>, Option<&'static str>)> {
        let sync = self.sync.clone().unwrap_or_default();
        let remote = self.sync.as_ref().is_none_or(|s| s.has_remote);
        let no_remote = "This repository has no remote.";
        let commit = if self.files.is_empty() { (vec![], Some("Worktree is clean. Make changes before committing.")) } else { (self.commit_steps(), None) };
        let push = if !remote { (vec![], Some(no_remote)) }
            else if sync.behind > 0 { (vec![], Some("Behind upstream. Pull first.")) }
            else if self.upstream.is_some() && self.sync.is_some() && sync.ahead == 0 { (vec![], Some("Nothing to push.")) }
            else { (vec![self.push_step(), Step::Sync], None) };
        let pull = if !remote || self.upstream.is_none() { (vec![], Some("This branch has no upstream to pull from.")) } else { (vec![Step::Pull, Step::Sync], None) };
        let fetch = if remote { (vec![Step::Fetch, Step::Sync], None) } else { (vec![], Some(no_remote)) };
        let mut items = vec![("Commit", commit.0, commit.1), ("Push", push.0, push.1), ("Pull", pull.0, pull.1), ("Fetch", fetch.0, fetch.1)];
        if !self.pr.as_ref().is_some_and(|pr| pr.state.eq_ignore_ascii_case("open")) {
            let create = if !remote { (vec![], Some(no_remote)) }
                else if self.on_default_branch() { (vec![], Some("Switch to a feature branch to open a pull request.")) }
                else if !self.files.is_empty() { (vec![], Some("Commit your changes first.")) }
                else { (vec![self.push_step(), Step::Sync, Step::CreatePr], None) };
            items.push(("Create PR", create.0, create.1));
        }
        items
    }
}

impl App {
    /// Start a chain of steps; each runs when the one before it succeeds.
    pub fn git_run(&mut self, steps: Vec<Step>) {
        if steps.is_empty() || self.git_panel.pending.is_some() { return; }
        self.git_panel.action_menu = false;
        self.git_panel.branch_menu = false;
        self.git_panel.error = None;
        self.git_panel.notice = None;
        self.git_panel.plan = steps.into();
        self.git_advance();
    }

    /// Run the next step, or finish and look at the remote again.
    pub(super) fn git_advance(&mut self) {
        if self.git_panel.pending.is_some() { return; }
        let Some(step) = self.git_panel.plan.pop_front() else {
            if self.git_panel.running.take().is_some() { self.git_look(); }
            return;
        };
        self.git_panel.running = Some(step.status());
        let (message, kind) = match &step {
            Step::StageAll => (GitClientMessage::StageAll, Operation::Write),
            Step::Commit(message) => {
                self.git_panel.subject = message.lines().next().unwrap_or_default().to_owned();
                (GitClientMessage::Commit { message: message.clone() }, Operation::Commit(self.git_panel.message.text.clone()))
            }
            Step::Push => (GitClientMessage::Push, Operation::Write),
            Step::Publish => (GitClientMessage::Publish, Operation::Write),
            Step::Pull => (GitClientMessage::Pull, Operation::Write),
            Step::Fetch => (GitClientMessage::Fetch, Operation::Write),
            Step::Sync => (GitClientMessage::Sync, Operation::Remote),
            Step::Discover => (GitClientMessage::PullRequest { request: PullRequestRequest::Discover { branch: None } }, Operation::Remote),
            Step::CreatePr => {
                let state = &self.git_panel;
                let (Some(branch), Some(sync)) = (state.branch.clone(), state.sync.clone()) else { return self.git_fail("The branch could not be read; refresh and try again."); };
                let (Some(base), Some(head)) = (sync.default_branch, sync.head) else { return self.git_fail("This repository's default branch is unknown, so a pull request has no base."); };
                let title = if state.subject.is_empty() { branch.replace(['-', '_', '/'], " ") } else { state.subject.clone() };
                (GitClientMessage::PullRequest { request: PullRequestRequest::Create { title, body: String::new(), base, draft: false, expected_branch: branch, expected_head: head } }, Operation::Remote)
            }
        };
        self.git_panel.pending = self.git_request(message, kind);
        if self.git_panel.pending.is_none() { self.git_panel.plan.clear(); self.git_panel.running = None; }
    }

    pub(super) fn git_fail(&mut self, message: impl Into<String>) {
        self.git_panel.error = Some(message.into());
        self.git_panel.plan.clear();
        self.git_panel.running = None;
    }

    /// Ask, without blocking the panel, how the branch stands and whether it has a pull request.
    pub(super) fn git_look(&mut self) {
        self.git_panel.aux.clear();
        for message in [GitClientMessage::Sync, GitClientMessage::PullRequest { request: PullRequestRequest::Discover { branch: None } }, GitClientMessage::Branches] {
            if let Some(pending) = self.git_request(message, Operation::Remote) { self.git_panel.aux.push(pending); }
        }
    }

    /// A reply about the remote: from a step of the chain (`step`) or a background look.
    pub(super) fn git_remote_reply(&mut self, message: GitServerMessage, step: bool) {
        match message {
            GitServerMessage::Sync { ahead, behind, default_branch, has_remote, head } => {
                self.git_panel.sync = Some(Sync { ahead, behind, default_branch, has_remote, head });
            }
            GitServerMessage::Branches { branches } => self.git_panel.branches = branches,
            GitServerMessage::PullRequest { reply } => match reply {
                PullRequestReply::Discovered { default_branch, pull_request, .. } => {
                    self.git_panel.pr = pull_request;
                    self.git_panel.discovered = true;
                    if let Some(sync) = &mut self.git_panel.sync { if sync.default_branch.is_none() && !default_branch.is_empty() { sync.default_branch = Some(default_branch); } }
                }
                PullRequestReply::Created { pull_request } => {
                    self.git_panel.notice = Some(format!("Opened pull request #{}", pull_request.number));
                    self.git_panel.pr = Some(pull_request);
                }
                PullRequestReply::Error { message, .. } if step => return self.git_fail(message),
                _ => {}
            },
            GitServerMessage::Error { message } if step => return self.git_fail(message),
            _ => {}
        }
        if step { self.git_advance(); }
    }

    pub fn git_toggle_branches(&mut self) {
        self.git_panel.branch_menu = !self.git_panel.branch_menu;
        self.git_panel.action_menu = false;
        self.git_panel.branch_query = TextEdit::default();
        self.git_panel.branch_query.focused = self.git_panel.branch_menu;
        self.git_panel.message.focused = false;
    }

    pub fn git_checkout(&mut self, branch: &str, create: bool) {
        if self.git_panel.pending.is_some() { return; }
        self.git_panel.branch_menu = false;
        self.git_panel.branch_query = TextEdit::default();
        self.git_panel.error = None;
        let message = if create { GitClientMessage::CreateBranch { name: branch.into() } } else { GitClientMessage::Checkout { branch: branch.into() } };
        self.git_panel.running = Some("Switching branch\u{2026}");
        // The pull request and the counts belong to the branch being left.
        self.git_panel.pr = None;
        self.git_panel.discovered = false;
        self.git_panel.sync = None;
        self.git_panel.pending = self.git_request(message, Operation::Write);
        if self.git_panel.pending.is_none() { self.git_panel.running = None; }
    }
}

/// A fixture pull request for the headless scenes.
pub(super) fn fixture_pr() -> PullRequestSummary {
    PullRequestSummary {
        number: 128, title: "Add native Git review".into(), url: "https://github.com/neoism/neoism/pull/128".into(), author: "parker".into(),
        state: "OPEN".into(), draft: false, base_branch: "main".into(), head_branch: "rune-frontend".into(),
        base_sha: "a".repeat(40), head_sha: "b".repeat(40), review_decision: "REVIEW_REQUIRED".into(), checks_state: "SUCCESS".into(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn state(files: usize, sync: Option<Sync>, upstream: bool) -> State {
        let mut state = State { loaded: true, branch: Some("topic".into()), upstream: upstream.then(|| "origin/topic".into()), sync, discovered: true, ..State::default() };
        state.set_files((0..files).map(|i| GitFileChange { path: format!("src/f{i}.rs"), status: GitChangeStatus::Modified, additions: 1, deletions: 0, staged: false }).collect(), Some("topic".into()));
        state
    }
    fn remote(ahead: u32, behind: u32) -> Option<Sync> {
        Some(Sync { ahead, behind, default_branch: Some("main".into()), has_remote: true, head: Some("c".repeat(40)) })
    }

    #[test]
    fn the_main_action_follows_the_state_of_the_branch() {
        // Changes on a feature branch go all the way to a pull request, staging first.
        let q = state(2, remote(0, 0), true).quick();
        assert_eq!(q.label, "Commit, push & PR");
        assert_eq!(q.steps, vec![Step::StageAll, Step::Commit("Update 2 files".into()), Step::Push, Step::Sync, Step::CreatePr]);
        // Behind, or with no remote, or before the remote is known, it only commits.
        assert_eq!(state(1, remote(0, 2), true).quick().label, "Commit");
        assert_eq!(state(1, None, true).quick().label, "Commit");
        // On the default branch, or with a pull request already open, there is no PR step.
        let mut main = state(1, remote(0, 0), true); main.branch = Some("main".into());
        assert_eq!(main.quick().label, "Commit & push");
        let mut open = state(1, remote(0, 0), true); open.pr = Some(fixture_pr());
        assert_eq!(open.quick().label, "Commit & push");
        // Clean trees: publish, push, pull, diverged, create, up to date.
        assert_eq!(state(0, remote(0, 0), false).quick().label, "Publish branch");
        assert_eq!(state(0, remote(2, 0), true).quick().label, "Push & create PR");
        assert_eq!(state(0, remote(0, 3), true).quick().label, "Pull");
        let diverged = state(0, remote(1, 1), true).quick();
        assert!(diverged.steps.is_empty() && diverged.hint.is_some_and(|h| h.contains("diverged")));
        assert_eq!(state(0, remote(0, 0), true).quick().label, "Create PR");
        let mut done = state(0, remote(0, 0), true); done.pr = Some(fixture_pr());
        assert!(done.quick().steps.is_empty());
    }

    #[test]
    fn an_empty_message_is_written_from_the_files() {
        let mut one = state(1, None, true);
        assert_eq!(one.commit_message(), "Update f0.rs");
        one.files[0].status = GitChangeStatus::Untracked;
        assert_eq!(one.commit_message(), "Add f0.rs");
        one.message = TextEdit::new("  Fix the cursor \n");
        assert_eq!(one.commit_message(), "Fix the cursor");
        // Only the staged files are named when some are staged.
        let mut some = state(3, None, true);
        some.files[1].staged = true;
        assert_eq!(some.commit_message(), "Update f1.rs");
    }

    #[test]
    fn the_menu_says_why_an_action_cannot_run() {
        let clean = state(0, remote(0, 0), true);
        let items = clean.menu();
        let find = |name: &str| items.iter().find(|i| i.0 == name).unwrap();
        assert!(find("Commit").1.is_empty() && find("Commit").2.is_some());
        assert!(find("Push").2.is_some_and(|h| h.contains("Nothing to push")));
        assert!(!find("Pull").1.is_empty() && !find("Fetch").1.is_empty());
        let mut open = state(0, remote(1, 0), true); open.pr = Some(fixture_pr());
        assert!(!open.menu().iter().any(|i| i.0 == "Create PR"), "an open pull request needs no second one");
    }
}
