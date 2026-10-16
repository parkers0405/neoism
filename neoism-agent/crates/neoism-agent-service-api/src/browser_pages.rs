//! Self-contained browser handoff pages, matching the neoism-web hero.
use base64::Engine;
use std::sync::OnceLock;

pub const FAVICON: &str = include_str!("browser_pages/favicon.svg");
pub const WORDMARK: &str = include_str!("browser_pages/wordmark.svg");
pub const WORDMARK_SCRIPT: &str = include_str!("browser_pages/pixel-wordmark.js");

pub fn style() -> &'static str {
    static STYLE: OnceLock<String> = OnceLock::new();
    STYLE.get_or_init(|| {
        let font = base64::engine::general_purpose::STANDARD
            .encode(include_bytes!("browser_pages/geist.woff2"));
        format!(
            "/* {} */\n@font-face{{font-family:Geist;font-style:normal;font-weight:100 900;font-display:swap;src:url(data:font/woff2;base64,{font}) format('woff2')}}\n{}\n{}",
            include_str!("browser_pages/geist-LICENSE.txt"),
            include_str!("browser_pages/style.css"),
            include_str!("browser_pages/pixel-wordmark.css"),
        )
    })
}

pub fn escape_html(value: &str) -> String {
    value.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&#39;")
}

/// Content and script are trusted templates; escape interpolated values first.
pub fn render(title: &str, content: &str, script: &str) -> String {
    let script = if script.is_empty() {
        String::new()
    } else {
        format!("<script>{script}</script>")
    };
    format!(
        r#"<!doctype html><html lang="en"><head><meta charset="utf-8"><meta name="viewport" content="width=device-width,initial-scale=1"><meta name="color-scheme" content="light dark"><meta name="referrer" content="no-referrer"><title>{title}</title><link rel="icon" type="image/svg+xml" href="data:image/svg+xml;base64,{favicon}"><style>{style}</style></head><body><main><h1 id="hero-title" class="hero-wordmark" aria-label="Neoism"><span class="sr-only">Neoism</span><button id="wordmark-trigger" class="wordmark-trigger" type="button" aria-label="Replay Neoism pixel animation">{WORDMARK}</button></h1>{content}</main><script>{WORDMARK_SCRIPT}</script>{script}</body></html>"#,
        title = escape_html(title),
        favicon = base64::engine::general_purpose::STANDARD.encode(FAVICON.as_bytes()),
        style = style(),
    )
}

/// Callback success confirms credentials, not a live MCP runtime connection.
pub fn mcp_connected() -> String {
    render(
        "MCP authorized | Neoism",
        r#"<section class="content" aria-labelledby="page-title"><h2 id="page-title">MCP authorization complete</h2><p>Authorization succeeded. Return to Neoism to check the server's connection status.</p><p class="note completion">You can close this tab.</p></section>"#,
        "",
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn callback_uses_the_shared_website_wordmark_and_self_contained_font() {
        let html = mcp_connected();
        assert!(html.contains(WORDMARK));
        assert_eq!(WORDMARK.matches("<path ").count(), 6);
        assert_eq!(WORDMARK.matches("<use ").count(), 18);
        assert!(WORDMARK.contains("viewBox=\"-24 566 1550 338\""));
        assert!(style().contains("data:font/woff2;base64,"));
        assert!(style().contains("prefers-color-scheme:dark"));
        assert!(style().contains("--page:#dedbd2"));
        assert!(style().contains("SIL OPEN FONT LICENSE"));
        assert_eq!(html.matches("<script>").count(), 1);
        assert!(html.contains(WORDMARK_SCRIPT));
        assert!(html.contains("id=\"wordmark-trigger\""));
        let icon = html.split_once("href=\"data:image/svg+xml;base64,").unwrap().1
            .split_once('"').unwrap().0;
        assert_eq!(base64::engine::general_purpose::STANDARD.decode(icon).unwrap(), FAVICON.as_bytes());
        assert_eq!(html.matches("<link ").count(), 1);
    }

    #[test]
    fn callback_reports_authorization_not_an_unverified_runtime_connection() {
        let html = mcp_connected();
        assert!(html.contains("MCP authorization complete"));
        assert!(html.contains("connection status"));
        assert!(!html.contains("MCP server connected"));
    }

    #[test]
    fn title_and_dynamic_text_are_escaped() {
        assert_eq!(escape_html("<&\"'>"), "&lt;&amp;&quot;&#39;&gt;");
        let html = render("<script>alert('x')</script>", "", "");
        assert!(html.contains("<title>&lt;script&gt;alert(&#39;x&#39;)&lt;/script&gt;</title>"));
        assert_eq!(html.matches("<script>").count(), 1);
        assert!(html.contains(WORDMARK_SCRIPT));
        assert!(html.contains("id=\"wordmark-trigger\""));
    }
}
