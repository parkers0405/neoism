//! Sub-agent families: which conversation each child session belongs to, the ordered roster
//! of a root's children, and the `← Back` / `← Prev` / `Next →` order between them.
//! Rules from `gpuix/src/agent/family.ts` (routing and roster upkeep) and
//! `shared/src/panels/agent_pane/conversation_header.rs` (navigation order).

use neoism_protocol::agent::{AgentRuntimeSnapshot, AgentServerMessage, HistoryMessage, SubagentStatus};
use serde_json::Value;
use std::collections::HashMap;

#[derive(Clone, Debug, PartialEq)]
pub struct Member {
    pub id: String,
    pub parent: String,
    pub title: String,
    pub agent: Option<String>,
    pub status: String,
    pub depth: usize,
    pub started_at: Option<u64>,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Navigation<'a> {
    pub previous: Option<&'a str>,
    pub next: Option<&'a str>,
    /// `previous` is the root conversation ("← Back") rather than a sibling ("← Prev").
    pub previous_is_main: bool,
}

/// Snapshot ordering per root and source: an older revision of the same execution is stale,
/// and a catalog execution that was replaced never comes back.
#[derive(Clone, Debug, Default)]
struct Version {
    execution: Option<Value>,
    revision: u64,
    retired: Vec<Value>,
}

#[derive(Default)]
pub struct Families {
    parents: HashMap<String, String>,
    roots: HashMap<String, String>,
    rosters: HashMap<String, Vec<Member>>,
    versions: HashMap<String, Version>,
    /// Children seen at work during this run of the app. A child only named by an old task
    /// card was never seen running and is not listed in the sidebar.
    live: std::collections::HashSet<String>,
    /// When a child seen at work finished.
    ended: HashMap<String, std::time::Instant>,
}

/// A finished child stays in the sidebar this long after it was seen to finish, a failed one
/// longer (`side_panel.rs` completed / stopped linger).
const DONE_LINGER: std::time::Duration = std::time::Duration::from_secs(7);
const FAILED_LINGER: std::time::Duration = std::time::Duration::from_secs(60);

impl Member {
    pub fn working(&self) -> bool { matches!(self.status.as_str(), "running" | "blocked") }
    fn linger(&self) -> std::time::Duration { if self.status == "failed" { FAILED_LINGER } else { DONE_LINGER } }
}

/// One incoming branch, from a snapshot or a lifecycle update.
struct Branch {
    id: String,
    parent: Option<String>,
    title: Option<String>,
    agent: Option<String>,
    status: Option<String>,
    started_at: Option<u64>,
}

impl Families {
    /// The root conversation `session` belongs to (itself when it has no recorded parent).
    pub fn root_of<'a>(&'a self, session: &'a str) -> &'a str {
        if let Some(root) = self.roots.get(session).filter(|r| !r.is_empty()) { return root; }
        let mut root = session;
        let mut hops = 0;
        while let Some(parent) = self.parents.get(root).filter(|p| !p.is_empty()) {
            root = parent;
            hops += 1;
            if hops > 64 { break; }
        }
        root
    }

    pub fn is_child(&self, session: &str) -> bool { self.root_of(session) != session }

    pub fn roster(&self, root: &str) -> &[Member] {
        self.rosters.get(root).map(Vec::as_slice).unwrap_or(&[])
    }

    /// The children the conversations sidebar lists under `root`: the ones at work, and the
    /// ones seen to finish a moment ago (`CatalogFamily` rows in the original).
    pub fn sidebar_children(&self, root: &str) -> Vec<&Member> {
        self.roster(root).iter().filter(|m| self.live.contains(&m.id)
            && (m.working() || self.ended.get(&m.id).is_some_and(|at| at.elapsed() < m.linger()))).collect()
    }

    /// Whether `session` is a sub-agent seen at work in this run and not yet seen to stop. A
    /// child only named by an old task card has no known state and is not "working".
    pub fn working_now(&self, session: &str) -> bool {
        self.live.contains(session) && self.member(session).is_some_and(Member::working)
    }

    /// A sub-agent stopped (its session went idle, or its task card settled) without the
    /// family saying so yet. Returns whether that changed anything.
    pub fn finish(&mut self, session: &str, failed: bool) -> bool {
        let Some(root) = self.roots.get(session).cloned() else { return false };
        let Some(member) = self.rosters.get_mut(&root).and_then(|rows| rows.iter_mut().find(|m| m.id == session)) else { return false };
        if !member.working() { return false; }
        member.status = if failed { "failed" } else { "completed" }.into();
        if self.live.contains(session) { self.ended.insert(session.to_owned(), std::time::Instant::now()); }
        true
    }

