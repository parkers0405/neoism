//! Host-only GitHub issues through `gh`, run in the workspace's repository so the host, not
//! the client, decides which repository is read.

use neoism_protocol::issue::*;
use neoism_protocol::pull_request::PullRequestErrorKind;
use serde_json::{json, Value};
use std::path::Path;

type Result<T> = std::result::Result<T, (PullRequestErrorKind, String)>;

const FIELDS: &str = "number,title,url,author,state,labels,assignees,comments,updatedAt";
const LIMIT: &str = "60";

fn text(v: &Value, key: &str) -> String { v.get(key).and_then(Value::as_str).unwrap_or_default().to_owned() }
fn login(v: &Value) -> String { v.get("login").and_then(Value::as_str).unwrap_or_default().to_owned() }
fn names(v: &Value, key: &str, field: &str) -> Vec<String> {
    v.get(key).and_then(Value::as_array).map(|a| a.iter().map(|x| text(x, field)).filter(|s| !s.is_empty()).collect()).unwrap_or_default()
}

fn summary(v: &Value) -> IssueSummary {
    IssueSummary {
        number: v.get("number").and_then(Value::as_u64).unwrap_or_default(),
        title: text(v, "title"),
        url: text(v, "url"),
        author: v.get("author").map(login).unwrap_or_default(),
        state: text(v, "state"),
        labels: names(v, "labels", "name"),
        assignees: names(v, "assignees", "login"),
        comments: v.get("comments").and_then(Value::as_array).map_or(0, |c| c.len() as u32),
        updated_at: text(v, "updatedAt"),
    }
}

fn parse(raw: &str) -> Result<Value> {
    serde_json::from_str(raw).map_err(|_| (PullRequestErrorKind::Failed, "GitHub returned a response that could not be read".to_owned()))
}

fn gh(root: &Path, args: &[&str], input: Option<&Value>) -> Result<String> {
    crate::pull_request::host_run(root, "gh", &args.iter().map(|s| s.to_string()).collect::<Vec<_>>(), input)
}

fn dispatch(root: &Path, request: IssueRequest) -> Result<IssueReply> {
    match request {
        IssueRequest::List { query, filter, mine } => {
            if query.len() > 512 || query.contains('\0') { return Err((PullRequestErrorKind::Invalid, "Search text is too long".into())); }
            let state = match filter { IssueFilter::Open => "open", IssueFilter::Closed => "closed", IssueFilter::All => "all" };
            let search = format!("--search={}", query.trim());
            let mut args = vec!["issue", "list", "--limit", LIMIT, "--state", state, "--json", FIELDS];
            if !query.trim().is_empty() { args.push(&search); }
            if mine { args.extend(["--assignee", "@me"]); }
            let listed = parse(&gh(root, &args, None)?)?;
            Ok(IssueReply::Listed { items: listed.as_array().map(|a| a.iter().map(summary).collect()).unwrap_or_default() })
        }
        IssueRequest::Read { number } => {
            let fields = format!("{FIELDS},body,createdAt");
            let v = parse(&gh(root, &["issue", "view", &number.to_string(), "--json", &fields], None)?)?;
            let comments = v.get("comments").and_then(Value::as_array).map(|a| a.iter().map(|c| IssueComment {
                author: c.get("author").map(login).unwrap_or_default(),
                body: text(c, "body"),
                created_at: text(c, "createdAt"),
                url: text(c, "url"),
            }).collect()).unwrap_or_default();
            Ok(IssueReply::Detail { detail: IssueDetail { summary: summary(&v), body: text(&v, "body"), created_at: text(&v, "createdAt"), comments } })
        }
        IssueRequest::Comment { number, body } => {
            if body.trim().is_empty() || body.len() > 65536 || body.contains('\0') {
                return Err((PullRequestErrorKind::Invalid, "A comment needs text, within the supported limit".into()));
            }
            // `{owner}/{repo}` is filled in by gh from the repository it runs in.
            let path = format!("repos/{{owner}}/{{repo}}/issues/{number}/comments");
            gh(root, &["api", &path, "--method", "POST", "--input", "-"], Some(&json!({ "body": body })))?;
            Ok(IssueReply::Commented { number })
        }
    }
}

pub fn handle_at(root: &Path, request: IssueRequest) -> IssueReply {
    dispatch(root, request).unwrap_or_else(|(kind, message)| IssueReply::Error { kind, message })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn summaries_read_the_host_shape() {
        let v = json!({"number": 12, "title": "Crash on open", "url": "https://example.test/12", "author": {"login": "ada"},
            "state": "OPEN", "labels": [{"name": "bug"}], "assignees": [{"login": "lin"}], "comments": [{}, {}], "updatedAt": "2026-11-20T10:00:00Z"});
        let s = summary(&v);
        assert_eq!((s.number, s.author.as_str(), s.comments), (12, "ada", 2));
        assert_eq!((s.labels, s.assignees), (vec!["bug".to_owned()], vec!["lin".to_owned()]));
    }

    #[test]
    fn empty_and_oversized_text_is_refused_before_any_host_call() {
        let root = std::env::temp_dir();
        for body in [String::new(), "  ".into(), "x".repeat(70_000)] {
            assert!(matches!(handle_at(&root, IssueRequest::Comment { number: 1, body }), IssueReply::Error { kind: PullRequestErrorKind::Invalid, .. }));
        }
        assert!(matches!(handle_at(&root, IssueRequest::List { query: "q".repeat(600), filter: IssueFilter::Open, mine: false }), IssueReply::Error { kind: PullRequestErrorKind::Invalid, .. }));
    }
}
