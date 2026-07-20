use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::time::Duration;

use serde::{Deserialize, Serialize};

#[derive(Debug, Deserialize)]
struct CuratedEntry {
    id: String,
    #[serde(default)]
    npm_package: Option<String>,
    #[serde(default)]
    pip_package: Option<String>,
    bin_name: String,
    #[serde(default)]
    command_args: Vec<String>,
    #[serde(default)]
    categories: Vec<String>,
    #[serde(default)]
    env_keys: Vec<String>,
}

#[derive(Debug, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
enum InstallKind {
    Npm { package: String, version: String },
    Pip { package: String, version: String },
}

#[derive(Debug, Serialize)]
struct RunSpec {
    command: Vec<String>,
    env: BTreeMap<String, String>,
}

#[derive(Debug, Serialize)]
struct ExtensionManifest {
    id: String,
    name: String,
    version: String,
    description: String,
    author: String,
    downloads: Option<u64>,
    categories: Vec<String>,
    repository_url: Option<String>,
    executables: Vec<String>,
    install: InstallKind,
    run: Option<RunSpec>,
    env_keys: Vec<String>,
}

fn main() {
    let manifest_dir = PathBuf::from(std::env::var_os("CARGO_MANIFEST_DIR").unwrap());
    let curated_path = manifest_dir.join("data/mcp_curated.json");
    let registry_path = manifest_dir.join("data/mcp_registry.json");

    println!("cargo:rerun-if-changed=data/mcp_curated.json");
    println!("cargo:rerun-if-changed=build.rs");

    // If a generated registry already exists, do nothing. The curated-input
    // rerun trigger above forces regeneration when the curated list changes,
    // so this only short-circuits steady-state rebuilds.
    if registry_path.exists() {
        return;
    }

    let curated_raw = match std::fs::read(&curated_path) {
        Ok(b) => b,
        Err(e) => {
            eprintln!(
                "cargo:warning=neoism-extensions: cannot read {}: {e}",
                curated_path.display()
            );
            return;
        }
    };
    let curated: Vec<CuratedEntry> = match serde_json::from_slice(&curated_raw) {
        Ok(v) => v,
        Err(e) => panic!("mcp_curated.json parse error: {e}"),
    };

    let agent = ureq::AgentBuilder::new()
        .timeout_connect(Duration::from_secs(5))
        .timeout(Duration::from_secs(20))
        .build();

    let mut out: Vec<ExtensionManifest> =
        curated.into_iter().map(|e| enrich(&agent, e)).collect();
    out.sort_by(|a, b| a.id.cmp(&b.id));

    let body = serde_json::to_string_pretty(&out).expect("serialize registry");
    if let Some(parent) = registry_path.parent() {
        std::fs::create_dir_all(parent).expect("create data dir");
    }
    std::fs::write(&registry_path, format!("{body}\n")).expect("write mcp_registry.json");
}

fn enrich(agent: &ureq::Agent, entry: CuratedEntry) -> ExtensionManifest {
    let mut command = Vec::with_capacity(1 + entry.command_args.len());
    command.push(entry.bin_name.clone());
    command.extend(entry.command_args.iter().cloned());
    let executables = vec![entry.bin_name.clone()];
    let run = Some(RunSpec {
        command,
        env: BTreeMap::new(),
    });

    let (install, version, description, author, downloads, repository_url) =
        if let Some(pkg) = entry.npm_package.as_ref() {
            let meta = fetch_npm(agent, pkg).unwrap_or_default();
            let dl = fetch_npm_downloads(agent, pkg);
            let version = meta.version.unwrap_or_else(|| "0.0.0".to_string());
            (
                InstallKind::Npm {
                    package: pkg.clone(),
                    version: version.clone(),
                },
                version,
                meta.description.unwrap_or_default(),
                meta.author.unwrap_or_default(),
                dl,
                meta.repository_url,
            )
        } else if let Some(pkg) = entry.pip_package.as_ref() {
            let meta = fetch_pypi(agent, pkg).unwrap_or_default();
            let version = meta.version.unwrap_or_else(|| "0.0.0".to_string());
            (
                InstallKind::Pip {
                    package: pkg.clone(),
                    version: version.clone(),
                },
                version,
                meta.description.unwrap_or_default(),
                meta.author.unwrap_or_default(),
                None,
                meta.repository_url,
            )
        } else {
            panic!(
                "curated entry `{}` has neither npm_package nor pip_package",
                entry.id
            );
        };

    let name = display_name(&entry.id);
    ExtensionManifest {
        id: entry.id,
        name,
        version,
        description,
        author,
        downloads,
        categories: entry.categories,
        repository_url,
        executables,
        install,
        run,
        env_keys: entry.env_keys,
    }
}

#[derive(Debug, Default)]
struct PkgMeta {
    version: Option<String>,
    description: Option<String>,
    author: Option<String>,
    repository_url: Option<String>,
}

