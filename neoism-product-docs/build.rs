use serde_json::Value;
use sha2::{Digest, Sha256};
use std::{
    collections::HashSet,
    env, fs,
    path::{Component, Path, PathBuf},
};

fn source(root: &Path, relative: &str) -> PathBuf {
    assert!(
        !relative.is_empty()
            && Path::new(relative)
                .components()
                .all(|part| matches!(part, Component::Normal(_))),
        "unsafe documentation source path: {relative}"
    );
    let path = root.join(relative);
    println!("cargo:rerun-if-changed={}", path.display());
    path
}

fn main() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../vendor/neoism-docs");
    let manifest_path = source(&root, "manifest.json");
    let manifest: Value = serde_json::from_slice(
        &fs::read(&manifest_path)
            .expect("Shared docs are missing. Run git submodule update --init --recursive."),
    )
    .expect("invalid shared documentation manifest");
    let bundle_path = source(&root, "bundle/content.json");
    let bytes = fs::read(bundle_path)
        .expect("Shared docs content bundle is missing; regenerate it in vendor/neoism-docs.");
    let bundle: Value = serde_json::from_slice(&bytes).expect("invalid shared documentation bundle");
    assert_eq!(
        manifest["schemaVersion"], 1,
        "unsupported documentation manifest"
    );
    assert_eq!(bundle["schemaVersion"], 1, "unsupported documentation bundle");
    let version = manifest["version"].as_str().expect("docs version is required");
    assert_eq!(bundle["version"], version, "documentation bundle version is stale");
    let pages = bundle["pages"].as_array().expect("documentation pages are required");
    let mut paths = HashSet::new();
    let mut generated = format!("pub const DOCS_VERSION: &str = {version:?};\npub const DOCS_SHA256: &str = {:?};\npub static BUNDLED_DOCS: &[BundledDoc] = &[\n", format!("{:x}", Sha256::digest(&bytes)));
    for page in pages {
        let path = page["path"]
            .as_str()
            .expect("documentation page path is required");
        let title = page["title"]
            .as_str()
            .expect("documentation page title is required");
        let body = page["content"]
            .as_str()
            .expect("documentation page content is required");
        assert!(
            paths.insert(path.to_lowercase()),
            "duplicate documentation path: {path}"
        );
        generated.push_str(&format!("BundledDoc {{ path: {path:?}, title: {title:?}, body: {body:?} }},\n"));
    }
    generated.push_str("];\n");
    for page in manifest["pages"].as_array().expect("manifest pages are required") {
        let path = page["path"].as_str().expect("manifest page path is required");
        let file = source(
            &root,
            page["file"]
                .as_str()
                .expect("manifest source file is required"),
        );
        let body = fs::read_to_string(file).expect("missing documentation source");
        let packed = pages
            .iter()
            .find(|page| page["path"] == path)
            .expect("manifest page missing from documentation bundle");
        assert_eq!(
            packed["content"].as_str(),
            Some(body.as_str()),
            "stale documentation bundle for {path}; regenerate bundle/content.json"
        );
        assert_eq!(
            packed["title"], page["title"],
            "stale documentation title for {path}"
        );
    }
    let api_path = source(
        &root,
        manifest["openapi"]
            .as_str()
            .expect("OpenAPI source is required"),
    );
    let api_hash = format!(
        "{:x}",
        Sha256::digest(fs::read(api_path).expect("missing documentation API contract"))
    );
    assert_eq!(
        bundle["openapiSha256"].as_str(),
        Some(api_hash.as_str()),
        "stale documentation API reference; regenerate bundle/content.json"
    );
    let application_api =
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../neoism-agent/openapi/v2.json");
    println!("cargo:rerun-if-changed={}", application_api.display());
    assert_eq!(api_hash, format!("{:x}", Sha256::digest(fs::read(application_api).expect("missing application API contract"))), "Shared documentation describes a different API contract. Export the application's OpenAPI into neoism-docs and update the pinned revision.");
    fs::write(
        Path::new(&env::var("OUT_DIR").unwrap()).join("docs.rs"),
        generated,
    )
    .unwrap();
}