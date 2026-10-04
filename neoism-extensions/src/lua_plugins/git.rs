use std::path::Path;
use std::process::{Command, Output};

use super::error::{AcquisitionError, AcquisitionStage};

pub(crate) enum Repository<'a> {
    Https(&'a str),
    #[cfg(test)]
    TestLocal(&'a Path),
}

impl Repository<'_> {
    fn argument(&self) -> String {
        match self {
            Self::Https(url) => (*url).to_owned(),
            #[cfg(test)]
            Self::TestLocal(path) => path.to_string_lossy().into_owned(),
        }
    }

    fn allows_file_protocol(&self) -> bool {
        #[cfg(test)]
        if matches!(self, Self::TestLocal(_)) {
            return true;
        }
        false
    }
}

pub(crate) fn checkout(
    repository: Repository<'_>,
    requested_ref: &str,
    staging: &Path,
) -> Result<String, AcquisitionError> {
    run(
        None,
        AcquisitionStage::Clone,
        "clone",
        repository.allows_file_protocol(),
        [
            "clone",
            "--no-checkout",
            "--no-tags",
            "--",
            &repository.argument(),
            &staging.to_string_lossy(),
        ],
    )?;
    run(
        Some(staging),
        AcquisitionStage::Fetch,
        "fetch",
        repository.allows_file_protocol(),
        ["fetch", "--force", "--tags", "origin", requested_ref],
    )?;
    let revision = format!("{}^{{commit}}", "FETCH_HEAD");
    let output = run(
        Some(staging),
        AcquisitionStage::Resolve,
        "rev-parse",
        repository.allows_file_protocol(),
        ["rev-parse", "--verify", &revision],
    )?;
    let commit = String::from_utf8_lossy(&output.stdout)
        .trim()
        .to_ascii_lowercase();
    if !valid_commit(&commit) {
        return Err(AcquisitionError::InvalidResolvedCommit(commit));
    }
    run(
        Some(staging),
        AcquisitionStage::Checkout,
        "checkout",
        repository.allows_file_protocol(),
        [
            "-c",
            "core.symlinks=false",
            "checkout",
            "--detach",
            "--force",
            &commit,
        ],
    )?;
    let git_dir = staging.join(".git");
    std::fs::remove_dir_all(&git_dir)
        .map_err(|e| super::error::io(AcquisitionStage::Checkout, git_dir, e))?;
    Ok(commit)
}

fn run<const N: usize>(
    cwd: Option<&Path>,
    stage: AcquisitionStage,
    operation: &'static str,
    file_protocol: bool,
    args: [&str; N],
) -> Result<Output, AcquisitionError> {
    let mut command = Command::new("git");
    command.args([
        "-c",
        if file_protocol {
            "protocol.file.allow=always"
        } else {
            "protocol.file.allow=never"
        },
    ]);
    command
        .args(args)
        .env("GIT_TERMINAL_PROMPT", "0")
        .env("GIT_CONFIG_NOSYSTEM", "1");
    if let Some(cwd) = cwd {
        command.current_dir(cwd);
    }
    let output = command.output().map_err(|e| {
        if e.kind() == std::io::ErrorKind::NotFound {
            AcquisitionError::GitMissing
        } else {
            super::error::io(stage, cwd.unwrap_or_else(|| Path::new("git")), e)
        }
    })?;
    if output.status.success() {
        Ok(output)
    } else {
        Err(AcquisitionError::Git {
            stage,
            operation,
            status: output.status.code(),
            stderr: String::from_utf8_lossy(&output.stderr).trim().to_owned(),
        })
    }
}

fn valid_commit(value: &str) -> bool {
    matches!(value.len(), 40 | 64) && value.bytes().all(|b| b.is_ascii_hexdigit())
}
