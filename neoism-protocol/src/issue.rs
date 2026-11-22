//! Hosted issue operations. As with pull requests, the repository is resolved by the host
//! from the workspace, never named by a client.

use crate::pull_request::PullRequestErrorKind;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct IssueSummary {
    pub number: u64,
    pub title: String,
    pub url: String,
    pub author: String,
    /// `OPEN` or `CLOSED`, as the host spells it.
    pub state: String,
    pub labels: Vec<String>,
    pub assignees: Vec<String>,
    pub comments: u32,
    pub updated_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct IssueComment {
    pub author: String,
    pub body: String,
    pub created_at: String,
    pub url: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct IssueDetail {
    pub summary: IssueSummary,
    pub body: String,
    pub created_at: String,
    pub comments: Vec<IssueComment>,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Default)]
pub enum IssueFilter {
    #[default]
    Open,
    Closed,
    All,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub enum IssueRequest {
    /// `query` is the host's own search text; `mine` keeps issues assigned to the viewer.
    List { query: String, filter: IssueFilter, mine: bool },
    Read { number: u64 },
    Comment { number: u64, body: String },
}

impl IssueRequest {
    pub fn is_mutation(&self) -> bool { matches!(self, Self::Comment { .. }) }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum IssueReply {
    Listed { items: Vec<IssueSummary> },
    Detail { detail: IssueDetail },
    Commented { number: u64 },
    Error { kind: PullRequestErrorKind, message: String },
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::git::{GitClientMessage, GitServerMessage};

    #[test]
    fn requests_roundtrip_and_only_comment_mutates() {
        for request in [
            IssueRequest::List { query: "label:bug".into(), filter: IssueFilter::Open, mine: true },
            IssueRequest::Read { number: 7 },
            IssueRequest::Comment { number: 7, body: "Looking".into() },
        ] {
            let message = GitClientMessage::Issue { request: request.clone() };
            let encoded = serde_json::to_string(&message).unwrap();
            let decoded: GitClientMessage = serde_json::from_str(&encoded).unwrap();
            assert!(matches!(&decoded, GitClientMessage::Issue { request: r } if *r == request));
            assert_eq!(decoded.is_mutation(), matches!(request, IssueRequest::Comment { .. }));
        }
        let reply = GitServerMessage::Issue { reply: IssueReply::Commented { number: 7 } };
        let encoded = serde_json::to_string(&reply).unwrap();
        assert!(matches!(serde_json::from_str::<GitServerMessage>(&encoded).unwrap(), GitServerMessage::Issue { reply: IssueReply::Commented { number: 7 } }));
    }
}
