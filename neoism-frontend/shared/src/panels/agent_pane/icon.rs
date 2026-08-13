//! POD mirror of the host agent icon module.
//!
//! Owns the value-identity bits an agent pane needs everywhere (which
//! agent is in this tab? what's its display name? what's its image id
//! and which synthetic panel does its overlay live on?). Asset bytes,
//! image registration, and `/proc` foreground-process detection stay
//! in the desktop fork (`frontends/neoism/src/neoism/icon.rs`) — that
//! file re-exports the items defined here so callers don't have to
//! care which side of the split owns what.

use sugarloaf::Sugarloaf;

/// Synthetic panel id for chrome image overlays. Matches the desktop
/// constant so cross-references through `Sugarloaf` keep the same
/// numeric ids. Image overlays whose panel id is absent from
/// `state.content.states` default to visible.
pub const ICON_PANEL_ID: usize = usize::MAX - 7;
pub const SIDE_PANEL_ICON_PANEL_ID: usize = usize::MAX - 8;

/// Reserved high-range image ids — kitty graphics ids come from the
/// PTY stream and realistically never reach the 0xA0DE prefix, so we
/// won't collide.
pub const CLAUDE_IMAGE_ID: u32 = 0xA0DE_0001;
pub const CODEX_IMAGE_ID: u32 = 0xA0DE_0002;
pub const OPENCODE_IMAGE_ID: u32 = 0xA0DE_0003;
pub const NEOISM_IMAGE_ID: u32 = 0xA0DE_0004;

/// POD agent identity. Mirrors the desktop enum variant-for-variant so
/// the view code can switch on `AgentKind` without dragging in PTY /
/// install / detection machinery.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AgentKind {
    Claude,
    Codex,
    OpenCode,
    Neoism,
}

impl AgentKind {
    pub fn image_id(self) -> u32 {
        match self {
            AgentKind::Claude => CLAUDE_IMAGE_ID,
            AgentKind::Codex => CODEX_IMAGE_ID,
            AgentKind::OpenCode => OPENCODE_IMAGE_ID,
            AgentKind::Neoism => NEOISM_IMAGE_ID,
        }
    }

    /// Stable lowercase id used for palette/modal tags and for
    /// round-tripping through `IdeToolInstallFinished`. Matches the
    /// binary name on disk in every case.
    pub fn id(self) -> &'static str {
        match self {
            AgentKind::Claude => "claude",
            AgentKind::Codex => "codex",
            AgentKind::OpenCode => "opencode",
            AgentKind::Neoism => "neoism",
        }
    }

    pub fn from_id(id: &str) -> Option<Self> {
        match id {
            "claude" => Some(AgentKind::Claude),
            "codex" => Some(AgentKind::Codex),
            "opencode" => Some(AgentKind::OpenCode),
            "neoism" | "neoism-agent" => Some(AgentKind::Neoism),
            _ => None,
        }
    }

    pub fn from_label(label: &str) -> Option<Self> {
        let lower = label.trim().to_ascii_lowercase();
        let normalized = lower
            .replace('_', "-")
            .replace(' ', "-")
            .replace("open-code", "opencode");
        if normalized.contains("claude") {
            Some(AgentKind::Claude)
        } else if normalized.contains("opencode") {
            Some(AgentKind::OpenCode)
        } else if normalized.contains("codex") {
            Some(AgentKind::Codex)
        } else if normalized.contains("neoism") {
            Some(AgentKind::Neoism)
        } else {
            None
        }
    }

    pub fn binary(self) -> &'static str {
        // Same as `id()` today, kept separate so install paths that
        // ship a launcher under a different name can override here
        // without breaking modal/palette wiring.
        self.id()
    }

    pub fn display_name(self) -> &'static str {
        match self {
            AgentKind::Claude => "Claude Code",
            AgentKind::Codex => "Codex",
            AgentKind::OpenCode => "OpenCode",
            AgentKind::Neoism => "Neoism",
        }
    }
}

// Bridge `AgentKind` into the shared `AgentLabel` trait so generic
// `BufferTabs<AgentKind>` can read tab titles without depending on the
// desktop fork.
impl crate::panels::buffer_tabs::AgentLabel for AgentKind {
    fn display_name(&self) -> &str {
        AgentKind::display_name(*self)
    }
}

// ── Stubs ──────────────────────────────────────────────────────────
//
// The desktop fork owns icon registration / overlay machinery (asset
// bytes + image_rs decode + `Sugarloaf::push_image_overlay`). The web
// build has no equivalent and the shared agent pane view calls into
// these from the same call sites the desktop does. Keep them as
// no-ops so the shared view compiles standalone; native callers reach
// the real impls through the desktop `crate::neoism::icon::*` path
// (those functions have the same names but live on the desktop side
// of the tree, parallel to these stubs).

/// Stub: the desktop fork owns the actual clear.
pub fn clear_side_panel_icon_overlays(_sugarloaf: &mut Sugarloaf) {}

/// Stub: the desktop fork owns the actual overlay push.
pub fn push_icon_overlay_to_panel(
    _sugarloaf: &mut Sugarloaf,
    _kind: AgentKind,
    _panel_id: usize,
    _x: f32,
    _y: f32,
    _size: f32,
) {
}

/// Stub: the desktop fork registers the actual icon images on startup.
/// Returning `true` here lets the shared view's "icons ready" gate stay
/// open; when the icon provider trait lands this becomes a host-supplied
/// readiness check.
pub fn register_agent_icons(_sugarloaf: &mut Sugarloaf) -> bool {
    true
}
