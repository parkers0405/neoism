//! Branch-scoped PR identity and an exact, non-force source-branch push route.
use super::*;
use git2::{Config, Repository};
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub(super) struct Association {
    pub version: u32,
    pub number: u64,
    pub base: String,
    pub head_repo: String,
    pub head_ref: String,
    pub remote: String,
}
fn identity(name: &str) -> Result<Identity> {
    Identity::parse(&format!("https://github.com/{name}"))
}
impl Association {
    pub(super) fn from_pr(base: &Identity, pr: &Value, remote: String) -> Result<Self> {
        let base_name = text(&pr["base"]["repo"], "full_name");
        if !base_name.eq_ignore_ascii_case(&base.name()) {
            return Err(error(
                PullRequestErrorKind::Stale,
                "PR base repository identity changed",
            ));
        }
        let head_repo = text(&pr["head"]["repo"], "full_name");
        if head_repo.is_empty() {
            return Err(error(PullRequestErrorKind::Unsupported,"PR source repository was deleted or is inaccessible; safe checkout/push association is unavailable"));
        }
        identity(&head_repo)?;
        let association = Self {
            version: 1,
            number: pr["number"].as_u64().unwrap_or(0),
            base: base.name(),
            head_repo,
            head_ref: text(&pr["head"], "ref"),
            remote,
        };
        association.validate()?;
        Ok(association)
    }
    fn validate(&self) -> Result<()> {
        if self.version != 1 || self.number == 0 {
            return Err(invalid("Invalid branch PR association version or number"));
        }
        identity(&self.base)?;
        identity(&self.head_repo)?;
        if !branch(&self.head_ref)
            || !git2::Reference::is_valid_name(&format!("refs/heads/{}", self.head_ref))
            || !self.remote.starts_with("neoism-pr-")
            || !self
                .remote
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b == b'-')
        {
            return Err(invalid(
                "Invalid PR source branch or dedicated remote identity",
            ));
        }
        Ok(())
    }
    pub(super) fn verify_pr(&self, base: &Identity, pr: &Value) -> Result<()> {
        if !self.base.eq_ignore_ascii_case(&base.name()) {
            return Err(error(PullRequestErrorKind::Stale,"Local PR association belongs to a different base repository; do not publish this generated branch"));
        }
        let actual = Self::from_pr(base, pr, self.remote.clone())?;
        if self.number != actual.number
            || !self.head_repo.eq_ignore_ascii_case(&actual.head_repo)
            || self.head_ref != actual.head_ref
        {
            return Err(error(PullRequestErrorKind::Stale,"PR source repository or branch changed; refresh/re-checkout before pushing"));
        }
        Ok(())
    }
    fn url(&self) -> String {
        format!("https://github.com/{}.git", self.head_repo)
    }
    fn tracking(&self) -> String {
        format!("refs/remotes/{}/{}", self.remote, self.head_ref)
    }
    fn refspec(&self, local: &str) -> String {
        format!("refs/heads/{local}:refs/heads/{}", self.head_ref)
    }
}
fn key(local: &str) -> String {
    format!("branch.{local}.neoism-pr-association")
}
fn read(repo: &Repository, local: &str) -> Result<Option<Association>> {
    let config = repo
        .config()
        .map_err(|_| invalid("Cannot read branch PR association"))?;
    let value = match config.get_string(&key(local)) {
        Ok(value) => value,
        Err(e) if e.code() == git2::ErrorCode::NotFound => return Ok(None),
        Err(_) => return Err(invalid("Cannot read branch PR association")),
    };
    if value.len() > 8192 {
        return Err(invalid("Branch PR association exceeds its size limit"));
    }
    let association: Association = serde_json::from_str(&value)
        .map_err(|_| invalid("Malformed branch PR association"))?;
    association.validate()?;
    validate_route(repo, local, &association)?;
    Ok(Some(association))
}
fn exactly(config: &Config, key: &str, expected: Option<&str>) -> Result<()> {
    let mut entries = match config.multivar(key, None) {
        Ok(entries) => entries,
        Err(e) if e.code() == git2::ErrorCode::NotFound && expected.is_none() => {
            return Ok(())
        }
        Err(_) => return Err(invalid("Cannot inspect PR push routing")),
    };
    let mut values = Vec::new();
    while let Some(entry) = entries.next() {
        values.push(
            entry
                .map_err(|_| invalid("Cannot inspect PR push routing"))?
                .value()
                .unwrap_or("")
                .to_string(),
        );
    }
    if match expected {
        Some(value) => values.len() == 1 && values[0] == value,
        None => values.is_empty(),
    } {
        Ok(())
    } else {
        Err(invalid("Dedicated PR remote/tracking configuration changed; refusing an ambiguous push route"))
    }
}
fn validate_route(
    repo: &Repository,
    local: &str,
    association: &Association,
) -> Result<()> {
    let config = repo
        .config()
        .map_err(|_| invalid("Cannot inspect PR push route"))?;
    exactly(
        &config,
        &format!("remote.{}.url", association.remote),
        Some(&association.url()),
    )?;
    exactly(
        &config,
        &format!("remote.{}.pushurl", association.remote),
        None,
    )?;
    exactly(
        &config,
        &format!("remote.{}.push", association.remote),
        Some(&association.refspec(local)),
    )?;
    exactly(
        &config,
        &format!("branch.{local}.remote"),
        Some(&association.remote),
    )?;
    exactly(
        &config,
        &format!("branch.{local}.pushRemote"),
        Some(&association.remote),
    )?;
    exactly(
        &config,
        &format!("branch.{local}.merge"),
        Some(&format!("refs/heads/{}", association.head_ref)),
    )?;
    if config
        .get_bool(&format!("remote.{}.mirror", association.remote))
        .unwrap_or(false)
    {
        return Err(invalid("PR source remote must not be a mirror"));
    }
    Ok(())
}
pub(super) fn configure(
    repo: &Repository,
    local: &str,
    expected: &str,
    association: &Association,
) -> Result<()> {
    association.validate()?;
    if !local.starts_with("neoism/pr-")
        || !git2::Reference::is_valid_name(&format!("refs/heads/{local}"))
    {
        return Err(invalid("PR association requires a generated local branch"));
    }
    if repo
        .find_reference(&format!("refs/heads/{local}"))
        .ok()
        .and_then(|r| r.target())
        .map(|o| o.to_string())
        .as_deref()
        != Some(expected)
    {
        return Err(error(
            PullRequestErrorKind::Stale,
            "Local PR branch head changed before tracking setup",
        ));
    }
    let remote_prefix = format!("remote.{}.", association.remote);
    let branch_prefix = format!("branch.{local}.");
    let config = repo
        .config()
        .map_err(|_| invalid("Cannot inspect existing remote configuration"))?;
    let mut entries = config
        .entries(None)
        .map_err(|_| invalid("Cannot inspect existing remote configuration"))?;
    while let Some(entry) = entries.next() {
        let entry =
            entry.map_err(|_| invalid("Cannot inspect existing remote configuration"))?;
        if entry.name().is_some_and(|key| {
            key.starts_with(&remote_prefix) || key.starts_with(&branch_prefix)
        }) {
            return Err(error(PullRequestErrorKind::Conflict,"Generated PR remote/branch configuration already exists; user configuration was not overwritten"));
        }
    }
    // Explicit common config works even with extensions.worktreeConfig enabled.
    // Every key is in a fresh generated namespace. Write the association first:
    // until route validation succeeds, reads/Push/Publish fail CLOSED rather
    // than falling back to treating a partially configured checkout as a new PR.
    let mut config = Config::open(&repo.commondir().join("config"))
        .map_err(|_| invalid("Cannot configure the common repository"))?;
    config
        .set_str(
            &key(local),
            &serde_json::to_string(association)
                .map_err(|_| invalid("Cannot encode PR association"))?,
        )
        .map_err(|_| invalid("Cannot persist PR association"))?;
    config
        .set_str(
            &format!("remote.{}.url", association.remote),
            &association.url(),
        )
        .map_err(|_| invalid("Cannot establish PR source remote"))?;
    config
        .set_str(
            &format!("remote.{}.fetch", association.remote),
            &format!(
                "+refs/heads/{}:{}",
                association.head_ref,
                association.tracking()
            ),
        )
        .map_err(|_| invalid("Cannot establish exact PR fetch route"))?;
    config
        .set_str(
            &format!("remote.{}.push", association.remote),
            &association.refspec(local),
        )
        .map_err(|_| invalid("Cannot establish exact PR push route"))?;
    config
        .set_str(&format!("branch.{local}.remote"), &association.remote)
        .map_err(|_| invalid("Cannot configure PR tracking"))?;
    config
        .set_str(&format!("branch.{local}.pushRemote"), &association.remote)
        .map_err(|_| invalid("Cannot configure PR push tracking"))?;
    config
        .set_str(
            &format!("branch.{local}.merge"),
            &format!("refs/heads/{}", association.head_ref),
        )
        .map_err(|_| invalid("Cannot configure PR upstream"))?;
    repo.reference(&association.tracking(),git2::Oid::from_str(expected).map_err(|_|invalid("Invalid expected PR head"))?,false,"Neoism verified PR source head")
        .map_err(|_|invalid("Cannot establish verified PR tracking reference without overwriting an existing ref"))?;
    validate_route(repo, local, association)?;
    // The association is already persisted; successful route validation now enables it.
    Ok(())
}
pub(super) fn discover(
    host: &Host<'_>,
    local: &str,
) -> Result<Option<PullRequestSummary>> {
    let repo = Repository::discover(host.root)
        .map_err(|_| invalid("Cannot discover local PR association"))?;
    let Some(association) = read(&repo, local)? else {
        return Ok(None);
    };
    let pr = host.pr(association.number)?;
    association.verify_pr(&host.repo, &pr)?;
    let mut summary = summary(&pr);
    host.enrich(&mut summary)?;
    Ok(Some(summary))
}
pub(super) fn push(root: &Path) -> Option<std::result::Result<(), String>> {
    let repo = Repository::discover(root).ok()?;
    let local = repo.head().ok()?.shorthand()?.to_string();
    let association = match read(&repo, &local) {
        Ok(Some(a)) => a,
        Ok(None) => return None,
        Err((_, e)) => return Some(Err(e)),
    };
    if let Err((_, message)) = checkout::authorized_root(root) {
        return Some(Err(message));
    }
    // Explicit refspec, no tags, no force, no mirror, and never a base/origin
    // default. Normal GitHub branch protection/permissions still apply.
    Some(
        git(
            root,
            &[
                "push",
                "--no-follow-tags",
                "--no-force",
                "--no-mirror",
                "--",
                &association.remote,
                &association.refspec(&local),
            ],
        )
        .map(|_| ())
        .map_err(|(_, e)| e),
    )
}
pub(super) fn associated(root: &Path, local: &str) -> std::result::Result<bool, String> {
    let repo = Repository::discover(root)
        .map_err(|_| "Cannot inspect PR branch identity".to_string())?;
    read(&repo, local).map(|a| a.is_some()).map_err(|(_, e)| e)
}
