//! Repository-scoped, read-only inbox. Search syntax is deliberately a tiny whitelist.
use super::*;
const FIELDS: &str = "number title url author{login} state isDraft baseRefName headRefName baseRefOid headRefOid reviewDecision updatedAt additions deletions repository{nameWithOwner} labels(first:100){nodes{name}} reviewRequests(first:100){nodes{requestedReviewer{... on User{login} ... on Team{slug}}}} reviews(first:100){nodes{author{login}}} assignees(first:100){nodes{login}} commits(last:1){nodes{commit{statusCheckRollup{state}}}}";

fn query_text(query: &str) -> Result<String> {
    if query.len() > 256 {
        return Err(invalid("Inbox query is too long"));
    }
    let mut terms = Vec::new();
    for token in query.split_whitespace() {
        let term = token.strip_prefix("label:").unwrap_or(token);
        if term.is_empty()
            || term.len() > 80
            || matches!(term.to_ascii_uppercase().as_str(), "OR" | "AND" | "NOT")
            || !term
                .chars()
                .all(|c| c.is_alphanumeric() || matches!(c, '_' | '-' | '.' | '/'))
            || term.starts_with('-')
            || token.contains(':') && !token.starts_with("label:")
        {
            return Err(invalid(
                "Inbox supports plain search words and label:name only",
            ));
        }
        // Quote even whitelisted words so boolean/operator interpretation cannot escape scope.
        terms.push(if token.starts_with("label:") {
            format!("label:\"{term}\"")
        } else {
            format!("\"{term}\"")
        });
    }
    Ok(terms.join(" "))
}
fn cursor_valid(cursor: &str) -> bool {
    !cursor.is_empty()
        && cursor.len() <= 256
        && cursor
            .bytes()
            .all(|c| c.is_ascii_alphanumeric() || b"+/=_-".contains(&c))
}
fn viewer(v: &Value) -> Result<String> {
    let login = text(&v["data"]["viewer"], "login");
    if login.is_empty() {
        return Err(error(
            PullRequestErrorKind::Authentication,
            "Cannot identify GitHub viewer",
        ));
    }
    Ok(login)
}
fn item(host: &Host<'_>, pr: &Value, viewer: &str) -> Result<PullRequestInboxItem> {
    if text(&pr["repository"], "nameWithOwner").to_lowercase()
        != host.repo.name().to_lowercase()
        || pr["number"].as_u64().unwrap_or(0) == 0
        || !sha(&text(pr, "headRefOid"))
        || !sha(&text(pr, "baseRefOid"))
    {
        return Err(error(
            PullRequestErrorKind::Failed,
            "Invalid or out-of-scope inbox result",
        ));
    }
    let mut reviewers: Vec<String> = list(&pr["reviewRequests"]["nodes"])
        .iter()
        .take(100)
        .map(|r| {
            let r = &r["requestedReviewer"];
            if r["login"].is_string() {
                text(r, "login")
            } else {
                text(r, "slug")
            }
        })
        .filter(|s| !s.is_empty())
        .collect();
    for review in list(&pr["reviews"]["nodes"]).iter().take(100) {
        let login = text(&review["author"], "login");
        if !login.is_empty() && !reviewers.iter().any(|r| r.eq_ignore_ascii_case(&login))
        {
            reviewers.push(login);
        }
    }
    let author = text(&pr["author"], "login");
    Ok(PullRequestInboxItem {
        summary: PullRequestSummary {
            number: pr["number"].as_u64().unwrap_or(0),
            title: text(pr, "title"),
            url: text(pr, "url"),
            author: author.clone(),
            state: text(pr, "state").to_lowercase(),
            draft: pr["isDraft"].as_bool().unwrap_or(false),
            base_branch: text(pr, "baseRefName"),
            head_branch: text(pr, "headRefName"),
            base_sha: text(pr, "baseRefOid"),
            head_sha: text(pr, "headRefOid"),
            review_decision: text(pr, "reviewDecision"),
            checks_state: text(
                &pr["commits"]["nodes"][0]["commit"]["statusCheckRollup"],
                "state",
            ),
        },
        repository: host.repo.name(),
        updated_at: text(pr, "updatedAt"),
        additions: pr["additions"].as_u64().unwrap_or(0),
        deletions: pr["deletions"].as_u64().unwrap_or(0),
        labels: list(&pr["labels"]["nodes"])
            .iter()
            .take(100)
            .map(|l| text(l, "name"))
            .collect(),
        authored: author.eq_ignore_ascii_case(viewer),
        review_requested: list(&pr["reviewRequests"]["nodes"])
            .iter()
            .any(|r| text(&r["requestedReviewer"], "login").eq_ignore_ascii_case(viewer)),
        assigned: list(&pr["assignees"]["nodes"])
            .iter()
            .any(|a| text(a, "login").eq_ignore_ascii_case(viewer)),
        reviewers,
    })
}
pub(super) fn page(
    host: &Host<'_>,
    query: &str,
    sort: PullRequestSort,
    filter: PullRequestFilter,
    cursor: Option<String>,
    generation: u64,
) -> Result<PullRequestReply> {
    let terms = query_text(query)?;
    if cursor.as_ref().is_some_and(|c| !cursor_valid(c)) {
        return Err(invalid("Invalid inbox cursor"));
    }
    let sort = match sort {
        PullRequestSort::Updated => "updated-desc",
        PullRequestSort::Newest => "created-desc",
        PullRequestSort::Oldest => "created-asc",
    };
    let filter = match filter {
        PullRequestFilter::Open => "is:open",
        PullRequestFilter::Closed => "is:closed is:unmerged",
        PullRequestFilter::Merged => "is:merged",
        PullRequestFilter::All => "",
    };
    let q = format!(
        "repo:{} is:pr {filter} sort:{sort} {terms}",
        host.repo.name()
    );
    let query = format!("query($q:String!,$cursor:String){{viewer{{login}} search(query:$q,type:ISSUE,first:30,after:$cursor){{issueCount pageInfo{{hasNextPage endCursor}} nodes{{... on PullRequest{{{FIELDS}}}}}}}}}");
    let v = host.graphql(&query, json!({"q":q,"cursor":cursor}))?;
    let viewer = viewer(&v)?;
    let conn = &v["data"]["search"];
    if !conn["nodes"].is_array() || list(&conn["nodes"]).len() > 30 {
        return Err(error(PullRequestErrorKind::Failed, "Invalid inbox page"));
    }
    let items = list(&conn["nodes"])
        .iter()
        .map(|pr| item(host, pr, &viewer))
        .collect::<Result<Vec<_>>>()?;
    let next_cursor = if conn["pageInfo"]["hasNextPage"].as_bool() == Some(true) {
        let c = text(&conn["pageInfo"], "endCursor");
        if !cursor_valid(&c) {
            return Err(error(PullRequestErrorKind::Failed, "Invalid GitHub cursor"));
        }
        Some(c)
    } else {
        None
    };
    Ok(PullRequestReply::Listed {
        page: PullRequestPage {
            repository: host.repo.name(),
            viewer,
            generation,
            items,
            next_cursor,
            total_count: conn["issueCount"]
                .as_u64()
                .unwrap_or(0)
                .min(u32::MAX as u64) as u32,
        },
    })
}
pub(super) fn read(host: &Host<'_>, number: u64) -> Result<PullRequestReply> {
    let detail = detail(host, number)?;
    let query = format!("query($owner:String!,$repo:String!,$number:Int!){{viewer{{login}} repository(owner:$owner,name:$repo){{pullRequest(number:$number){{{FIELDS}}}}}}}");
    let v = host.graphql(
        &query,
        json!({"owner":host.repo.owner,"repo":host.repo.repo,"number":number}),
    )?;
    let item = item(host, &v["data"]["repository"]["pullRequest"], &viewer(&v)?)?;
    let final_pr = host.pr(number)?;
    if item.summary.number != number
        || item.summary.head_sha != detail.summary.head_sha
        || item.summary.base_sha != detail.summary.base_sha
        || text(&final_pr["head"], "sha") != detail.summary.head_sha
        || text(&final_pr["base"], "sha") != detail.summary.base_sha
    {
        return Err(error(
            PullRequestErrorKind::Stale,
            "PR comparison changed while loading inbox metadata; refresh",
        ));
    }
    Ok(PullRequestReply::InboxDetail { detail, item })
}

#[cfg(test)]
#[path = "pull_request_inbox_tests.rs"]
mod tests;
