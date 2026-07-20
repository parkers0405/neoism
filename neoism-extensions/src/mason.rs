use std::collections::BTreeMap;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

use serde::{Deserialize, Serialize};

use crate::install_runner::InstallError;
use crate::installed::write_atomic;
use crate::manifest::{ExtensionManifest, GithubAsset, InstallKind, RunSpec};

const MASON_SNAPSHOT_URL: &str =
    "https://github.com/mason-org/mason-registry/releases/latest/download/registry.json.zip";
const CACHE_MAX_AGE: Duration = Duration::from_secs(24 * 60 * 60);

pub type MasonRegistry = Vec<MasonPackage>;

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct MasonPackage {
    pub name: String,
    #[serde(default)]
    pub description: String,
    #[serde(default)]
    pub categories: Vec<String>,
    #[serde(default)]
    pub languages: Vec<String>,
    #[serde(default)]
    pub homepage: Option<String>,
    pub source: MasonSource,
    #[serde(default)]
    pub bin: BTreeMap<String, String>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct MasonSource {
    pub id: String,
    #[serde(default)]
    pub asset: Option<MasonAssetField>,
    /// Companion packages installed into the same package-manager prefix as
    /// the primary source. Mason uses this for TypeScript/framework plugins
    /// and Ruby LSP add-ons.
    #[serde(default)]
    pub extra_packages: Vec<String>,
}

/// Mason's `asset` is either a single object or an array of per-target objects.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(untagged)]
pub enum MasonAssetField {
    Single(MasonAsset),
    Multiple(Vec<MasonAsset>),
}

impl MasonAssetField {
    pub fn into_vec(self) -> Vec<MasonAsset> {
        match self {
            MasonAssetField::Single(a) => vec![a],
            MasonAssetField::Multiple(v) => v,
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct MasonAsset {
    #[serde(default)]
    pub target: MasonTargetField,
    pub file: MasonFileField,
    #[serde(default)]
    pub bin: Option<MasonAssetBinField>,
}

/// A release asset may expose one executable path or a named set of paths.
/// The latter is used by packages such as ElixirLS (`lsp` and `dap`).
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(untagged)]
pub enum MasonAssetBinField {
    Single(String),
    Named(BTreeMap<String, String>),
}

impl MasonAssetBinField {
    fn resolve(&self, selector: Option<&str>) -> Option<&str> {
        match self {
            MasonAssetBinField::Single(path) => Some(path),
            MasonAssetBinField::Named(paths) => {
                selector.and_then(|selector| paths.get(selector).map(String::as_str))
            }
        }
    }
}

/// Mason's `file` is usually a single string (the archive / binary
/// name) but a handful of packages list multiple files per target —
/// e.g. checkmake ships the binary plus its manpage:
/// `["checkmake-...amd64", "checkmake.1:man1/"]`. We accept both
/// shapes and the install runner just uses the first entry as the
/// downloadable artifact.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(untagged)]
pub enum MasonFileField {
    Single(String),
    Multiple(Vec<String>),
}

impl MasonFileField {
    /// Primary file name — the one the install runner downloads. For
    /// `Multiple`, returns the first entry (the others are extra
    /// payload like manpages).
    pub fn primary(&self) -> &str {
        match self {
            MasonFileField::Single(s) => s.as_str(),
            MasonFileField::Multiple(v) => v.first().map(|s| s.as_str()).unwrap_or(""),
        }
    }
}

/// `target` can be a single string or a vec of strings in Mason.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(untagged)]
pub enum MasonTargetField {
    Single(String),
    Multiple(Vec<String>),
    None,
}

impl Default for MasonTargetField {
    fn default() -> Self {
        MasonTargetField::None
    }
}

/// Local cache path: `$XDG_CACHE_HOME/neoism/extensions/mason-registry.json`.
pub fn mason_cache_path() -> PathBuf {
    let base = dirs::cache_dir().unwrap_or_else(|| {
        std::env::var("HOME")
            .map(|h| PathBuf::from(h).join(".cache"))
            .unwrap_or_else(|_| PathBuf::from(".cache"))
    });
    base.join("neoism")
        .join("extensions")
        .join("mason-registry.json")
}

/// Download the Mason registry snapshot (zip file). Returns raw bytes.
pub async fn fetch_mason_snapshot() -> Result<Vec<u8>, InstallError> {
    let ua = format!("neoism/{}", env!("CARGO_PKG_VERSION"));
    let client = reqwest::Client::builder()
        .user_agent(ua)
        .build()
        .map_err(|e| InstallError::Network(e.to_string()))?;
    let resp = client
        .get(MASON_SNAPSHOT_URL)
        .send()
        .await
        .map_err(|e| InstallError::Network(e.to_string()))?
        .error_for_status()
        .map_err(|e| InstallError::Network(e.to_string()))?;
    let bytes = resp
        .bytes()
        .await
        .map_err(|e| InstallError::Network(e.to_string()))?;
    Ok(bytes.to_vec())
}

/// Ensure a fresh-enough `registry.json` is cached on disk. Returns the path.
pub async fn ensure_cached_mason_registry() -> Result<PathBuf, InstallError> {
    let path = mason_cache_path();
    if is_fresh(&path) {
        return Ok(path);
    }
    let zip_bytes = fetch_mason_snapshot().await?;
    let json_bytes = extract_registry_json(&zip_bytes)?;
    write_atomic(&path, &json_bytes)?;
    Ok(path)
}

/// Load and parse a Mason registry JSON file from disk.
///
/// **Lenient**: bad individual entries are dropped (logged) rather
/// than killing the whole parse. Mason ships new shape variants
/// faster than we can track them (e.g. `file` was string-only until
/// some entries became arrays), and one schema-novel package
/// shouldn't make all ~1500 packages disappear from the Extensions
/// panel. The outer Vec parses first; each element is then converted
/// individually.
pub fn load_mason_registry(path: &Path) -> Result<MasonRegistry, InstallError> {
    let bytes = std::fs::read(path)?;
    parse_mason_registry(&bytes)
}

/// Parse the Mason registry from raw JSON bytes. See `load_mason_registry`.
pub fn parse_mason_registry(bytes: &[u8]) -> Result<MasonRegistry, InstallError> {
    let raw: Vec<serde_json::Value> = serde_json::from_slice(bytes)
        .map_err(|e| InstallError::ParseManifest(e.to_string()))?;
    let total = raw.len();
    let mut out = Vec::with_capacity(total);
    let mut skipped = 0usize;
    for value in raw {
        let name_hint = value
            .get("name")
            .and_then(|v| v.as_str())
            .unwrap_or("<unknown>")
            .to_string();
        match serde_json::from_value::<MasonPackage>(value) {
            Ok(pkg) => out.push(pkg),
            Err(err) => {
                skipped += 1;
                eprintln!("mason: skipping `{name_hint}`: {err}");
            }
        }
    }
    if skipped > 0 {
        eprintln!(
            "mason: parsed {} of {} packages ({} skipped due to schema mismatch)",
            out.len(),
            total,
            skipped
        );
    }
    Ok(out)
}

fn is_fresh(path: &Path) -> bool {
    let Ok(meta) = std::fs::metadata(path) else {
        return false;
    };
    let Ok(modified) = meta.modified() else {
        return false;
    };
    SystemTime::now()
        .duration_since(modified)
        .map(|age| age < CACHE_MAX_AGE)
        .unwrap_or(false)
}

fn extract_registry_json(zip_bytes: &[u8]) -> Result<Vec<u8>, InstallError> {
    let reader = std::io::Cursor::new(zip_bytes);
    let mut archive = zip::ZipArchive::new(reader)
        .map_err(|e| InstallError::ParseManifest(format!("zip open: {e}")))?;
    let mut entry = archive.by_name("registry.json").map_err(|e| {
        InstallError::ParseManifest(format!("registry.json missing: {e}"))
    })?;
    let mut out = Vec::with_capacity(entry.size() as usize);
    entry
        .read_to_end(&mut out)
        .map_err(|e| InstallError::Io(e))?;
    Ok(out)
}

// ---------------------------------------------------------------------------
// purl -> ExtensionManifest translation
// ---------------------------------------------------------------------------

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum PurlKind {
    Github,
    Npm,
    Pypi,
    Cargo,
    Golang,
    Gem,
}

/// Parse a Mason `pkg:` purl into (kind, name_path, version). `name_path` is
/// scheme-specific: `owner/repo` for github, `pkg_name` (possibly scoped) for
/// npm/pypi/cargo. URL-encoded `%2F` segments inside scoped npm names are
/// decoded so `%40anthropic%2Ffoo` becomes `@anthropic/foo`. Purl qualifiers
/// and subpaths are metadata, not part of the package-manager version.
fn parse_purl(purl: &str) -> Option<(PurlKind, String, String)> {
    let rest = purl.strip_prefix("pkg:")?;
    let (scheme, remainder) = rest.split_once('/')?;
    let (name_path_raw, version_and_metadata) = remainder.rsplit_once('@')?;
    let version = version_and_metadata
        .split_once(['?', '#'])
        .map_or(version_and_metadata, |(version, _)| version);
    if version.is_empty() || name_path_raw.is_empty() {
        return None;
    }
    let mut name_path = decode_purl_component(name_path_raw)?;
    let kind = match scheme {
        "github" => PurlKind::Github,
        "npm" => PurlKind::Npm,
        "pypi" => PurlKind::Pypi,
        "cargo" => PurlKind::Cargo,
        "golang" => PurlKind::Golang,
        "gem" => PurlKind::Gem,
        _ => return None,
    };
    if kind == PurlKind::Golang {
        if let Some((_, subpath)) = version_and_metadata.split_once('#') {
            let subpath = subpath.split_once('?').map_or(subpath, |(path, _)| path);
            let subpath = decode_purl_component(subpath)?;
            if !valid_purl_subpath(&subpath) {
                return None;
            }
            name_path.push('/');
            name_path.push_str(&subpath);
        }
    }
    Some((kind, name_path, version.to_string()))
}

fn valid_purl_subpath(subpath: &str) -> bool {
    !subpath.is_empty()
        && !subpath.starts_with('/')
        && !subpath.ends_with('/')
        && subpath
            .split('/')
            .all(|segment| !segment.is_empty() && segment != "." && segment != "..")
}

fn decode_purl_component(component: &str) -> Option<String> {
    let bytes = component.as_bytes();
    let mut decoded = Vec::with_capacity(bytes.len());
    let mut index = 0;
    while index < bytes.len() {
        if bytes[index] != b'%' {
            decoded.push(bytes[index]);
            index += 1;
            continue;
        }
        let high = *bytes.get(index + 1)?;
        let low = *bytes.get(index + 2)?;
        decoded.push(hex_value(high)? << 4 | hex_value(low)?);
        index += 3;
    }
    String::from_utf8(decoded).ok()
}

fn hex_value(byte: u8) -> Option<u8> {
    match byte {
        b'0'..=b'9' => Some(byte - b'0'),
        b'a'..=b'f' => Some(byte - b'a' + 10),
        b'A'..=b'F' => Some(byte - b'A' + 10),
        _ => None,
    }
}

fn mason_asset_to_github_assets(
    asset: &MasonAsset,
    package_bins: &BTreeMap<String, String>,
    fallback_bin: &str,
    bin_selector: Option<&str>,
    version: &str,
) -> Vec<GithubAsset> {
    let default_bin = normalize_direct_executable_path(
        asset
            .bin
            .as_ref()
            .and_then(|bin| bin.resolve(bin_selector))
            .filter(|bin| !bin.is_empty())
            .unwrap_or(fallback_bin),
    );
    let executables = package_bins
        .iter()
        .filter_map(|(name, expression)| {
            resolve_mason_asset_executable(asset, expression, &default_bin)
                .map(|path| (name.clone(), path))
        })
        .collect::<BTreeMap<_, _>>();
    let bin = executables
        .get(fallback_bin)
        .and_then(|recipe| executable_recipe_payload(recipe))
        .unwrap_or_else(|| default_bin.clone());
    // Mason asset names are Liquid templates, not literal release filenames.
    // The registry commonly uses `{{version}}` and
    // `{{ version | strip_prefix "v" }}`.  Sending those braces to GitHub
    // produces a guaranteed 404 (docker-language-server was one visible
    // example).  Mason also permits `archive:destination` in `file`; only the
    // part before the colon is the downloadable asset name.
    let file = render_asset_file(asset.file.primary(), version);
    match &asset.target {
        MasonTargetField::Single(t) => vec![GithubAsset {
            target: t.clone(),
            file: file.clone(),
            bin: bin.clone(),
            executables: executables.clone(),
        }],
        MasonTargetField::Multiple(targets) => targets
            .iter()
            .map(|t| GithubAsset {
                target: t.clone(),
                file: file.clone(),
                bin: bin.clone(),
                executables: executables.clone(),
            })
            .collect(),
        MasonTargetField::None => vec![GithubAsset {
            target: String::new(),
            file,
            bin: bin.clone(),
            executables,
        }],
    }
}

fn resolve_mason_asset_executable(
    asset: &MasonAsset,
    expression: &str,
    primary_path: &str,
) -> Option<String> {
    let compact = expression.split_whitespace().collect::<String>();
    if compact == "{{source.asset.bin}}" {
        return asset
            .bin
            .as_ref()
            .and_then(|bin| bin.resolve(None))
            .map(normalize_direct_executable_path);
    }
    if let Some(selector) = compact
        .strip_prefix("{{source.asset.bin.")
        .and_then(|selector| selector.strip_suffix("}}"))
    {
        return asset
            .bin
            .as_ref()
            .and_then(|bin| bin.resolve(Some(selector)))
            .map(normalize_direct_executable_path);
    }
    if compact == "{{source.asset.file}}" {
        return Some(primary_path.to_string());
    }
    if let Some(path) = compact.strip_prefix("exec:") {
        return (!path.is_empty()).then(|| path.to_string());
    }
    // Preserve only the interpreter recipes the install runner knows how to
    // materialize as fixed argv launchers. This is data, not a shell command.
    if ["dotnet:", "java-jar:", "node:", "ruby:", "php:"]
        .iter()
        .any(|prefix| compact.starts_with(prefix))
        && !compact.contains("{{")
    {
        return Some(compact);
    }
    // Unresolved Liquid/build expressions and unknown recipe schemes are not
    // executable by the generic release runner.
    if compact.contains(':') || compact.contains("{{") || compact.is_empty() {
        return None;
    }
    Some(compact.trim_start_matches("./").to_string())
}

fn executable_recipe_payload(recipe: &str) -> Option<String> {
    for prefix in ["dotnet:", "java-jar:", "node:", "ruby:", "php:"] {
        if let Some(payload) = recipe.strip_prefix(prefix) {
            return (!payload.is_empty()).then(|| payload.to_string());
        }
    }
    (!recipe.is_empty()).then(|| recipe.to_string())
}

fn normalize_direct_executable_path(path: &str) -> String {
    path.strip_prefix("exec:")
        .unwrap_or(path)
        .trim_start_matches("./")
        .to_string()
}

fn render_asset_file(template: &str, version: &str) -> String {
    let artifact = template.split_once(':').map_or(template, |(file, _)| file);
    let mut rendered = artifact.to_string();
    while let Some(start) = rendered.find("{{") {
        let Some(relative_end) = rendered[start + 2..].find("}}") else {
            break;
        };
        let end = start + 2 + relative_end;
        let expression = rendered[start + 2..end].trim();
        let replacement = render_version_expression(expression, version);
        rendered.replace_range(start..end + 2, replacement);
    }
    rendered
}

fn render_version_expression<'a>(expression: &str, version: &'a str) -> &'a str {
    let normalized = expression.replace("||", "|");
    let mut parts = normalized.split('|').map(str::trim);
    if parts.next() != Some("version") {
        return version;
    }
    let Some(filter) = parts.next() else {
        return version;
    };
    let Some(prefix) = filter
        .strip_prefix("strip_prefix")
        .map(str::trim)
        .and_then(|value| value.strip_prefix('"'))
        .and_then(|value| value.strip_suffix('"'))
    else {
        return version;
    };
    version.strip_prefix(prefix).unwrap_or(version)
}

/// Pick the bin name we expose. Prefer a key matching `pkg.name`, otherwise
/// fall back to the first sorted key, otherwise `pkg.name` itself.
fn resolve_bin_name(pkg: &MasonPackage) -> String {
    if pkg.bin.is_empty() {
        return pkg.name.clone();
    }
    if pkg.bin.len() == 1 {
        return pkg
            .bin
            .keys()
            .next()
            .cloned()
            .unwrap_or_else(|| pkg.name.clone());
    }
    if let Some(name) = pkg
        .bin
        .keys()
        .find(|name| name.ends_with("-langserver") || name.contains("language-server"))
    {
        return name.clone();
    }
    if pkg.bin.contains_key(&pkg.name) {
        return pkg.name.clone();
    }
    pkg.bin
        .keys()
        .next()
        .cloned()
        .unwrap_or_else(|| pkg.name.clone())
}

fn asset_bin_selector<'a>(pkg: &'a MasonPackage, bin_name: &str) -> Option<&'a str> {
    let expression = pkg.bin.get(bin_name)?.trim();
    expression
        .strip_prefix("{{source.asset.bin.")
        .and_then(|selector| selector.strip_suffix("}}"))
        .map(str::trim)
        .filter(|selector| !selector.is_empty())
}

