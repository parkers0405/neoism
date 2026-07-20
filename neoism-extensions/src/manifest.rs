use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ExtensionManifest {
    pub id: String,
    pub name: String,
    pub version: String,
    pub description: String,
    pub author: String,
    pub downloads: Option<u64>,
    pub categories: Vec<String>,
    #[serde(default)]
    pub languages: Vec<String>,
    pub repository_url: Option<String>,
    #[serde(default)]
    pub homepage: Option<String>,
    /// Public executable names provided by this package. The first runtime
    /// command remains the primary executable, while this list preserves
    /// additional catalog-declared commands without package-specific aliases.
    #[serde(default)]
    pub executables: Vec<String>,
    pub install: InstallKind,
    pub run: Option<RunSpec>,
    #[serde(default)]
    pub env_keys: Vec<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum InstallKind {
    Npm {
        package: String,
        version: String,
        /// Packages Mason declares alongside the primary npm package (for
        /// example TypeScript and framework plugins). They are installed in
        /// the same private npm prefix so the language server can resolve
        /// them without relying on a user's global npm state.
        #[serde(default)]
        extra_packages: Vec<String>,
    },
    Pip {
        package: String,
        version: String,
    },
    GithubRelease {
        owner: String,
        repo: String,
        tag: String,
        assets: Vec<GithubAsset>,
    },
    Cargo {
        crate_name: String,
        version: String,
        features: Vec<String>,
    },
    Go {
        /// Import path passed to `go install`, including a Mason purl subpath
        /// such as `golang.org/x/tools/cmd/goimports` when applicable.
        package: String,
        version: String,
    },
    Gem {
        package: String,
        version: String,
        /// Additional gems installed into the same private GEM_HOME.
        #[serde(default)]
        extra_packages: Vec<String>,
    },
}

/// One per-platform asset entry. `target` follows the Mason naming scheme
/// (e.g. `linux_x64_gnu`, `darwin_arm64`).
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct GithubAsset {
    pub target: String,
    pub file: String,
    pub bin: String,
    /// Public executable name to extracted relative path for this target.
    /// This is target-specific because named Mason asset binaries can differ
    /// between Unix and Windows archives.
    #[serde(default)]
    pub executables: BTreeMap<String, String>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct RunSpec {
    pub command: Vec<String>,
    pub env: BTreeMap<String, String>,
}
