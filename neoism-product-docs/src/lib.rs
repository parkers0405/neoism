/// One immutable documentation page bundled with Neoism.
#[derive(Debug, Clone, Copy)]
pub struct BundledDoc {
    pub path: &'static str,
    pub title: &'static str,
    pub body: &'static str,
}

// The same machine-readable bundle is published with the website and local
// handbook. It is compiled here so agent docs work offline, even without Notes.
include!(concat!(env!("OUT_DIR"), "/docs.rs"));

pub fn bundled_doc(path: &str) -> Option<&'static BundledDoc> {
    let normalized = path.trim().trim_start_matches('/');
    BUNDLED_DOCS
        .iter()
        .find(|doc| doc.path.eq_ignore_ascii_case(normalized))
}

pub fn title(doc: &BundledDoc) -> &str {
    doc.title
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tool_composition_is_bundled_and_linked() {
        let page = bundled_doc("Neoism Agent/Tool Composition.md").unwrap();
        assert_eq!(title(page), "Tool Composition");
        for required in [
            "Windows, macOS, and Linux",
            "compose",
            "Rust interpreter",
            "Every child call",
            "not automatically repeated or rolled back",
            "Unsupported syntax is rejected before tool effects",
            "String.indexOf",
            "Array.push",
            "String(error)",
            "these aliases do not import duplicate payloads",
        ] {
            assert!(
                page.body.contains(required),
                "missing composition guidance: {required}"
            );
        }
        for path in [
            "Neoism Agent/The Neoism Agent.md",
            "Neoism Agent/Tools and Background Tasks.md",
        ] {
            assert!(bundled_doc(path)
                .unwrap()
                .body
                .contains("[[Tool Composition]]"));
        }
    }

    #[test]
    fn typesafe_mode_setup_and_privacy_are_bundled() {
        let page = bundled_doc("Neoism Agent/TypeSafe Browser Mode.md").unwrap();
        for required in [
            "off-by-default",
            "TYPESAFE_API_KEY",
            "computer.browser_goal",
            "computer.browser_step",
            "step/time budget",
            "partial_unknown",
            "DOM",
            "server credential store",
            "external provider",
            "preview",
            "false",
        ] {
            assert!(
                page.body.contains(required),
                "missing TypeSafe guidance: {required}"
            );
        }
        assert!(bundled_doc("Neoism Agent/Computer Use.md")
            .unwrap()
            .body
            .contains("[[TypeSafe Browser Mode]]"));
    }

    #[test]
    fn computer_use_attachment_guide_is_bundled_and_linked() {
        let page = bundled_doc("Neoism Agent/Computer Use.md")
            .expect("computer MCP setup must be available through Docs");
        assert_eq!(title(page), "Computer Use");
        for required in [
            "computer.capabilities",
            "Firefox",
            "NEOISM_BROWSER_BIDI_URL",
            "Chromium",
            "NEOISM_BROWSER_CDP_URL",
            "computer.browser_disconnect",
            "computer.browser_attach",
            "no Neoism restart",
            "agent server",
        ] {
            assert!(
                page.body.contains(required),
                "missing attachment guidance: {required}"
            );
        }
        assert!(bundled_doc("Neoism Agent/MCP Servers.md")
            .unwrap()
            .body
            .contains("[[Computer Use]]"));
    }

}
