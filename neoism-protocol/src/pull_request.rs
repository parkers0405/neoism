//! Hosted pull-request operations. Repository identity is resolved by the host,
//! never supplied as an arbitrary API URL by a client.

#[cfg(test)]
mod tests {
    use super::*;
    use crate::git::{GitClientMessage, GitServerMessage};
    #[test]
    fn all_requests_roundtrip_and_delegate_mutation_gate() {
        let requests = [
            PullRequestRequest::Discover { branch: None },
            PullRequestRequest::Read { number: 1 },
            PullRequestRequest::ReadInbox { number: 1 },
            PullRequestRequest::List {
                query: "label:bug fix".into(),
                sort: PullRequestSort::Updated,
                filter: PullRequestFilter::Open,
                cursor: None,
                generation: 42,
            },
            PullRequestRequest::Create {
                title: "Title".into(),
                body: "Body".into(),
                base: "main".into(),
                draft: true,
                expected_branch: "topic".into(),
                expected_head: "a".repeat(40),
            },
            PullRequestRequest::SubmitReview {
                number: 1,
                expected_head: "a".repeat(40),
                body: "Review".into(),
                event: PullRequestReviewEvent::RequestChanges,
                comments: vec![PullRequestInlineComment {
                    path: "file".into(),
                    side: PullRequestSide::Left,
                    line: 2,
                    start_line: Some(1),
                    body: "Comment".into(),
                }],
            },
            PullRequestRequest::ReplyThread {
                number: 1,
                thread_id: "thread".into(),
                body: "Reply".into(),
            },
            PullRequestRequest::ResolveThread {
                number: 1,
                thread_id: "thread".into(),
                resolved: true,
            },
            PullRequestRequest::Checkout {
                number: 1,
                expected_head: "a".repeat(40),
                new_worktree: true,
            },
            PullRequestRequest::Merge {
                number: 1,
                expected_head: "a".repeat(40),
                method: PullRequestMergeMethod::Squash,
            },
        ];
        for (index, request) in requests.into_iter().enumerate() {
            let mutation = index > 3;
            assert_eq!(request.is_mutation(), mutation);
            let msg = GitClientMessage::PullRequest {
                request: request.clone(),
            };
            assert_eq!(msg.is_mutation(), mutation);
            let encoded = serde_json::to_string(&msg).unwrap();
            assert!(encoded.starts_with("{\"PullRequest\":{\"request\":"));
            let decoded: GitClientMessage = serde_json::from_str(&encoded).unwrap();
            assert!(
                matches!(decoded,GitClientMessage::PullRequest { request:decoded } if decoded == request)
            );
        }
    }
    #[test]
    fn inbox_page_and_enum_roundtrips() {
        for sort in [
            PullRequestSort::Updated,
            PullRequestSort::Newest,
            PullRequestSort::Oldest,
        ] {
            for filter in [
                PullRequestFilter::Open,
                PullRequestFilter::Closed,
                PullRequestFilter::Merged,
                PullRequestFilter::All,
            ] {
                let request = PullRequestRequest::List {
                    query: "label:bug".into(),
                    sort,
                    filter,
                    cursor: Some("abc=".into()),
                    generation: 7,
                };
                assert_eq!(
                    serde_json::from_str::<PullRequestRequest>(
                        &serde_json::to_string(&request).unwrap()
                    )
                    .unwrap(),
                    request
                );
            }
        }
        let page = PullRequestPage {
            repository: "owner/repo".into(),
            viewer: "viewer".into(),
            generation: 7,
            items: vec![],
            next_cursor: Some("abc=".into()),
            total_count: 42,
        };
        let reply = PullRequestReply::Listed { page: page.clone() };
        assert!(
            matches!(serde_json::from_str::<PullRequestReply>(&serde_json::to_string(&reply).unwrap()).unwrap(), PullRequestReply::Listed { page: actual } if actual == page)
        );
    }
    #[test]
    fn typed_error_reply_roundtrip() {
        for kind in [
            PullRequestErrorKind::Unsupported,
            PullRequestErrorKind::Unavailable,
            PullRequestErrorKind::Authentication,
            PullRequestErrorKind::Permission,
            PullRequestErrorKind::Stale,
            PullRequestErrorKind::Conflict,
            PullRequestErrorKind::Invalid,
            PullRequestErrorKind::Failed,
        ] {
            let reply = GitServerMessage::PullRequest {
                reply: PullRequestReply::Error {
                    kind,
                    message: "Safe diagnostic".into(),
                },
            };
            let encoded = serde_json::to_string(&reply).unwrap();
            assert!(
                matches!(serde_json::from_str::<GitServerMessage>(&encoded).unwrap(),GitServerMessage::PullRequest { reply:PullRequestReply::Error { kind:actual,.. } } if actual == kind)
            );
        }
    }
}

