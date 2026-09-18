/// One immutable documentation page bundled with Neoism.
#[derive(Debug, Clone, Copy)]
pub struct BundledDoc {
    pub path: &'static str,
    pub body: &'static str,
}

macro_rules! docs {
    ($($path:literal),+ $(,)?) => { &[$(BundledDoc {
        path: $path,
        body: include_str!(concat!("../../neoism-workspace-index/src/welcome/", $path)),
    }),+] };
}

/// Canonical product documentation. Editable Welcome pages are seeded from
/// this bundle, but deleting those copies does not remove this resource.
pub static BUNDLED_DOCS: &[BundledDoc] = docs![
    "Start Here.md",
    "Getting Started/01 Meet Neoism.md",
    "Getting Started/02 Open Your First Workspace.md",
    "Getting Started/03 Terminal, Editor, and Notes.md",
    "Getting Started/04 Start Your First Agent.md",
    "Getting Started/05 Connect Another Device.md",
    "Getting Started/06 Configure Neoism.md",
    "Getting Started/07 Essential Keybindings.md",
    "Neoism/Workspaces.md",
    "Neoism/Terminal.md",
    "Neoism/Editor.md",
    "Neoism/Notes and Drawings.md",
    "Neoism/Navigation and Keybindings.md",
    "Neoism/Appearance.md",
    "Neoism Agent/The Neoism Agent.md",
    "Neoism Agent/Configure.md",
    "Neoism Agent/Providers.md",
    "Neoism Agent/Models.md",
    "Neoism Agent/Agents and Subagents.md",
    "Neoism Agent/Permissions.md",
    "Neoism Agent/Sessions and Sharing.md",
    "Neoism Agent/Undo and Redo.md",
    "Neoism Agent/Commands.md",
    "Neoism Agent/Skills.md",
    "Neoism Agent/Scheduled Workflows.md",
    "Neoism Agent/Instructions.md",
    "Neoism Agent/MCP Servers.md",
    "Neoism Agent/Computer Use.md",
    "Neoism Agent/TypeSafe Browser Mode.md",
    "Neoism Agent/Attachments.md",
    "Neoism Agent/Compaction.md",
    "Neoism Agent/Tools and Background Tasks.md",
    "Neoism Agent/Memory.md",
    "Neoism Agent/Formatters, LSP, and References.md",
    "Neoism Agent/Troubleshooting.md",
    "Neoism Agent/Server and API.md",
    "Neoism Agent/Plugins.md",
    "Neoism Agent/SDK.md",
    "Neoism Daemon/The Neoism Daemon.md",
    "Neoism Daemon/Sessions and Persistence.md",
    "Neoism Daemon/Remote Devices and Pairing.md",
    "Neoism Daemon/Multiplayer and Sync.md",
    "Neoism Daemon/Troubleshooting.md",
];

pub fn bundled_doc(path: &str) -> Option<&'static BundledDoc> {
    let normalized = path.trim().trim_start_matches('/');
    BUNDLED_DOCS
        .iter()
        .find(|doc| doc.path.eq_ignore_ascii_case(normalized))
}

pub fn title(doc: &BundledDoc) -> &str {
    doc.body
        .lines()
        .find_map(|line| line.strip_prefix("# ").map(str::trim))
        .unwrap_or_else(|| doc.path.trim_end_matches(".md"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn typesafe_mode_setup_and_privacy_are_bundled() {
        let page = bundled_doc("Neoism Agent/TypeSafe Browser Mode.md").unwrap();
        for required in ["off-by-default", "TYPESAFE_API_KEY", "computer.browser_goal", "computer.browser_step", "step/time budget", "partial_unknown", "DOM", "server credential store", "external provider", "preview", "false"] {
            assert!(page.body.contains(required), "missing TypeSafe guidance: {required}");
        }
        assert!(bundled_doc("Neoism Agent/Computer Use.md").unwrap().body.contains("[[TypeSafe Browser Mode]]"));
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
