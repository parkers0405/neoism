//! Host-owned theme scripts only. Never accepts arbitrary code or raw CSS fragments.
use crate::{ArtifactDocument, ArtifactStyles, Error, Theme};

fn inline_json(value: &impl serde::Serialize) -> Result<String, Error> {
    serde_json::to_string(value)
        .map(|json| {
            json.replace('<', "\\u003c")
                .replace('\u{2028}', "\\u2028")
                .replace('\u{2029}', "\\u2029")
        })
        .map_err(|error| Error::Backend(format!("Theme serialization: {error}")))
}

pub(crate) fn update_script(
    styles: &ArtifactStyles,
    theme: Theme,
    notify: bool,
) -> Result<String, Error> {
    let variables = styles
        .css_variables()
        .map_err(|error| Error::Backend(format!("Invalid artifact styles: {error}")))?;
    let variables = inline_json(&variables)?;
    let scheme = match theme {
        Theme::Light => "light",
        Theme::Dark => "dark",
    };
    // Values are JSON data, not interpolated CSS/JS. Only fixed default rules are CSS text.
    // Inline properties are updated without replacing nodes, navigating or resetting JS state.
    Ok(format!(
        r#"(()=>{{
const root=document.documentElement;if(!root)return false;
const variables={variables};
for(const [name,value] of Object.entries(variables))root.style.setProperty(name,value);
root.style.colorScheme="{scheme}";
let defaults=document.getElementById("neoism-artifact-default-theme");
if(!defaults){{defaults=document.createElement("style");defaults.id="neoism-artifact-default-theme";
defaults.textContent="body{{background:var(--background);color:var(--foreground);font-family:var(--font-sans)}}code,pre,kbd,samp{{font-family:var(--font-mono)}}";
(document.head||root).appendChild(defaults);}}
{notification}
// Force style/layout work before the evaluation completion callback permits CPU readback.
root.getBoundingClientRect();
return true;
}})();"#,
        notification = if notify {
            "window.dispatchEvent(new CustomEvent(\"neoism-theme-changed\"));"
        } else {
            ""
        }
    ))
}

pub(crate) fn live_script(
    styles: &ArtifactStyles,
    theme: Theme,
    url: &str,
) -> Result<String, Error> {
    let script = update_script(styles, theme, true)?;
    let url = inline_json(&url)?;
    Ok(format!(
        "(()=>{{if(document.location.origin!=={url})return false;return {script}}})();"
    ))
}

pub(crate) fn inline_html(document: &ArtifactDocument) -> Result<Vec<u8>, Error> {
    let script = update_script(&document.styles, document.theme, false)?;
    // A leading standards doctype and parser-blocking script run before author HTML/code.
    // Prefixing a full HTML document is handled by the HTML parser's normal html/head merge.
    Ok(format!("<!doctype html><script>{script}\ndocument.addEventListener(\"DOMContentLoaded\",()=>window.dispatchEvent(new CustomEvent(\"neoism-theme-changed\")),{{once:true}});</script>{}", document.html).into_bytes())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn script_closing_data_never_closes_bootstrap() -> Result<(), Error> {
        let json = inline_json(&std::collections::BTreeMap::from([(
            "--font-sans",
            "</script><script>bad()</script>",
        )]))?;
        assert!(!json.contains('<'));
        assert!(json.contains("\\u003c/script>"));
        Ok(())
    }
    #[test]
    fn styles_are_mutated_without_source_revision_or_navigation() -> Result<(), Error> {
        let mut styles = ArtifactStyles::default();
        let initial = update_script(&styles, Theme::Dark, false)?;
        styles.font_sans = "Neoism Selected Font".into();
        let updated = update_script(&styles, Theme::Light, true)?;
        assert_ne!(initial, updated);
        assert!(updated.contains("root.style.setProperty"));
        assert!(updated.contains("neoism-theme-changed"));
        assert!(!updated.contains("location"));
        assert!(!updated.contains("document.write"));
        assert!(!updated.contains("innerHTML"));
        Ok(())
    }
    #[test]
    fn initial_html_contains_only_one_host_script_terminator() -> Result<(), Error> {
        let mut styles = ArtifactStyles::default();
        styles.font_sans = "</script><script>bad()</script>".into();
        let document = ArtifactDocument {
            key: "test".into(),
            html: "<p>body</p>".into(),
            revision: 7,
            viewport: crate::Viewport {
                width: 320,
                height: 320,
                scale: 1.0,
            },
            visible: true,
            theme: Theme::Dark,
            styles,
        };
        if let Ok(bytes) = inline_html(&document) {
            let html = String::from_utf8(bytes)
                .map_err(|error| Error::Backend(error.to_string()))?;
            assert_eq!(html.matches("</script>").count(), 1);
            assert!(!html.contains("<script>bad()"));
        } // Rejecting a forbidden family value is also a valid outcome.
        Ok(())
    }
}