/// Translate a single Mason package into our internal ExtensionManifest.
/// Returns Err for unsupported install kinds (purl schemes we don't handle).
pub fn package_to_manifest(
    pkg: &MasonPackage,
) -> Result<ExtensionManifest, InstallError> {
    let (kind, name_path, version) = parse_purl(&pkg.source.id).ok_or_else(|| {
        InstallError::ParseManifest(format!("unsupported purl: {}", pkg.source.id))
    })?;

    let bin_name = resolve_bin_name(pkg);
    let bin_selector = asset_bin_selector(pkg, &bin_name);

    let install = match kind {
        PurlKind::Github => {
            let (owner, repo) = name_path.split_once('/').ok_or_else(|| {
                InstallError::ParseManifest(format!(
                    "github purl missing owner/repo: {}",
                    pkg.source.id
                ))
            })?;
            let asset_field = pkg.source.asset.as_ref().ok_or_else(|| {
                InstallError::ParseManifest(
                    "github source missing asset list".to_string(),
                )
            })?;
            let assets: Vec<GithubAsset> = asset_field
                .clone()
                .into_vec()
                .iter()
                .flat_map(|asset| {
                    mason_asset_to_github_assets(
                        asset,
                        &pkg.bin,
                        &bin_name,
                        bin_selector,
                        &version,
                    )
                })
                .collect();
            if assets.is_empty() {
                return Err(InstallError::ParseManifest(
                    "github source asset list resolved to zero entries".to_string(),
                ));
            }
            InstallKind::GithubRelease {
                owner: owner.to_string(),
                repo: repo.to_string(),
                tag: version.clone(),
                assets,
            }
        }
        PurlKind::Npm => InstallKind::Npm {
            package: name_path,
            version: version.clone(),
            extra_packages: pkg.source.extra_packages.clone(),
        },
        PurlKind::Pypi => InstallKind::Pip {
            package: name_path,
            version: version.clone(),
        },
        PurlKind::Cargo => InstallKind::Cargo {
            crate_name: name_path,
            version: version.clone(),
            features: Vec::new(),
        },
        PurlKind::Golang => InstallKind::Go {
            package: name_path,
            version: version.clone(),
        },
        PurlKind::Gem => InstallKind::Gem {
            package: name_path,
            version: version.clone(),
            extra_packages: pkg.source.extra_packages.clone(),
        },
    };

    let run = Some(RunSpec {
        command: vec![bin_name],
        env: BTreeMap::new(),
    });

    Ok(ExtensionManifest {
        id: pkg.name.clone(),
        name: pkg.name.clone(),
        version,
        description: pkg.description.trim().to_string(),
        author: String::new(),
        downloads: None,
        categories: pkg.categories.clone(),
        languages: pkg.languages.clone(),
        repository_url: None,
        homepage: pkg.homepage.clone(),
        executables: pkg.bin.keys().cloned().collect(),
        install,
        run,
        env_keys: Vec::new(),
    })
}