use crate::git::{GitFileChange, GitFileDiff};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct PullRequestSummary {
    pub number: u64,
    pub title: String,
    pub url: String,
    pub author: String,
    pub state: String,
    pub draft: bool,
    pub base_branch: String,
    pub head_branch: String,
    pub base_sha: String,
    pub head_sha: String,
    pub review_decision: String,
    pub checks_state: String,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
pub enum PullRequestSort {
    Updated,
    Newest,
    Oldest,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
pub enum PullRequestFilter {
    Open,
    Closed,
    Merged,
    All,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct PullRequestInboxItem {
    pub summary: PullRequestSummary,
    pub repository: String,
    pub updated_at: String,
    pub additions: u64,
    pub deletions: u64,
    pub labels: Vec<String>,
    pub reviewers: Vec<String>,
    pub authored: bool,
    pub review_requested: bool,
    pub assigned: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct PullRequestPage {
    pub repository: String,
    pub viewer: String,
    pub generation: u64,
    pub items: Vec<PullRequestInboxItem>,
    pub next_cursor: Option<String>,
    pub total_count: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PullRequestDetail {
    pub summary: PullRequestSummary,
    pub body: String,
    pub files: Vec<GitFileChange>,
    pub diffs: Vec<GitFileDiff>,
    pub conversation: Vec<PullRequestComment>,
    pub threads: Vec<PullRequestThread>,
    pub checks: Vec<PullRequestCheck>,
    /// True when the host bounded any file, patch, conversation, or check list.
    pub truncated: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct PullRequestComment {
    pub id: String,
    pub author: String,
    pub body: String,
    pub created_at: String,
    pub url: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct PullRequestThread {
    pub id: String,
    pub path: String,
    pub side: PullRequestSide,
    pub line: Option<u32>,
    pub start_line: Option<u32>,
    pub outdated: bool,
    pub resolved: bool,
    pub comments: Vec<PullRequestComment>,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
pub enum PullRequestSide {
    Left,
    Right,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct PullRequestCheck {
    pub name: String,
    pub status: String,
    pub conclusion: String,
    pub url: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct PullRequestInlineComment {
    pub path: String,
    pub side: PullRequestSide,
    pub line: u32,
    pub start_line: Option<u32>,
    pub body: String,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
pub enum PullRequestReviewEvent {
    Comment,
    Approve,
    RequestChanges,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
pub enum PullRequestMergeMethod {
    Merge,
    Squash,
    Rebase,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub enum PullRequestRequest {
    List {
        query: String,
        sort: PullRequestSort,
        filter: PullRequestFilter,
        cursor: Option<String>,
        generation: u64,
    },
    ReadInbox {
        number: u64,
    },
    Discover {
        branch: Option<String>,
    },
    Read {
        number: u64,
    },
    Create {
        title: String,
        body: String,
        base: String,
        draft: bool,
        expected_branch: String,
        expected_head: String,
    },
    SubmitReview {
        number: u64,
        expected_head: String,
        body: String,
        event: PullRequestReviewEvent,
        comments: Vec<PullRequestInlineComment>,
    },
    ReplyThread {
        number: u64,
        thread_id: String,
        body: String,
    },
    ResolveThread {
        number: u64,
        thread_id: String,
        resolved: bool,
    },
    Checkout {
        number: u64,
        expected_head: String,
        new_worktree: bool,
    },
    Merge {
        number: u64,
        expected_head: String,
        method: PullRequestMergeMethod,
    },
}

impl PullRequestRequest {
    pub fn is_mutation(&self) -> bool {
        !matches!(
            self,
            Self::Discover { .. }
                | Self::Read { .. }
                | Self::List { .. }
                | Self::ReadInbox { .. }
        )
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
pub enum PullRequestErrorKind {
    Unsupported,
    Unavailable,
    Authentication,
    Permission,
    Stale,
    Conflict,
    Invalid,
    Failed,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum PullRequestReply {
    Listed {
        page: PullRequestPage,
    },
    InboxDetail {
        detail: PullRequestDetail,
        item: PullRequestInboxItem,
    },
    Discovered {
        branch: String,
        head: String,
        default_branch: String,
        pull_request: Option<PullRequestSummary>,
    },
    Detail {
        detail: PullRequestDetail,
    },
    Created {
        pull_request: PullRequestSummary,
    },
    ReviewSubmitted {
        number: u64,
    },
    ThreadUpdated {
        number: u64,
    },
    CheckedOut {
        number: u64,
        path: String,
        worktree: bool,
    },
    Merged {
        number: u64,
    },
    Error {
        kind: PullRequestErrorKind,
        message: String,
    },
}
