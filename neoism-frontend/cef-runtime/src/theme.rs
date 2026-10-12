//! Host-owned theme bootstrap; values are JSON, never executable interpolation.
use neoism_interactive_artifacts::{ArtifactDocument, Theme};

pub fn theme_script(d: &ArtifactDocument) -> Result<String, String> {
    let vars = d.styles.css_variables().map_err(|e| e.to_string())?;
    let json = serde_json::to_string(&vars)
        .map_err(|e| e.to_string())?
        .replace('<', "\\u003c")
        .replace('\u{2028}', "\\u2028")
        .replace('\u{2029}', "\\u2029");
    let scheme = if d.theme == Theme::Dark {
        "dark"
    } else {
        "light"
    };
    Ok(format!(
        r#"(()=>{{const r=document.documentElement;for(const [k,v] of Object.entries({json}))r.style.setProperty(k,v);r.style.colorScheme='{scheme}';let s=document.getElementById('neoism-artifact-theme');if(!s){{s=document.createElement('style');s.id='neoism-artifact-theme';s.textContent='body{{margin:0;background:transparent;color:var(--foreground);font-family:var(--font-sans)}}pre,code,kbd,samp{{font-family:var(--font-mono)}}';(document.head||r).prepend(s)}}window.dispatchEvent(new CustomEvent('neoism-theme-changed'));}})();"#
    ))
}

// Deliberate exact-host CDN policy, not "https:". No connect-src, frames, workers,
// forms, object embeds, arbitrary remote scripts, local files or private-network URLs.
pub const CSP: &str = "default-src 'none'; script-src 'unsafe-inline' https://cdn.jsdelivr.net https://cdnjs.cloudflare.com; style-src 'unsafe-inline' https://cdn.jsdelivr.net https://cdnjs.cloudflare.com https://fonts.googleapis.com; font-src data: https://fonts.gstatic.com https://cdn.jsdelivr.net; img-src data: https://images.unsplash.com https://cdn.jsdelivr.net; connect-src 'none'; frame-src 'none'; worker-src 'none'; object-src 'none'; base-uri 'none'; form-action 'none'";

pub fn document_url(d: &ArtifactDocument) -> Result<String, String> {
    let html = format!(
        "<!doctype html><meta http-equiv=\"Content-Security-Policy\" content=\"{CSP}\"><script>{}</script>{}",
        theme_script(d)?,
        d.html
    );
    let mut url = String::from("data:text/html;charset=utf-8,");
    for byte in html.bytes() {
        use std::fmt::Write;
        write!(url, "%{byte:02X}").unwrap();
    }
    Ok(url)
}

pub fn allowed_resource(url: &str) -> bool {
    if url.starts_with("data:image/") || url.starts_with("data:font/") {
        return true;
    }
    [
        "cdn.jsdelivr.net",
        "cdnjs.cloudflare.com",
        "fonts.googleapis.com",
        "fonts.gstatic.com",
        "images.unsplash.com",
    ]
    .iter()
    .any(|host| url.starts_with(&format!("https://{host}/")))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn bootstrap_precedes_author_html_and_theme_is_data() {
        let d = ArtifactDocument {
            key: "test".into(),
            html: "<p>author</p>".into(),
            revision: 1,
            viewport: neoism_interactive_artifacts::Viewport {
                width: 320,
                height: 240,
                scale: 1.0,
            },
            visible: true,
            theme: Theme::Dark,
            styles: Default::default(),
        };
        let script = theme_script(&d).unwrap();
        assert!(script.contains("setProperty"));
        assert!(script.contains("neoism-theme-changed"));
        assert!(!script.contains("innerHTML"));
        assert!(!script.contains("document.write"));
        let url = document_url(&d).unwrap();
        assert!(url.starts_with("data:text/html;charset=utf-8,"));
        assert!(!url.contains('<'));
        assert!(
            url.find("%43%6F%6E%74%65%6E%74%2D%53%65%63%75%72%69%74%79")
                .unwrap()
                < url.find("%61%75%74%68%6F%72").unwrap()
        );
    }
    #[test]
    fn exact_hosts_only() {
        for url in [
            "file:///etc/passwd",
            "https://127.0.0.1/x",
            "http://cdn.jsdelivr.net/x",
            "https://cdn.jsdelivr.net.evil/x",
            "https://cdn.jsdelivr.net@127.0.0.1/x",
            "https://cdn.jsdelivr.net:444/x",
        ] {
            assert!(!allowed_resource(url));
        }
        assert!(allowed_resource("https://cdn.jsdelivr.net/npm/example"));
        assert!(allowed_resource("data:image/svg+xml,test"));
    }
}