    /// The catalog started over (`CatalogReset`): its next snapshots are a fresh baseline,
    /// whatever their revision, and only the families it names again are still at work.
    pub fn catalog_reset(&mut self) {
        self.versions.retain(|key, _| !key.ends_with(":CatalogFamily"));
        let stale: Vec<String> = self.rosters.values().flatten().filter(|m| m.working() && self.live.contains(&m.id)).map(|m| m.id.clone()).collect();
        for id in stale { self.finish(&id, false); }
    }

    /// Every sub-agent known to have stopped: `(parent, child, failed)`.
    pub fn finished(&self) -> Vec<(String, String, bool)> {
        self.rosters.values().flatten().filter(|m| !m.working() && self.live.contains(&m.id))
            .map(|m| (m.parent.clone(), m.id.clone(), m.status == "failed")).collect()
    }

    /// When the next lingering child leaves the sidebar.
    pub fn next_expiry(&self) -> Option<std::time::Duration> {
        self.rosters.values().flatten().filter(|m| !m.working())
            .filter_map(|m| self.ended.get(&m.id).map(|at| m.linger().saturating_sub(at.elapsed())))
            .filter(|left| !left.is_zero()).min()
    }

    /// How many children the sidebar lists in all (it re-runs when this changes).
    pub fn sidebar_count(&self) -> usize {
        self.rosters.keys().map(|root| self.sidebar_children(root).len()).sum()
    }

    pub fn member(&self, session: &str) -> Option<&Member> {
        self.roster(self.root_of(session)).iter().find(|m| m.id == session)
    }

    /// Header links for viewing `viewed`: previous sibling (or the root), next sibling, no wrap.
    pub fn navigation<'a>(&'a self, viewed: &'a str) -> Navigation<'a> {
        let root = self.root_of(viewed);
        if viewed.is_empty() || root == viewed { return Navigation::default(); }
        let mut previous = root;
        let mut rows = self.roster(root).iter().filter(|m| !m.id.is_empty() && m.id != root);
        while let Some(row) = rows.next() {
            if row.id == viewed {
                let next = rows.find(|m| m.id != viewed).map(|m| m.id.as_str());
                return Navigation { previous: Some(previous), next, previous_is_main: previous == root };
            }
            previous = &row.id;
        }
        Navigation { previous: Some(root), next: None, previous_is_main: true }
    }

    /// Fold one daemon event into the families. Returns whether anything changed.
    pub fn note(&mut self, message: &AgentServerMessage) -> bool {
        match message {
            AgentServerMessage::CatalogFamily { session_id, snapshot } => {
                let root = snapshot["rootSessionId"].as_str().filter(|r| !r.is_empty()).unwrap_or(session_id).to_owned();
                let branches = snapshot["branches"].as_array().into_iter().flatten().map(|raw| Branch {
                    id: str_field(raw, &["sessionId", "session_id"]).unwrap_or_default(),
                    parent: str_field(raw, &["parentSessionId", "parent_session_id"]),
                    title: str_field(raw, &["title"]),
                    agent: str_field(raw, &["agent"]),
                    status: str_field(raw, &["status"]).map(|status| match status.as_str() {
                        "outstanding" if raw["permissionPending"] == true => "blocked".into(),
                        "outstanding" => "running".into(),
                        _ => status,
                    }),
                    started_at: raw["startedAt"].as_u64().or(raw["started_at"].as_u64()),
                }).collect();
                let revision = snapshot["familyRevision"].as_u64().unwrap_or(0);
                let execution = snapshot.get("executionId").cloned().filter(|v| !v.is_null());
                if !self.accept_version(&root, "CatalogFamily", execution, revision) { return false; }
                self.merge(&root, branches, true, false)
            }
            AgentServerMessage::RuntimeSnapshot { snapshot, .. } => self.note_runtime(snapshot),
            AgentServerMessage::SubagentUpdate { session_id, status, title, agent, started_at, parent_session_id, root_session_id, .. } => {
                let parent = parent_session_id.clone().filter(|p| !p.is_empty()).or_else(|| self.parents.get(session_id).cloned());
                let root = root_session_id.clone().filter(|r| !r.is_empty())
                    .or_else(|| parent.as_deref().map(|p| self.root_of(p).to_owned()))
                    .or_else(|| self.roots.get(session_id).cloned());
                let Some(root) = root else { return false };
                let status = match status {
                    SubagentStatus::Running => "running",
                    SubagentStatus::Blocked => "blocked",
                    SubagentStatus::Completed => "completed",
                    SubagentStatus::Failed => "failed",
                };
                let branch = Branch { id: session_id.clone(), parent, title: title.clone(), agent: agent.clone(), status: Some(status.into()), started_at: *started_at };
                self.merge(&root, vec![branch], false, false)
            }
            AgentServerMessage::SubagentMetadata { session_id, title, agent, parent_session_id } => {
                // Metadata may rename a known row but never creates one.
                if !self.parents.contains_key(session_id) && !self.roots.contains_key(session_id) { return false; }
                let parent = parent_session_id.clone().filter(|p| !p.is_empty()).or_else(|| self.parents.get(session_id).cloned());
                let root = parent.as_deref().map(|p| self.root_of(p).to_owned()).or_else(|| self.roots.get(session_id).cloned());
                let Some(root) = root else { return false };
                let branch = Branch { id: session_id.clone(), parent, title: title.clone(), agent: agent.clone(), status: None, started_at: None };
                self.merge(&root, vec![branch], false, true)
            }
            _ => false,
        }
    }

