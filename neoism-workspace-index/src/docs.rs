/// One immutable documentation page bundled with Neoism.
#[derive(Debug, Clone, Copy)]
pub struct BundledDoc {
    pub path: &'static str,
    pub body: &'static str,
}

/// Canonical product documentation. The editable `Welcome/` vault folder is
/// seeded from this bundle, but deleting that copy never deletes these docs.
pub const BUNDLED_DOCS: &[BundledDoc] = &[
    BundledDoc {
        path: "Start Here.md",
        body: include_str!("welcome/Start Here.md"),
    },
    BundledDoc {
        path: "Getting Started/01 Meet Neoism.md",
        body: include_str!("welcome/Getting Started/01 Meet Neoism.md"),
    },
    BundledDoc {
        path: "Getting Started/02 Open Your First Workspace.md",
        body: include_str!("welcome/Getting Started/02 Open Your First Workspace.md"),
    },
    BundledDoc {
        path: "Getting Started/03 Terminal, Editor, and Notes.md",
        body: include_str!("welcome/Getting Started/03 Terminal, Editor, and Notes.md"),
    },
    BundledDoc {
        path: "Getting Started/04 Start Your First Agent.md",
        body: include_str!("welcome/Getting Started/04 Start Your First Agent.md"),
    },
    BundledDoc {
        path: "Getting Started/05 Connect Another Device.md",
        body: include_str!("welcome/Getting Started/05 Connect Another Device.md"),
    },
    BundledDoc {
        path: "Getting Started/06 Configure Neoism.md",
        body: include_str!("welcome/Getting Started/06 Configure Neoism.md"),
    },
    BundledDoc {
        path: "Getting Started/07 Essential Keybindings.md",
        body: include_str!("welcome/Getting Started/07 Essential Keybindings.md"),
    },
    BundledDoc {
        path: "Neoism/Workspaces.md",
        body: include_str!("welcome/Neoism/Workspaces.md"),
    },
    BundledDoc {
        path: "Neoism/Terminal.md",
        body: include_str!("welcome/Neoism/Terminal.md"),
    },
    BundledDoc {
        path: "Neoism/Editor.md",
        body: include_str!("welcome/Neoism/Editor.md"),
    },
    BundledDoc {
        path: "Neoism/Notes and Drawings.md",
        body: include_str!("welcome/Neoism/Notes and Drawings.md"),
    },
    BundledDoc {
        path: "Neoism/Navigation and Keybindings.md",
        body: include_str!("welcome/Neoism/Navigation and Keybindings.md"),
    },
    BundledDoc {
        path: "Neoism/Appearance.md",
        body: include_str!("welcome/Neoism/Appearance.md"),
    },
    BundledDoc {
        path: "Neoism Agent/The Neoism Agent.md",
        body: include_str!("welcome/Neoism Agent/The Neoism Agent.md"),
    },
    BundledDoc {
        path: "Neoism Agent/Configure.md",
        body: include_str!("welcome/Neoism Agent/Configure.md"),
    },
    BundledDoc {
        path: "Neoism Agent/Providers.md",
        body: include_str!("welcome/Neoism Agent/Providers.md"),
    },
    BundledDoc {
        path: "Neoism Agent/Models.md",
        body: include_str!("welcome/Neoism Agent/Models.md"),
    },
    BundledDoc {
        path: "Neoism Agent/Agents and Subagents.md",
        body: include_str!("welcome/Neoism Agent/Agents and Subagents.md"),
    },
    BundledDoc {
        path: "Neoism Agent/Permissions.md",
        body: include_str!("welcome/Neoism Agent/Permissions.md"),
    },
    BundledDoc {
        path: "Neoism Agent/Sessions and Sharing.md",
        body: include_str!("welcome/Neoism Agent/Sessions and Sharing.md"),
    },
    BundledDoc {
        path: "Neoism Agent/Undo and Redo.md",
        body: include_str!("welcome/Neoism Agent/Undo and Redo.md"),
    },
    BundledDoc {
        path: "Neoism Agent/Commands.md",
        body: include_str!("welcome/Neoism Agent/Commands.md"),
    },
    BundledDoc {
        path: "Neoism Agent/Skills.md",
        body: include_str!("welcome/Neoism Agent/Skills.md"),
    },
    BundledDoc {
        path: "Neoism Agent/Instructions.md",
        body: include_str!("welcome/Neoism Agent/Instructions.md"),
    },
    BundledDoc {
        path: "Neoism Agent/MCP Servers.md",
        body: include_str!("welcome/Neoism Agent/MCP Servers.md"),
    },
    BundledDoc {
        path: "Neoism Agent/Attachments.md",
        body: include_str!("welcome/Neoism Agent/Attachments.md"),
    },
    BundledDoc {
        path: "Neoism Agent/Compaction.md",
        body: include_str!("welcome/Neoism Agent/Compaction.md"),
    },
    BundledDoc {
        path: "Neoism Agent/Tools and Background Tasks.md",
        body: include_str!("welcome/Neoism Agent/Tools and Background Tasks.md"),
    },
    BundledDoc {
        path: "Neoism Agent/Memory.md",
        body: include_str!("welcome/Neoism Agent/Memory.md"),
    },
    BundledDoc {
        path: "Neoism Agent/Formatters, LSP, and References.md",
        body: include_str!("welcome/Neoism Agent/Formatters, LSP, and References.md"),
    },
    BundledDoc {
        path: "Neoism Agent/Troubleshooting.md",
        body: include_str!("welcome/Neoism Agent/Troubleshooting.md"),
    },
    BundledDoc {
        path: "Neoism Daemon/The Neoism Daemon.md",
        body: include_str!("welcome/Neoism Daemon/The Neoism Daemon.md"),
    },
    BundledDoc {
        path: "Neoism Daemon/Sessions and Persistence.md",
        body: include_str!("welcome/Neoism Daemon/Sessions and Persistence.md"),
    },
    BundledDoc {
        path: "Neoism Daemon/Remote Devices and Pairing.md",
        body: include_str!("welcome/Neoism Daemon/Remote Devices and Pairing.md"),
    },
    BundledDoc {
        path: "Neoism Daemon/Multiplayer and Sync.md",
        body: include_str!("welcome/Neoism Daemon/Multiplayer and Sync.md"),
    },
    BundledDoc {
        path: "Neoism Daemon/Troubleshooting.md",
        body: include_str!("welcome/Neoism Daemon/Troubleshooting.md"),
    },
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
