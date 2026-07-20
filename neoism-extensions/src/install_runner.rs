use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use futures_util::StreamExt;
use thiserror::Error;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::process::Command;
use tokio::sync::mpsc::UnboundedSender;
use tokio::task::JoinHandle;

use crate::installed::{InstalledEntry, InstalledIndex};
use crate::manifest::{ExtensionManifest, GithubAsset, InstallKind};
use crate::paths;

mod ecosystems;
mod managed;
mod process;
mod release;
mod resolution;
mod runtime;

use ecosystems::*;
pub use managed::*;
use process::*;
pub use release::*;
use resolution::*;
use runtime::*;

#[derive(Clone, Debug)]
pub enum ProgressEvent {
    Started,
    Downloading { bytes: u64, total: Option<u64> },
    Extracting,
    Linking,
    // Carries both a coarse percent and a status line so the panel can render
    // a single progress bar + label without needing to translate variants.
    Progress { percent: u8, status: String },
    Waiting { status: String },
    Done,
    Failed { message: String },
}

pub struct InstallHandle {
    pub id: String,
    task: JoinHandle<Result<InstalledEntry, InstallError>>,
}

impl InstallHandle {
    /// Abort the background install. Subprocesses are configured with
    /// `kill_on_drop`, and partial HTTP downloads carry a drop guard, so this
    /// does not leave a package manager or `.part` file running behind us.
    pub fn cancel(&self) {
        self.task.abort();
    }

    pub fn is_finished(&self) -> bool {
        self.task.is_finished()
    }

    pub async fn join(
        self,
    ) -> Result<Result<InstalledEntry, InstallError>, tokio::task::JoinError> {
        self.task.await
    }

    /// Wrap a host-provided install task (Tree-sitter/kernel installers) so
    /// the UI has one cancellation/completion abstraction for every job.
    pub fn from_task(
        id: impl Into<String>,
        task: JoinHandle<Result<InstalledEntry, InstallError>>,
    ) -> Self {
        Self {
            id: id.into(),
            task,
        }
    }
}

#[derive(Debug, Error)]
pub enum InstallError {
    #[error("network error: {0}")]
    Network(String),
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),
    #[error("command `{command}` failed with status {status}: {stderr}")]
    CommandFailed {
        command: String,
        status: i32,
        stderr: String,
    },
    #[error("install process exited with status {exit:?}; tail: {tail}")]
    Process {
        exit: std::process::ExitStatus,
        tail: String,
    },
    #[error("tool `{0}` not found on PATH")]
    MissingTool(&'static str),
    #[error("{tool} install timed out after {seconds} seconds")]
    TimedOut { tool: String, seconds: u64 },
    #[error("no asset matches target `{0}`")]
    NoAssetForTarget(String),
    #[error("binary `{0}` not found after install (archive layout mismatch or failed download)")]
    BinaryNotFound(String),
    #[error("zip error: {0}")]
    Zip(String),
    #[error("extension is already installed")]
    AlreadyInstalled,
    #[error("extension is not installed")]
    NotInstalled,
    #[error("not yet implemented")]
    NotImplemented,
    #[error("config parse error: {0}")]
    ParseManifest(String),
    #[error("invalid {kind} package specification `{value}`")]
    InvalidPackageSpec { kind: &'static str, value: String },
}

const INSTALL_PROCESS_TIMEOUT: Duration = Duration::from_secs(5 * 60);
const DOWNLOAD_CONNECT_TIMEOUT: Duration = Duration::from_secs(15);
const DOWNLOAD_REQUEST_TIMEOUT: Duration = Duration::from_secs(5 * 60);
const DOWNLOAD_IDLE_TIMEOUT: Duration = Duration::from_secs(30);
static INSTALLED_INDEX_LOCK: OnceLock<Mutex<()>> = OnceLock::new();

/// Spawn the install task and hand back its `JoinHandle`. Progress events
/// flow through `progress`; dropping the sender on completion is the EOF
/// signal for the receiver loop. The final `Done` event is emitted *after*
/// the symlink + record creation succeeds.
pub fn install(
    manifest: ExtensionManifest,
    progress: UnboundedSender<ProgressEvent>,
) -> InstallHandle {
    let id = manifest.id.clone();
    let task = tokio::spawn(async move {
        let result = run_install(manifest, progress.clone()).await;
        if let Err(error) = &result {
            emit(
                &progress,
                ProgressEvent::Failed {
                    message: error.to_string(),
                },
            );
        }
        result
    });
    InstallHandle::from_task(id, task)
}

async fn run_install(
    manifest: ExtensionManifest,
    progress: UnboundedSender<ProgressEvent>,
) -> Result<InstalledEntry, InstallError> {
    emit(&progress, ProgressEvent::Started);

    let install_dir = paths::install_dir_for(&manifest.id);
    tokio::fs::create_dir_all(&install_dir).await?;

    let bin_names = declared_executable_names(&manifest)?;

    let bin_path = match &manifest.install {
        InstallKind::Npm {
            package,
            version,
            extra_packages,
        } => {
            install_npm(
                &install_dir,
                package,
                version,
                extra_packages,
                &bin_names,
                &progress,
            )
            .await?
        }
        InstallKind::Pip { package, version } => {
            install_pip(&install_dir, package, version, &bin_names, &progress).await?
        }
        InstallKind::GithubRelease {
            owner,
            repo,
            tag,
            assets,
        } => {
            let target = current_target();
            let asset = pick_asset(assets, target)
                .ok_or_else(|| InstallError::NoAssetForTarget(target.to_string()))?;
            install_github_release(&install_dir, owner, repo, tag, asset, &progress)
                .await?
        }
        InstallKind::Cargo {
            crate_name,
            version,
            features,
        } => {
            install_cargo(
                &install_dir,
                crate_name,
                version,
                features,
                &bin_names,
                &progress,
            )
            .await?
        }
        InstallKind::Go { package, version } => {
            install_go(&install_dir, package, version, &bin_names, &progress).await?
        }
        InstallKind::Gem {
            package,
            version,
            extra_packages,
        } => {
            install_gem(
                &install_dir,
                package,
                version,
                extra_packages,
                &bin_names,
                &progress,
            )
            .await?
        }
    };

    let installed_binaries =
        resolve_installed_binaries(&manifest, &install_dir, &bin_path, &bin_names)
            .await?;
    emit(&progress, ProgressEvent::Linking);
    let primary_name = default_bin_name(&manifest, &bin_path);
    let mut link_path = None;
    for (name, path) in installed_binaries {
        let linked = link_bin(&path, name.clone()).await?;
        if name == primary_name {
            link_path = Some(linked);
        }
    }
    let link_path = link_path.ok_or_else(|| {
        InstallError::BinaryNotFound(format!(
            "primary executable `{primary_name}` was not produced by {}",
            manifest.id
        ))
    })?;

    let kind = install_kind_tag(&manifest.install);
    let entry = InstalledEntry {
        id: manifest.id.clone(),
        version: manifest.version.clone(),
        install_kind: kind.to_string(),
        bin_path: Some(link_path),
        installed_at: now_ms(),
    };

    // Installation is authoritative in the background worker. The UI may be
    // closed, switched to another tab, or miss a frame; none of those should
    // decide whether a completed binary appears in installed.json.
    record_installed(entry.clone()).await?;

    emit(
        &progress,
        ProgressEvent::Progress {
            percent: 100,
            status: format!("installed {} {}", manifest.id, manifest.version),
        },
    );
    emit(&progress, ProgressEvent::Done);
    Ok(entry)
}

#[cfg(test)]
mod tests;