    fn note_runtime(&mut self, snapshot: &AgentRuntimeSnapshot) -> bool {
        let root = snapshot.root_session_id.clone();
        if root.is_empty() || !self.accept_version(&root, "RuntimeSnapshot", None, snapshot.family_revision) { return false; }
        let branches = snapshot.branches.iter().map(|b| Branch {
            id: b.session_id.clone(), parent: Some(b.parent_session_id.clone()).filter(|p| !p.is_empty()),
            title: None, agent: None, status: Some(b.status.clone()), started_at: b.started_at,
        }).collect();
        self.merge(&root, branches, snapshot.branches_authoritative, false)
    }

    /// Children named by `task` cards in a history page (`task_id: <id>` lines), so an old
    /// conversation can be navigated before any family snapshot arrives.
    pub fn seed_from_history(&mut self, root: &str, messages: &[HistoryMessage]) -> bool {
        let branches: Vec<Branch> = messages.iter().filter(|m| m.tool == "task").filter_map(|m| {
            let id = task_id(&m.detail, &m.text)?;
            Some(Branch { id, parent: Some(root.to_owned()), title: Some(m.title.trim().to_owned()).filter(|t| !t.is_empty()), agent: None, status: None, started_at: None })
        }).collect();
        if branches.is_empty() { return false; }
        self.merge(root, branches, false, false)
    }

    fn accept_version(&mut self, root: &str, source: &str, execution: Option<Value>, revision: u64) -> bool {
        let previous = self.versions.entry(format!("{root}:{source}")).or_default();
        let catalog = source == "CatalogFamily";
        let same_execution = previous.execution == execution;
        if same_execution && revision < previous.revision { return false; }
        if catalog && execution.as_ref().is_some_and(|e| previous.retired.contains(e)) { return false; }
        if !same_execution {
            if let Some(old) = previous.execution.take() { previous.retired.push(old); }
        }
        previous.execution = execution;
        previous.revision = revision;
        true
    }

