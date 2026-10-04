use std::path::PathBuf;

use thiserror::Error;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum AcquisitionStage {
    ValidateInput,
    Lock,
    Clone,
    Fetch,
    Resolve,
    Checkout,
    Checksum,
    ValidatePlugin,
    PublishRevision,
    PublishLockfile,
    Remove,
    Prune,
}

#[derive(Debug, Error)]
pub enum AcquisitionError {
    #[error("invalid plugin id `{0}`")]
    InvalidPluginId(String),
    #[error("invalid repository URL `{url}`: {reason}")]
    InvalidRepositoryUrl { url: String, reason: String },
    #[error("invalid requested ref `{0}`")]
    InvalidRef(String),
    #[error("Lua plugin store is busy: {0}")]
    StoreBusy(PathBuf),
    #[error("I/O error during {stage:?} at {}: {source}", path.display())]
    Io {
        stage: AcquisitionStage,
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("could not parse Lua plugin lockfile {}: {message}", path.display())]
    LockfileParse { path: PathBuf, message: String },
    #[error("unsupported Lua plugin lockfile version {found}; this build supports version {supported}")]
    UnsupportedLockfileVersion { found: u32, supported: u32 },
    #[error("could not serialize Lua plugin lockfile: {0}")]
    LockfileSerialize(String),
    #[error("git is required to acquire Lua plugins")]
    GitMissing,
    #[error("git {operation} failed (status {status:?}): {stderr}")]
    Git {
        stage: AcquisitionStage,
        operation: &'static str,
        status: Option<i32>,
        stderr: String,
    },
    #[error("git resolved an invalid commit id `{0}`")]
    InvalidResolvedCommit(String),
    #[error("plugin validation failed: {0}")]
    Validation(String),
    #[error("tree checksum mismatch: expected {expected}, got {actual}")]
    ChecksumMismatch { expected: String, actual: String },
    #[error("immutable revision collision at {} (expected tree {expected}, got {actual})", path.display())]
    ImmutableRevisionCollision {
        path: PathBuf,
        expected: String,
        actual: String,
    },
    #[error("plugin `{0}` is not present in the Lua plugin lockfile")]
    NotInstalled(String),
    #[error("unsafe store path rejected: {}", .0.display())]
    UnsafePath(PathBuf),
}

impl AcquisitionError {
    pub fn stage(&self) -> AcquisitionStage {
        match self {
            Self::InvalidPluginId(_)
            | Self::InvalidRepositoryUrl { .. }
            | Self::InvalidRef(_) => AcquisitionStage::ValidateInput,
            Self::StoreBusy(_) => AcquisitionStage::Lock,
            Self::Io { stage, .. } | Self::Git { stage, .. } => *stage,
            Self::LockfileParse { .. }
            | Self::UnsupportedLockfileVersion { .. }
            | Self::LockfileSerialize(_) => AcquisitionStage::PublishLockfile,
            Self::GitMissing => AcquisitionStage::Clone,
            Self::InvalidResolvedCommit(_) => AcquisitionStage::Resolve,
            Self::Validation(_) => AcquisitionStage::ValidatePlugin,
            Self::ChecksumMismatch { .. } => AcquisitionStage::Checksum,
            Self::ImmutableRevisionCollision { .. } => AcquisitionStage::PublishRevision,
            Self::NotInstalled(_) => AcquisitionStage::ValidateInput,
            Self::UnsafePath(_) => AcquisitionStage::Prune,
        }
    }

    pub fn retryable(&self) -> bool {
        matches!(
            self,
            Self::StoreBusy(_) | Self::Git { .. } | Self::Io { .. }
        )
    }
}

pub(crate) fn io(
    stage: AcquisitionStage,
    path: impl Into<PathBuf>,
    source: std::io::Error,
) -> AcquisitionError {
    AcquisitionError::Io {
        stage,
        path: path.into(),
        source,
    }
}
