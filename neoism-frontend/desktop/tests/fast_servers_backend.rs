// Compile and exercise the transport backend independently of desktop App
// wiring. No GPU/window bootstrap, saved registry, or live relay is needed.
#[allow(dead_code)]
#[path = "../src/fast_servers.rs"]
mod fast_servers;
#[allow(dead_code)]
#[path = "../src/invite_links.rs"]
mod invite_links;
#[path = "../src/standalone_executable.rs"]
mod service_process;

#[tokio::test]
#[ignore = "opt-in render-only browser previews; never starts a tunnel or OAuth flow"]
async fn render_browser_page_previews() {
    let directory = std::path::PathBuf::from(
        std::env::var_os("NEOISM_BROWSER_PREVIEW_DIR").expect("set NEOISM_BROWSER_PREVIEW_DIR"),
    );
    std::fs::create_dir_all(&directory).unwrap();
    let response = fast_servers::invite_page::render(
        "https://preview.invalid/?name=Design%20workspace",
        None,
    );
    assert_eq!(response.status(), 200);
    let csp = response.headers()["content-security-policy"].to_str().unwrap().to_owned();
    let bytes = axum::body::to_bytes(response.into_body(), 256 * 1024).await.unwrap();
    let html = String::from_utf8(bytes.to_vec()).unwrap();
    // Keep the real rendered page, but do not launch Neoism just by previewing it.
    let handoff = "try { window.location.assign(openLink.href); } catch (_) {}";
    assert!(html.contains(handoff));
    use base64::Engine;
    use sha2::{Digest, Sha256};
    let hash = |source: &str| base64::engine::general_purpose::STANDARD
        .encode(Sha256::digest(source.as_bytes()));
    let script = html.rsplit_once("<script>").unwrap().1.split_once("</script>").unwrap().0;
    let csp = csp.replace(
        &format!("'sha256-{}'", hash(script)),
        &format!("'sha256-{}'", hash(&script.replace(handoff, ""))),
    );
    std::fs::write(directory.join("join-csp.txt"), csp).unwrap();
    std::fs::write(directory.join("join.html"), html.replace(handoff, "")).unwrap();
    std::fs::write(
        directory.join("mcp.html"),
        neoism_agent_service_api::browser_pages::mcp_connected(),
    ).unwrap();
    for (name, html) in [
        ("provider-received.html", neoism_agent_service_api::browser_pages::provider_authorization_received()),
        ("provider-failed.html", neoism_agent_service_api::browser_pages::provider_authorization_failed("The provider declined authorization. Return to Neoism to try again.")),
        ("provider-cancelled.html", neoism_agent_service_api::browser_pages::provider_authorization_cancelled()),
    ] {
        std::fs::write(directory.join(name), html).unwrap();
    }
}

#[tokio::test]
async fn landing_is_secretless_escaped_and_hash_csp_matches_exact_inline_sources() {
    use base64::Engine;
    use sha2::{Digest, Sha256};
    let mut invite = url::Url::parse("https://sdk.opentunnel.xyz/").unwrap();
    invite
        .query_pairs_mut()
        .append_pair("name", "\"<script>alert('x')</script>&雪");
    let response =
        fast_servers::invite_page::render(invite.as_str(), Some("name=untrusted"));
    assert_eq!(response.status(), 200);
    let headers = response.headers().clone();
    assert_eq!(headers["content-type"], "text/html; charset=utf-8");
    assert_eq!(headers["x-content-type-options"], "nosniff");
    assert_eq!(headers["referrer-policy"], "no-referrer");
    assert_eq!(headers["x-frame-options"], "DENY");
    assert_eq!(headers["cache-control"], "no-store");
    let bytes = axum::body::to_bytes(response.into_body(), 256 * 1024)
        .await
        .unwrap();
    let html = String::from_utf8(bytes.to_vec()).unwrap();
    assert!(html.contains("&quot;&lt;script&gt;alert(&#39;x&#39;)&lt;/script&gt;&amp;雪"));
    assert!(!html.contains("untrusted"));
    assert!(!html.contains("<script>alert"));
    assert!(html.contains("neoism://join?invite=https%3A%2F%2Fsdk.opentunnel.xyz"));
    let native = html
        .split_once("id=\"open\" href=\"")
        .unwrap()
        .1
        .split_once('"')
        .unwrap()
        .0;
    let join = invite_links::parse_invite_uri(native).unwrap();
    assert_eq!(join.name, "\"<script>alert('x')</script>&雪");
    assert!(join.token.is_none());
    assert!(html.contains("<link rel=\"icon\" type=\"image/svg+xml\" href=\"data:image/svg+xml;base64,"));
    assert!(headers["content-security-policy"].to_str().unwrap().contains("img-src data:"));
    assert!(html.contains(neoism_agent_service_api::browser_pages::WORDMARK));
    assert!(html.contains("data:font/woff2;base64,"));
    assert!(html.contains("prefers-color-scheme:dark"));
    assert!(headers["content-security-policy"].to_str().unwrap().contains("font-src data:"));
    assert!(html.contains("https://github.com/parkers0405/neoism/releases/latest"));
    assert!(!html.contains("location.search"));
    assert!(!html.contains("location.hash"));
    assert_eq!(html.matches("window.location.assign(").count(), 1);
    assert_eq!(html.matches("<script>").count(), 2);
    for tag in ["script", "style"] {
        let csp = headers["content-security-policy"].to_str().unwrap();
        let directive = csp.split(';')
            .map(str::trim)
            .find(|directive| directive.starts_with(&format!("{tag}-src ")))
            .unwrap();
        for block in html.split(&format!("<{tag}>")).skip(1) {
            let source = block.split_once(&format!("</{tag}>")).unwrap().0;
            let hash = base64::engine::general_purpose::STANDARD
                .encode(Sha256::digest(source.as_bytes()));
            assert!(directive.contains(&format!("'sha256-{hash}'")));
        }
    }
    assert!(headers["content-security-policy"]
        .to_str()
        .unwrap()
        .contains("frame-ancestors 'none'"));
    for query in [
        "token=private",
        "password=private",
        "name=ok&arbitrary=private",
    ] {
        let response = fast_servers::invite_page::render(invite.as_str(), Some(query));
        assert_eq!(response.status(), 400);
        let bytes = axum::body::to_bytes(response.into_body(), 256 * 1024)
            .await
            .unwrap();
        assert!(!String::from_utf8_lossy(&bytes).contains("private"));
    }
}