    fn merge(&mut self, root: &str, incoming: Vec<Branch>, authoritative: bool, metadata_only: bool) -> bool {
        let previous = self.rosters.get(root).cloned().unwrap_or_default();
        let mut rows: Vec<Member> = if authoritative { Vec::new() } else { previous.clone() };
        for branch in incoming {
            if branch.id.is_empty() || branch.id == root { continue; }
            let parent = branch.parent.or_else(|| self.parents.get(&branch.id).cloned()).unwrap_or_else(|| root.to_owned());
            self.parents.insert(branch.id.clone(), parent.clone());
            self.roots.insert(branch.id.clone(), root.to_owned());
            let old = previous.iter().find(|m| m.id == branch.id);
            // Seen at work, or seen to stop working: what the sidebar's child rows go by.
            if let Some(status) = branch.status.as_deref().map(str::to_lowercase) {
                if matches!(status.as_str(), "running" | "blocked") {
                    self.live.insert(branch.id.clone());
                    self.ended.remove(&branch.id);
                } else if self.live.contains(&branch.id) && old.is_some_and(Member::working) {
                    self.ended.insert(branch.id.clone(), std::time::Instant::now());
                }
            }
            let row = Member {
                title: branch.title.filter(|t| !t.is_empty()).or_else(|| old.map(|o| o.title.clone())).unwrap_or_else(|| branch.id.clone()),
                status: branch.status.map(|s| s.to_lowercase()).or_else(|| old.map(|o| o.status.clone())).unwrap_or_else(|| "running".into()),
                agent: branch.agent.or_else(|| old.and_then(|o| o.agent.clone())),
                started_at: branch.started_at.or_else(|| old.and_then(|o| o.started_at)),
                depth: 1,
                id: branch.id,
                parent,
            };
            match rows.iter_mut().find(|m| m.id == row.id) {
                Some(existing) => *existing = row,
                None if !metadata_only => rows.push(row),
                None => {}
            }
        }
        for row in &mut rows {
            let mut depth = 1;
            let mut parent = row.parent.as_str();
            let mut hops = 0;
            while !parent.is_empty() && parent != root && hops < 64 {
                depth += 1;
                hops += 1;
                parent = self.parents.get(parent).map(String::as_str).unwrap_or(root);
            }
            row.depth = depth;
        }
        let changed = rows != previous;
        if changed { self.rosters.insert(root.to_owned(), rows); }
        changed
    }
}

fn str_field(raw: &Value, keys: &[&str]) -> Option<String> {
    keys.iter().find_map(|k| raw[*k].as_str()).map(str::to_owned)
}

/// The child session a `task` card launched (`message_policy::task_id_from_text`).
pub fn task_id(detail: &str, text: &str) -> Option<String> {
    detail.lines().chain(text.lines()).find_map(|line| {
        line.trim().strip_prefix("task_id:").and_then(|rest| rest.split_whitespace().next()).map(str::to_owned)
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn family() -> Families {
        let mut f = Families::default();
        let snapshot = json!({"rootSessionId": "main", "familyRevision": 1, "branches": [
            {"sessionId": "first", "parentSessionId": "main", "status": "completed", "title": "First"},
            {"sessionId": "nested", "parentSessionId": "first", "status": "running"},
            {"sessionId": "last", "parentSessionId": "main", "status": "outstanding", "permissionPending": true},
        ]});
        assert!(f.note(&AgentServerMessage::CatalogFamily { session_id: "main".into(), snapshot }));
        f
    }

    #[test]
    fn navigation_follows_roster_order_without_wrapping() {
        let f = family();
        assert_eq!(f.navigation("first"), Navigation { previous: Some("main"), next: Some("nested"), previous_is_main: true });
        assert_eq!(f.navigation("nested"), Navigation { previous: Some("first"), next: Some("last"), previous_is_main: false });
        assert_eq!(f.navigation("last"), Navigation { previous: Some("nested"), next: None, previous_is_main: false });
        assert_eq!(f.navigation("main"), Navigation::default());
    }

    #[test]
    fn depth_status_and_roots() {
        let f = family();
        assert_eq!(f.root_of("nested"), "main");
        assert_eq!(f.member("nested").map(|m| m.depth), Some(2));
        assert_eq!(f.member("last").map(|m| m.status.as_str()), Some("blocked"));
        assert_eq!(f.member("first").map(|m| m.title.as_str()), Some("First"));
    }

    #[test]
    fn stale_revisions_and_unknown_metadata_are_ignored() {
        let mut f = family();
        let stale = json!({"rootSessionId": "main", "familyRevision": 0, "branches": []});
        assert!(!f.note(&AgentServerMessage::CatalogFamily { session_id: "main".into(), snapshot: stale }));
        assert_eq!(f.roster("main").len(), 3);
        let unknown = AgentServerMessage::SubagentMetadata { session_id: "ghost".into(), title: Some("x".into()), agent: None, parent_session_id: None };
        assert!(!f.note(&unknown));
    }

    #[test]
    fn task_ids_parse_from_card_text() {
        assert_eq!(task_id("ok\ntask_id: ses_123 (resume)", ""), Some("ses_123".into()));
        assert_eq!(task_id("", "nothing"), None);
    }
}