/// Convenience: translate the whole registry, dropping packages whose source
/// purl we don't support yet. Skipped packages are logged so the registry
/// generator step can audit coverage.
pub fn translate_registry(registry: &MasonRegistry) -> Vec<ExtensionManifest> {
    let mut out = Vec::with_capacity(registry.len());
    for pkg in registry {
        match package_to_manifest(pkg) {
            Ok(m) => out.push(m),
            Err(e) => {
                eprintln!("mason: skipping `{}` ({}): {}", pkg.name, pkg.source.id, e)
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    const FIXTURE: &str = r#"[
        {
            "name": "rust-analyzer",
            "description": "Rust compiler front-end for IDEs.",
            "categories": ["LSP"],
            "languages": ["Rust"],
            "homepage": "https://rust-analyzer.github.io",
            "source": {
                "id": "pkg:github/rust-lang/rust-analyzer@2024-01-01",
                "asset": [
                    {
                        "target": "linux_x64_gnu",
                        "file": "rust-analyzer-x86_64-unknown-linux-gnu.gz:rust-analyzer",
                        "bin": "rust-analyzer"
                    },
                    {
                        "target": ["darwin_arm64", "darwin_x64"],
                        "file": "rust-analyzer-aarch64-apple-darwin.gz:rust-analyzer",
                        "bin": "rust-analyzer"
                    }
                ]
            },
            "bin": { "rust-analyzer": "./rust-analyzer" }
        },
        {
            "name": "pyright",
            "description": "Static type checker for Python.",
            "categories": ["LSP"],
            "languages": ["Python"],
            "source": {
                "id": "pkg:npm/pyright@1.1.0"
            },
            "bin": { "pyright-langserver": "npx:pyright-langserver" }
        },
        {
            "name": "black",
            "description": "Python formatter.",
            "categories": ["Formatter"],
            "languages": ["Python"],
            "source": {
                "id": "pkg:pypi/black@24.0.0"
            },
            "bin": { "black": "venv:black" }
        }
    ]"#;

    #[test]
    fn parses_mixed_source_kinds() {
        let reg = parse_mason_registry(FIXTURE.as_bytes()).unwrap();
        assert_eq!(reg.len(), 3);

        let ra = &reg[0];
        assert_eq!(ra.name, "rust-analyzer");
        assert!(ra.source.id.starts_with("pkg:github/"));
        let asset_field = ra.source.asset.clone().expect("asset present");
        let assets = asset_field.into_vec();
        assert_eq!(assets.len(), 2);
        assert_eq!(
            assets[0].file.primary(),
            "rust-analyzer-x86_64-unknown-linux-gnu.gz:rust-analyzer"
        );
        match &assets[0].target {
            MasonTargetField::Single(s) => assert_eq!(s, "linux_x64_gnu"),
            other => panic!("expected single target, got {:?}", other),
        }
        match &assets[1].target {
            MasonTargetField::Multiple(v) => {
                assert_eq!(
                    v,
                    &vec!["darwin_arm64".to_string(), "darwin_x64".to_string()]
                );
            }
            other => panic!("expected multiple targets, got {:?}", other),
        }

        let py = &reg[1];
        assert_eq!(py.name, "pyright");
        assert!(py.source.id.starts_with("pkg:npm/"));
        assert!(py.source.asset.is_none());

        let bl = &reg[2];
        assert_eq!(bl.name, "black");
        assert!(bl.source.id.starts_with("pkg:pypi/"));
        assert!(bl.languages.contains(&"Python".to_string()));
    }

    #[test]
    fn round_trip_via_disk() {
        let dir = std::env::temp_dir().join(format!(
            "neoism-mason-test-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_nanos())
                .unwrap_or(0)
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("registry.json");
        std::fs::write(&path, FIXTURE).unwrap();
        let reg = load_mason_registry(&path).unwrap();
        assert_eq!(reg.len(), 3);
    }

    #[test]
    fn unknown_fields_are_ignored() {
        let raw = r#"[
            {
                "name": "weird",
                "description": "",
                "unknown_top_level": 42,
                "source": {
                    "id": "pkg:cargo/whatever@1.0.0",
                    "future_field": "ignored"
                }
            }
        ]"#;
        let reg = parse_mason_registry(raw.as_bytes()).unwrap();
        assert_eq!(reg.len(), 1);
        assert_eq!(reg[0].name, "weird");
        assert!(reg[0].source.asset.is_none());
    }

    fn single_pkg(json: &str) -> MasonPackage {
        let reg: MasonRegistry = serde_json::from_str(&format!("[{json}]")).unwrap();
        reg.into_iter().next().unwrap()
    }

    #[test]
    fn translates_github_source() {
        let pkg = single_pkg(
            r#"{
                "name": "rust-analyzer",
                "description": "  Rust LSP.  ",
                "categories": ["LSP"],
                "languages": ["Rust"],
                "homepage": "https://rust-analyzer.github.io",
                "source": {
                    "id": "pkg:github/rust-lang/rust-analyzer@2026-05-25",
                    "asset": [
                        {
                            "target": "linux_x64_gnu",
                            "file": "rust-analyzer-x86_64-unknown-linux-gnu.gz",
                            "bin": "rust-analyzer"
                        },
                        {
                            "target": ["darwin_arm64", "darwin_x64"],
                            "file": "rust-analyzer-aarch64-apple-darwin.gz",
                            "bin": "rust-analyzer"
                        }
                    ]
                },
                "bin": { "rust-analyzer": "./rust-analyzer" }
            }"#,
        );
        let m = package_to_manifest(&pkg).expect("translate ok");
        assert_eq!(m.id, "rust-analyzer");
        assert_eq!(m.version, "2026-05-25");
        assert_eq!(m.description, "Rust LSP.");
        assert_eq!(m.languages, vec!["Rust".to_string()]);
        assert_eq!(
            m.homepage.as_deref(),
            Some("https://rust-analyzer.github.io")
        );
        assert_eq!(
            m.run.as_ref().unwrap().command,
            vec!["rust-analyzer".to_string()]
        );
        match m.install {
            InstallKind::GithubRelease {
                owner,
                repo,
                tag,
                assets,
            } => {
                assert_eq!(owner, "rust-lang");
                assert_eq!(repo, "rust-analyzer");
                assert_eq!(tag, "2026-05-25");
                let targets: Vec<&str> =
                    assets.iter().map(|a| a.target.as_str()).collect();
                assert!(targets.contains(&"linux_x64_gnu"));
                assert!(targets.contains(&"darwin_arm64"));
                assert!(targets.contains(&"darwin_x64"));
                assert_eq!(assets.len(), 3);
                for a in &assets {
                    assert_eq!(a.bin, "rust-analyzer");
                }
            }
            other => panic!("expected github release, got {:?}", other),
        }
    }

    #[test]
    fn renders_mason_version_templates_and_archive_destination() {
        assert_eq!(
            render_asset_file(
                "server-{{ version | strip_prefix \"v\" }}.tar.gz:server",
                "v1.2.3",
            ),
            "server-1.2.3.tar.gz"
        );
        assert_eq!(
            render_asset_file("server-{{version}}-darwin-arm64", "v1.2.3"),
            "server-v1.2.3-darwin-arm64"
        );
    }

    #[test]
    fn translates_npm_source() {
        let pkg = single_pkg(
            r#"{
                "name": "pyright",
                "description": "Static type checker for Python.",
                "categories": ["LSP"],
                "languages": ["Python"],
                "source": {
                    "id": "pkg:npm/pyright@1.1.409",
                    "extra_packages": ["typescript@6.0.3", "@vue/typescript-plugin"]
                },
                "bin": { "pyright": "npm:pyright", "pyright-langserver": "npm:pyright-langserver" }
            }"#,
        );
        let m = package_to_manifest(&pkg).expect("translate ok");
        match m.install {
            InstallKind::Npm {
                package,
                version,
                extra_packages,
            } => {
                assert_eq!(package, "pyright");
                assert_eq!(version, "1.1.409");
                assert_eq!(
                    extra_packages,
                    vec![
                        "typescript@6.0.3".to_string(),
                        "@vue/typescript-plugin".to_string()
                    ]
                );
            }
            other => panic!("expected npm, got {:?}", other),
        }
        assert_eq!(
            m.run.unwrap().command,
            vec!["pyright-langserver".to_string()]
        );
    }

    #[test]
    fn translates_pypi_source() {
        let pkg = single_pkg(
            r#"{
                "name": "black",
                "description": "Python formatter.",
                "categories": ["Formatter"],
                "languages": ["Python"],
                "source": { "id": "pkg:pypi/black@24.0" },
                "bin": { "black": "venv:black" }
            }"#,
        );
        let m = package_to_manifest(&pkg).expect("translate ok");
        match m.install {
            InstallKind::Pip { package, version } => {
                assert_eq!(package, "black");
                assert_eq!(version, "24.0");
            }
            other => panic!("expected pip, got {:?}", other),
        }
    }

    #[test]
    fn translates_cargo_source() {
        let pkg = single_pkg(
            r#"{
                "name": "taplo",
                "description": "TOML toolkit.",
                "categories": ["Formatter"],
                "languages": ["TOML"],
                "source": { "id": "pkg:cargo/taplo-cli@0.9" },
                "bin": { "taplo": "cargo:taplo-cli" }
            }"#,
        );
        let m = package_to_manifest(&pkg).expect("translate ok");
        match m.install {
            InstallKind::Cargo {
                crate_name,
                version,
                features,
            } => {
                assert_eq!(crate_name, "taplo-cli");
                assert_eq!(version, "0.9");
                assert!(features.is_empty());
            }
            other => panic!("expected cargo, got {:?}", other),
        }
    }

    #[test]
    fn translates_go_source_and_appends_purl_subpath() {
        let pkg = single_pkg(
            r#"{
                "name": "goimports",
                "description": "Go imports formatter",
                "source": { "id": "pkg:golang/golang.org/x/tools@v0.48.0#cmd/goimports" },
                "bin": { "goimports": "golang:goimports" }
            }"#,
        );
        let manifest = package_to_manifest(&pkg).expect("translate Go package");
        match manifest.install {
            InstallKind::Go { package, version } => {
                assert_eq!(package, "golang.org/x/tools/cmd/goimports");
                assert_eq!(version, "v0.48.0");
            }
            other => panic!("expected Go install, got {other:?}"),
        }
        assert_eq!(manifest.run.unwrap().command, vec!["goimports"]);
    }

    #[test]
    fn translates_gem_source_with_companion_packages() {
        let pkg = single_pkg(
            r#"{
                "name": "ruby-lsp",
                "description": "Ruby language server",
                "source": {
                    "id": "pkg:gem/ruby-lsp@0.26.10",
                    "extra_packages": ["ruby-lsp-rails"]
                },
                "bin": { "ruby-lsp": "gem:ruby-lsp" }
            }"#,
        );
        let manifest = package_to_manifest(&pkg).expect("translate gem package");
        match manifest.install {
            InstallKind::Gem {
                package,
                version,
                extra_packages,
            } => {
                assert_eq!(package, "ruby-lsp");
                assert_eq!(version, "0.26.10");
                assert_eq!(extra_packages, vec!["ruby-lsp-rails"]);
            }
            other => panic!("expected gem install, got {other:?}"),
        }
    }

    #[test]
    fn generic_purl_remains_explicitly_unsupported() {
        let pkg = single_pkg(
            r#"{
                "name": "generic-build",
                "description": "requires a registry-specific build recipe",
                "source": { "id": "pkg:generic/example@1.0.0" }
            }"#,
        );
        assert!(package_to_manifest(&pkg).is_err());
    }

    #[test]
    fn translate_registry_drops_unsupported_silently() {
        let raw = r#"[
            {
                "name": "pyright",
                "description": "",
                "source": { "id": "pkg:npm/pyright@1.0.0" },
                "bin": { "pyright": "npm:pyright" }
            },
            {
                "name": "some-gem",
                "description": "",
                "source": { "id": "pkg:gem/some-gem@1.0.0" }
            },
            {
                "name": "generic-build",
                "description": "",
                "source": { "id": "pkg:generic/example@1.0.0" }
            },
            {
                "name": "black",
                "description": "",
                "source": { "id": "pkg:pypi/black@24.0" },
                "bin": { "black": "venv:black" }
            }
        ]"#;
        let reg = parse_mason_registry(raw.as_bytes()).unwrap();
        let out = translate_registry(&reg);
        assert_eq!(out.len(), 3);
        let ids: Vec<&str> = out.iter().map(|m| m.id.as_str()).collect();
        assert!(ids.contains(&"pyright"));
        assert!(ids.contains(&"black"));
        assert!(ids.contains(&"some-gem"));
        assert!(!ids.contains(&"generic-build"));
    }

    #[test]
    fn bin_name_prefers_language_server_when_multiple() {
        let pkg = single_pkg(
            r#"{
                "name": "pyright",
                "description": "",
                "source": { "id": "pkg:npm/pyright@1.1.0" },
                "bin": {
                    "pyright-langserver": "npm:pyright-langserver",
                    "pyright": "npm:pyright"
                }
            }"#,
        );
        let m = package_to_manifest(&pkg).unwrap();
        assert_eq!(
            m.run.unwrap().command,
            vec!["pyright-langserver".to_string()]
        );
    }

    #[test]
    fn github_asset_without_asset_bin_uses_manifest_bin() {
        let pkg = single_pkg(
            r#"{
                "name": "ast-grep",
                "description": "",
                "source": {
                    "id": "pkg:github/ast-grep/ast-grep@0.43.0",
                    "asset": [
                        { "target": "linux_x64_gnu", "file": "app-x86_64-unknown-linux-gnu.zip" }
                    ]
                },
                "bin": {
                    "ast-grep": "{{source.asset.ext}}",
                    "sg": "{{source.asset.ext}}"
                }
            }"#,
        );
        let m = package_to_manifest(&pkg).unwrap();
        let InstallKind::GithubRelease { assets, .. } = m.install else {
            panic!("expected github release")
        };
        assert_eq!(assets[0].bin, "ast-grep");
    }

    #[test]
    fn bin_name_falls_back_to_first_sorted_when_no_match() {
        let pkg = single_pkg(
            r#"{
                "name": "mything",
                "description": "",
                "source": { "id": "pkg:npm/mything@1.0.0" },
                "bin": {
                    "zzz-cli": "npm:zzz-cli",
                    "aaa-cli": "npm:aaa-cli"
                }
            }"#,
        );
        let m = package_to_manifest(&pkg).unwrap();
        // BTreeMap keys iterate in sorted order, so "aaa-cli" should win.
        assert_eq!(m.run.unwrap().command, vec!["aaa-cli".to_string()]);
    }

    #[test]
    fn purl_parse_handles_scoped_npm_name() {
        let parsed = parse_purl("pkg:npm/@scope%2Ffoo@2.0.0").unwrap();
        assert_eq!(parsed.0, PurlKind::Npm);
        assert_eq!(parsed.1, "@scope/foo");
        assert_eq!(parsed.2, "2.0.0");
    }

    #[test]
    fn purl_parse_decodes_scopes_and_strips_qualifiers_and_subpaths() {
        let npm = parse_purl("pkg:npm/%40vue%2Flanguage-server@3.3.7").unwrap();
        assert_eq!(npm.0, PurlKind::Npm);
        assert_eq!(npm.1, "@vue/language-server");
        assert_eq!(npm.2, "3.3.7");

        let cargo = parse_purl(
            "pkg:cargo/nil@2025-06-13?repository_url=https://github.com/oxalica/nil#bin",
        )
        .unwrap();
        assert_eq!(cargo.0, PurlKind::Cargo);
        assert_eq!(cargo.1, "nil");
        assert_eq!(cargo.2, "2025-06-13");
    }

    #[test]
    fn translates_named_asset_binary_used_by_elixir_ls() {
        let pkg = single_pkg(
            r#"{
                "name": "elixir-ls",
                "description": "Elixir language server.",
                "categories": ["LSP", "DAP"],
                "languages": ["Elixir"],
                "source": {
                    "id": "pkg:github/elixir-lsp/elixir-ls@v0.31.1",
                    "asset": [
                        {
                            "target": "unix",
                            "file": "elixir-ls-{{version}}.zip",
                            "bin": {
                                "lsp": "language_server.sh",
                                "dap": "debug_adapter.sh"
                            }
                        }
                    ]
                },
                "bin": {
                    "elixir-ls": "{{source.asset.bin.lsp}}",
                    "elixir-ls-debugger": "{{source.asset.bin.dap}}"
                }
            }"#,
        );
        let manifest = package_to_manifest(&pkg).expect("translate ElixirLS");
        assert_eq!(
            manifest.run.as_ref().unwrap().command,
            vec!["elixir-ls".to_string()]
        );
        let InstallKind::GithubRelease { assets, .. } = manifest.install else {
            panic!("expected GitHub release")
        };
        assert_eq!(assets.len(), 1);
        assert_eq!(assets[0].bin, "language_server.sh");
        assert_eq!(
            assets[0].executables.get("elixir-ls").map(String::as_str),
            Some("language_server.sh")
        );
        assert_eq!(
            assets[0]
                .executables
                .get("elixir-ls-debugger")
                .map(String::as_str),
            Some("debug_adapter.sh")
        );
    }

    #[test]
    fn translates_fixed_interpreter_recipe_without_treating_it_as_shell() {
        let pkg = single_pkg(
            r#"{
                "name": "omnisharp",
                "description": "C# language server",
                "categories": ["LSP"],
                "languages": ["C#"],
                "source": {
                    "id": "pkg:github/OmniSharp/omnisharp-roslyn@v1.39.15",
                    "asset": [{
                        "target": "linux_x64",
                        "file": "omnisharp-linux-x64-net6.0.zip:libexec/"
                    }]
                },
                "bin": { "OmniSharp": "dotnet:libexec/OmniSharp.dll" }
            }"#,
        );
        let manifest = package_to_manifest(&pkg).expect("translate OmniSharp");
        let InstallKind::GithubRelease { assets, .. } = manifest.install else {
            panic!("expected GitHub release")
        };
        assert_eq!(assets[0].bin, "libexec/OmniSharp.dll");
        assert_eq!(
            assets[0].executables.get("OmniSharp").map(String::as_str),
            Some("dotnet:libexec/OmniSharp.dll")
        );
    }

    #[test]
    fn purl_parse_rejects_malformed() {
        assert!(parse_purl("not-a-purl").is_none());
        assert!(parse_purl("pkg:github/owner/repo").is_none());
        assert!(parse_purl("pkg:github/foo@").is_none());
    }

    #[test]
    fn github_source_missing_asset_errors() {
        let pkg = single_pkg(
            r#"{
                "name": "lonely",
                "description": "",
                "source": { "id": "pkg:github/owner/repo@v1" },
                "bin": { "lonely": "./lonely" }
            }"#,
        );
        let err = package_to_manifest(&pkg).unwrap_err();
        let msg = format!("{err}");
        assert!(msg.contains("asset"), "got: {msg}");
    }

    #[test]
    fn cached_representative_lsp_packages_have_real_host_install_plans() {
        let path = mason_cache_path();
        if !path.is_file() {
            return;
        }
        let registry = load_mason_registry(&path).expect("cached Mason registry");
        for package_id in [
            "gopls",
            "solargraph",
            "nil",
            "omnisharp",
            "zls",
            "elixir-ls",
            "docker-language-server",
        ] {
            let package = registry
                .iter()
                .find(|package| package.name == package_id)
                .unwrap_or_else(|| panic!("cached Mason registry lost `{package_id}`"));
            let manifest = package_to_manifest(package).unwrap_or_else(|error| {
                panic!("cannot translate `{package_id}`: {error}")
            });
            assert!(
                crate::install_runner::supported_on_current_host(&manifest),
                "`{package_id}` translated but has no truthful install plan for {}",
                crate::install_runner::current_target()
            );
        }
    }
}