fn fetch_npm(agent: &ureq::Agent, pkg: &str) -> Option<PkgMeta> {
    let encoded = encode_npm_pkg(pkg);
    let url = format!("https://registry.npmjs.org/{encoded}");
    let body = match agent.get(&url).call() {
        Ok(resp) => resp.into_string().ok()?,
        Err(e) => {
            eprintln!("cargo:warning=neoism-extensions: npm fetch {pkg} failed: {e}");
            return None;
        }
    };
    let v: serde_json::Value = serde_json::from_str(&body).ok()?;
    let latest = v
        .pointer("/dist-tags/latest")
        .and_then(|x| x.as_str())
        .map(|s| s.to_string());
    let version_obj = latest
        .as_ref()
        .and_then(|tag| v.pointer(&format!("/versions/{tag}")));
    let description = version_obj
        .and_then(|o| o.get("description"))
        .or_else(|| v.get("description"))
        .and_then(|x| x.as_str())
        .map(|s| s.to_string());
    let author = version_obj
        .and_then(|o| o.get("author"))
        .or_else(|| v.get("author"))
        .map(author_to_string)
        .unwrap_or_default();
    let repository_url = version_obj
        .and_then(|o| o.get("repository"))
        .or_else(|| v.get("repository"))
        .and_then(repo_to_url);
    Some(PkgMeta {
        version: latest,
        description,
        author: if author.is_empty() {
            None
        } else {
            Some(author)
        },
        repository_url,
    })
}

fn fetch_npm_downloads(agent: &ureq::Agent, pkg: &str) -> Option<u64> {
    let encoded = encode_npm_pkg(pkg);
    let url = format!("https://api.npmjs.org/downloads/point/last-week/{encoded}");
    let body = match agent.get(&url).call() {
        Ok(resp) => resp.into_string().ok()?,
        Err(_) => return None,
    };
    let v: serde_json::Value = serde_json::from_str(&body).ok()?;
    v.get("downloads").and_then(|x| x.as_u64())
}

fn fetch_pypi(agent: &ureq::Agent, pkg: &str) -> Option<PkgMeta> {
    let url = format!("https://pypi.org/pypi/{pkg}/json");
    let body = match agent.get(&url).call() {
        Ok(resp) => resp.into_string().ok()?,
        Err(e) => {
            eprintln!("cargo:warning=neoism-extensions: pypi fetch {pkg} failed: {e}");
            return None;
        }
    };
    let v: serde_json::Value = serde_json::from_str(&body).ok()?;
    let info = v.get("info")?;
    let version = info
        .get("version")
        .and_then(|x| x.as_str())
        .map(|s| s.to_string());
    let description = info
        .get("summary")
        .and_then(|x| x.as_str())
        .map(|s| s.to_string());
    let author = info
        .get("author")
        .and_then(|x| x.as_str())
        .map(|s| s.to_string())
        .filter(|s| !s.is_empty());
    let repository_url = info
        .get("home_page")
        .and_then(|x| x.as_str())
        .map(|s| s.to_string())
        .filter(|s| !s.is_empty())
        .or_else(|| {
            info.get("project_urls").and_then(|p| {
                p.get("Homepage")
                    .or_else(|| p.get("Repository"))
                    .or_else(|| p.get("Source"))
                    .and_then(|x| x.as_str())
                    .map(|s| s.to_string())
            })
        });
    Some(PkgMeta {
        version,
        description,
        author,
        repository_url,
    })
}

fn encode_npm_pkg(pkg: &str) -> String {
    // npm registry: scoped packages need the slash url-encoded.
    pkg.replace('/', "%2F")
}

fn author_to_string(v: &serde_json::Value) -> String {
    match v {
        serde_json::Value::String(s) => s.clone(),
        serde_json::Value::Object(map) => map
            .get("name")
            .and_then(|x| x.as_str())
            .unwrap_or_default()
            .to_string(),
        _ => String::new(),
    }
}

fn repo_to_url(v: &serde_json::Value) -> Option<String> {
    match v {
        serde_json::Value::String(s) => Some(s.clone()),
        serde_json::Value::Object(map) => map
            .get("url")
            .and_then(|x| x.as_str())
            .map(|s| s.to_string()),
        _ => None,
    }
}

fn display_name(id: &str) -> String {
    let mut out = String::with_capacity(id.len());
    let mut upper = true;
    for ch in id.chars() {
        if ch == '-' || ch == '_' {
            out.push(' ');
            upper = true;
        } else if upper {
            out.extend(ch.to_uppercase());
            upper = false;
        } else {
            out.push(ch);
        }
    }
    out
}

// Acknowledge unused-import warning when build.rs is reused for non-build purposes.
#[allow(dead_code)]
fn _path_helper(_p: &Path) {}
