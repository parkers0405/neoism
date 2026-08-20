//! POD mirror of the host agent icon module.
//!
//! Owns the value-identity bits an agent pane needs everywhere (which
//! agent is in this tab? what's its display name? what's its image id
//! and which synthetic panel does its overlay live on?). Asset bytes,
//! image registration, and `/proc` foreground-process detection stay
//! in the desktop fork (`frontends/neoism/src/neoism/icon.rs`) — that
//! file re-exports the items defined here so callers don't have to
//! care which side of the split owns what.

use sugarloaf::{
    ColorType, GraphicData, GraphicDataEntry, GraphicId, GraphicOverlay, Sugarloaf,
};
use web_time::Instant;

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

/// The Neoism mark, owned by the SHARED crate so a host without its own
/// `AgentIconProvider` can still paint a real logo. `image_rs` is a
/// non-gated dependency here (the splash wordmark already decodes a PNG
/// this way on wasm), so this works in the browser build too.
const NEOISM_PNG: &[u8] = include_bytes!("../../../assets/icons/neoism.png");

/// Decode + upload the Neoism mark to sugarloaf's image store. Returns
/// `true` once the image is available. Idempotent — safe to call every
/// frame; later calls return immediately.
///
/// The desktop fork registers all four agent PNGs through its own
/// `register_agent_icons`. Only the Neoism mark lives here, because a
/// host whose `BufferTabs<A>` carries no agent identity (web runs
/// `Chrome<()>`) can't distinguish Claude/Codex/OpenCode tabs anyway —
/// but it CAN tell a Neoism agent tab from `neoism_agent_route_id`.
pub fn register_neoism_icon(sugarloaf: &mut Sugarloaf) -> bool {
    if sugarloaf.image_data.contains_key(&NEOISM_IMAGE_ID) {
        return true;
    }
    let Ok(img) = image_rs::load_from_memory(NEOISM_PNG) else {
        return false;
    };
    let img = img.to_rgba8();
    let (width, height) = img.dimensions();
    let entry = GraphicDataEntry::from_graphic_data(GraphicData {
        id: GraphicId::new(NEOISM_IMAGE_ID as u64),
        width: width as usize,
        height: height as usize,
        color_type: ColorType::Rgba,
        pixels: img.into_raw(),
        is_opaque: false,
        resize: None,
        display_width: None,
        display_height: None,
        transmit_time: Instant::now(),
    });
    sugarloaf.image_data.insert(NEOISM_IMAGE_ID, entry);
    true
}

/// Paint the registered Neoism mark into the tab strip's icon slot.
/// Mirrors the desktop `push_cropped_icon_overlay` call the shared
/// buffer-tabs render makes through an `AgentIconProvider`.
pub fn draw_neoism_tab_icon(
    sugarloaf: &mut Sugarloaf,
    x: f32,
    y: f32,
    width: f32,
    height: f32,
    source_rect: [f32; 4],
) {
    let scale = sugarloaf.scale_factor();
    sugarloaf.push_image_overlay(
        ICON_PANEL_ID,
        GraphicOverlay {
            image_id: NEOISM_IMAGE_ID,
            x: x * scale,
            y: y * scale,
            width: width * scale,
            height: height * scale,
            z_index: 1,
            source_rect,
        },
    );
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
