//! Public, inert GUI assets. API requests always pass through the original router.
use std::path::{Path, PathBuf};

use anyhow::Context;
use axum::{
    body::Body,
    extract::State,
    http::{header, Method, Request, StatusCode},
    middleware::{self, Next},
    response::{IntoResponse, Response},
    Router,
};

#[derive(Clone)]
pub struct GuiRoot(PathBuf);

impl GuiRoot {
    pub fn discover() -> anyhow::Result<Self> {
        if let Some(root) = std::env::var_os("NEOISM_AGENT_GUI_ROOT") {
            return Self::validate(PathBuf::from(root));
        }
        let mut candidates = Vec::new();
        if let Ok(exe) = std::env::current_exe() {
            if let Some(bin) = exe.parent() {
                candidates.push(bin.join("agent-gui"));
                candidates.push(bin.join("share/neoism-agent/agent-gui"));
                candidates.push(bin.join("../share/neoism-agent/agent-gui"));
                candidates.push(bin.join("../share/agent-gui"));
            }
        }
        candidates.push(
            Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("../../sdk/typescript/packages/gui/dist"),
        );
        for path in &candidates {
            if let Ok(root) = Self::validate(path.clone()) {
                return Ok(root);
            }
        }
        anyhow::bail!("GUI dist not found. Build neoism-agent/sdk/typescript/packages/gui, or set NEOISM_AGENT_GUI_ROOT to its dist directory (containing index.html). Searched: {}", candidates.iter().map(|p| p.display().to_string()).collect::<Vec<_>>().join(", "))
    }

    fn validate(path: PathBuf) -> anyhow::Result<Self> {
        let root = path
            .canonicalize()
            .with_context(|| format!("GUI root not found: {}", path.display()))?;
        let index = root.join("index.html").canonicalize().with_context(|| {
            format!("GUI root must contain index.html: {}", root.display())
        })?;
        anyhow::ensure!(
            root.is_dir() && index.is_file() && index.starts_with(&root),
            "GUI index.html must be a file inside the GUI root"
        );
        Ok(Self(root))
    }
}

pub(crate) fn with_gui(api: Router, root: GuiRoot) -> Router {
    api.layer(middleware::from_fn_with_state(root, serve_gui))
}

// Decode before routing and filesystem access, rejecting ambiguous platform paths.
fn decoded_path(path: &str) -> Option<String> {
    let mut bytes = Vec::new();
    let mut input = path.bytes();
    while let Some(byte) = input.next() {
        bytes.push(if byte == b'%' {
            let hi = (input.next()? as char).to_digit(16)?;
            let lo = (input.next()? as char).to_digit(16)?;
            (hi * 16 + lo) as u8
        } else {
            byte
        });
    }
    let path = String::from_utf8(bytes).ok()?;
    if !path.starts_with('/')
        || path.contains(['\\', ':', '\0', '%'])
        || path
            .split('/')
            .any(|part| part == ".." || part.starts_with('.'))
    {
        return None;
    }
    Some(path)
}

fn is_api(path: &str) -> bool {
    path == "/v2" || path.starts_with("/v2/")
}

fn content_type(path: &Path) -> Option<&'static str> {
    Some(match path.extension()?.to_str()? {
        "html" => "text/html; charset=utf-8",
        "js" | "mjs" => "text/javascript; charset=utf-8",
        "css" => "text/css; charset=utf-8",
        "json" | "map" => "application/json",
        "svg" => "image/svg+xml",
        "png" => "image/png",
        "jpg" | "jpeg" => "image/jpeg",
        "webp" => "image/webp",
        "gif" => "image/gif",
        "ico" => "image/x-icon",
        "woff" => "font/woff",
        "woff2" => "font/woff2",
        "ttf" => "font/ttf",
        "otf" => "font/otf",
        "wasm" => "application/wasm",
        "webmanifest" => "application/manifest+json",
        "txt" => "text/plain; charset=utf-8",
        _ => return None,
    })
}

