use std::io::Read;
use std::path::{Path, PathBuf};

use anyhow::Context;
use neoism_agent_service_api::{
    AgentServices, WorkspaceWorkerBinding, WorkspaceWorkerBootstrap,
    WorkspaceWorkerVerificationKey,
};

pub(crate) fn services(
    bootstrap: Option<PathBuf>,
    key_file: Option<PathBuf>,
) -> anyhow::Result<AgentServices> {
    let (bootstrap, key_file) = match (bootstrap, key_file) {
        (None, None) => return Ok(crate::standalone_services()),
        (Some(bootstrap), Some(key_file)) => (bootstrap, key_file),
        _ => anyhow::bail!(
            "worker bootstrap and verification-key file must be supplied together"
        ),
    };
    let profile: WorkspaceWorkerBootstrap =
        serde_json::from_slice(&read_bounded(&bootstrap, 16 * 1024)?)
            .context("invalid workspace worker bootstrap")?;
    let key = WorkspaceWorkerVerificationKey::new(read_bounded(&key_file, 32)?)?;
    let binding = WorkspaceWorkerBinding::new(profile, key)?;
    for path in [&bootstrap, &key_file] {
        let path = std::fs::canonicalize(path)
            .context("worker controller file is unavailable")?;
        anyhow::ensure!(
            !path.starts_with(binding.root()),
            "worker bootstrap and verification key must be outside the workspace"
        );
    }
    // Subprocesses need neither controller bootstrap locations nor infrastructure auth.
    std::env::remove_var("NEOISM_AGENT_WORKER_BOOTSTRAP");
    std::env::remove_var("NEOISM_AGENT_WORKER_VERIFICATION_KEY_FILE");
    let root = binding.root().to_owned();
    let services = neoism_agent_server::workspace_worker_services(
        crate::standalone_services(),
        binding,
    )?;
    // Relative tool/config defaults now refer to the worker, never the launcher directory.
    std::env::set_current_dir(root).context("cannot enter worker workspace")?;
    Ok(services)
}

fn read_bounded(path: &Path, limit: usize) -> anyhow::Result<Vec<u8>> {
    let mut bytes = Vec::new();
    std::fs::File::open(path)
        .context("cannot open controller-owned worker file")?
        .take((limit + 1) as u64)
        .read_to_end(&mut bytes)?;
    anyhow::ensure!(
        bytes.len() <= limit,
        "worker bootstrap or key file exceeds its size limit"
    );
    Ok(bytes)
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::Parser;

    #[test]
    fn worker_options_require_each_other_and_exclude_gui() {
        assert!(crate::Cli::try_parse_from([
            "neoism-agent",
            "serve",
            "--worker-bootstrap",
            "worker.json"
        ])
        .is_err());
        assert!(crate::Cli::try_parse_from([
            "neoism-agent",
            "serve",
            "--worker-verification-key-file",
            "worker.key"
        ])
        .is_err());
        assert!(crate::Cli::try_parse_from([
            "neoism-agent",
            "serve",
            "--worker-bootstrap",
            "worker.json",
            "--worker-verification-key-file",
            "worker.key",
            "--web"
        ])
        .is_err());
        assert!(crate::Cli::try_parse_from([
            "neoism-agent",
            "serve",
            "--worker-bootstrap",
            "worker.json",
            "--worker-verification-key-file",
            "worker.key"
        ])
        .is_ok());
    }

    #[test]
    fn partial_worker_bootstrap_never_falls_back_to_local() {
        assert!(services(Some("worker.json".into()), None).is_err());
        assert!(services(None, Some("worker.key".into())).is_err());
    }
}
