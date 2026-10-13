// Compile and exercise the transport backend independently of desktop App
// wiring. No GPU/window bootstrap, saved registry, or live relay is needed.
#[allow(dead_code)]
#[path = "../src/fast_servers.rs"]
mod fast_servers;
#[allow(dead_code)]
#[path = "../src/invite_links.rs"]
mod invite_links;

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
    let bytes = axum::body::to_bytes(response.into_body(), 65536)
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
    assert!(html.contains("https://github.com/parkers0405/neoism/releases/latest"));
    assert!(!html.contains("location.search"));
    assert!(!html.contains("location.hash"));
    assert_eq!(html.matches("window.location.assign(").count(), 1);
    for tag in ["script", "style"] {
        let source = html
            .split_once(&format!("<{tag}>"))
            .unwrap()
            .1
            .split_once(&format!("</{tag}>"))
            .unwrap()
            .0;
        let hash = base64::engine::general_purpose::STANDARD
            .encode(Sha256::digest(source.as_bytes()));
        assert!(headers["content-security-policy"]
            .to_str()
            .unwrap()
            .contains(&format!("{tag}-src 'sha256-{hash}'")));
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
        let bytes = axum::body::to_bytes(response.into_body(), 65536)
            .await
            .unwrap();
        assert!(!String::from_utf8_lossy(&bytes).contains("private"));
    }
}