async fn serve_gui(
    State(root): State<GuiRoot>,
    request: Request<Body>,
    next: Next,
) -> Response {
    // Preserve even malformed/unknown API paths and their existing auth/fallback.
    if is_api(request.uri().path()) {
        return next.run(request).await;
    }
    let Some(path) = decoded_path(request.uri().path()) else {
        return StatusCode::NOT_FOUND.into_response();
    };
    if is_api(&path) {
        return next.run(request).await;
    }
    if request.method() != Method::GET && request.method() != Method::HEAD {
        return StatusCode::METHOD_NOT_ALLOWED.into_response();
    }
    let relative = path.trim_start_matches('/');
    let requested = root.0.join(relative);
    let asset_namespace = matches!(relative.split('/').next(), Some("assets" | "fonts"));
    let file = if relative.is_empty()
        || (!asset_namespace && requested.extension().is_none() && !requested.is_file())
    {
        root.0.join("index.html")
    } else {
        requested
    };
    let Some(mime) = content_type(&file) else {
        return StatusCode::NOT_FOUND.into_response();
    };
    let Ok(file) = tokio::fs::canonicalize(file).await else {
        return StatusCode::NOT_FOUND.into_response();
    };
    if !file.starts_with(&root.0) || !file.is_file() {
        return StatusCode::NOT_FOUND.into_response();
    }
    let Ok(bytes) = tokio::fs::read(file).await else {
        return StatusCode::NOT_FOUND.into_response();
    };
    let len = bytes.len();
    let mut response = if request.method() == Method::HEAD {
        Body::empty()
    } else {
        Body::from(bytes)
    }
    .into_response();
    response
        .headers_mut()
        .insert(header::CONTENT_TYPE, mime.parse().unwrap());
    response
        .headers_mut()
        .insert(header::CONTENT_LENGTH, len.into());
    response
        .headers_mut()
        .insert(header::CACHE_CONTROL, "no-cache".parse().unwrap());
    response
        .headers_mut()
        .insert("x-content-type-options", "nosniff".parse().unwrap());
    response
        .headers_mut()
        .insert("x-neoism-agent-gui", "1".parse().unwrap());
    response
}

#[cfg(test)]
mod tests {
    use super::*;
    use tower::ServiceExt;

    #[test]
    fn paths_are_unambiguous() {
        for path in [
            "/../secret",
            "/%2e%2e/secret",
            "/.env",
            "/a\\b",
            "/C:/secret",
            "/%252e%252e/x",
            "/%00",
            "/%ff",
            "/%",
        ] {
            assert!(decoded_path(path).is_none(), "{path}");
        }
        assert_eq!(
            decoded_path("/assets/app.js").as_deref(),
            Some("/assets/app.js")
        );
        assert!(is_api(&decoded_path("/%76%32/unknown").unwrap()));
    }

    #[tokio::test]
    async fn gui_is_public_but_never_masks_api_or_missing_assets() {
        let dir = std::env::temp_dir().join(format!(
            "neoism-gui-test-{}-{}",
            std::process::id(),
            rand::random::<u64>()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        assert!(GuiRoot::validate(dir.clone()).is_err());
        std::fs::write(dir.join("index.html"), "<!doctype html>GUI").unwrap();
        let api = Router::new().fallback(|| async { StatusCode::UNAUTHORIZED });
        let app = with_gui(api, GuiRoot::validate(dir.clone()).unwrap());
        for (path, expected) in [
            ("/", 200),
            ("/sessions/abc", 200),
            ("/missing.js", 404),
            ("/assets/missing", 404),
            ("/assets/", 404),
            ("/fonts/missing", 404),
            ("/fonts/", 404),
            ("/v2/unknown", 401),
            ("/v2", 401),
            ("/%76%32/unknown", 401),
            ("/%2e%2e/secret", 404),
        ] {
            let response = app
                .clone()
                .oneshot(Request::builder().uri(path).body(Body::empty()).unwrap())
                .await
                .unwrap();
            assert_eq!(response.status().as_u16(), expected, "{path}");
            if expected == 200 {
                assert_eq!(response.headers()["x-neoism-agent-gui"], "1");
            }
        }
        let head = app
            .clone()
            .oneshot(
                Request::builder()
                    .method(Method::HEAD)
                    .uri("/")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(head.status(), StatusCode::OK);
        assert_eq!(head.headers()[header::CONTENT_LENGTH], "18");
        assert!(axum::body::to_bytes(head.into_body(), 1024)
            .await
            .unwrap()
            .is_empty());
        let post = app
            .clone()
            .oneshot(
                Request::builder()
                    .method(Method::POST)
                    .uri("/")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(post.status(), StatusCode::METHOD_NOT_ALLOWED);
        #[cfg(unix)]
        {
            std::os::unix::fs::symlink("/etc/passwd", dir.join("escape.txt")).unwrap();
            let response = app
                .oneshot(
                    Request::builder()
                        .uri("/escape.txt")
                        .body(Body::empty())
                        .unwrap(),
                )
                .await
                .unwrap();
            assert_eq!(response.status(), StatusCode::NOT_FOUND);
            std::fs::remove_file(dir.join("index.html")).unwrap();
            std::os::unix::fs::symlink("/etc/passwd", dir.join("index.html")).unwrap();
            assert!(GuiRoot::validate(dir.clone()).is_err());
        }
        std::fs::remove_dir_all(dir).unwrap();
    }
}
