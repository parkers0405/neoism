//! POD mirror of the host agent icon module.
//!
//! Owns the value-identity bits an agent pane needs everywhere (which
//! agent is in this tab? what's its display name? what's its image id
//! and which synthetic panel does its overlay live on?). Shared asset
//! registration keeps provider logos identical in native and web UI;
//! `/proc` foreground-process detection stays in the desktop fork.

use base64::Engine;
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
const PROVIDER_LOGO_IMAGE_PREFIX: u32 = 0xB000_0000;

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

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ProviderLogo {
    OpenRouter,
    Anthropic,
    OpenAi,
    Google,
    GitHubCopilot,
    Vercel,
    OpenCode,
}

impl ProviderLogo {
    const COUNT: f32 = 7.0;
    pub const SOURCE_SIDE: usize = 64;

    pub fn from_provider_id(id: &str) -> Option<Self> {
        match id.trim().to_ascii_lowercase().as_str() {
            "openrouter" => Some(Self::OpenRouter),
            "anthropic" | "claude" | "claude-code" => Some(Self::Anthropic),
            "openai" | "chatgpt" | "codex" => Some(Self::OpenAi),
            "google" | "google-vertex" | "vertex-ai" => Some(Self::Google),
            "github-copilot" | "copilot" => Some(Self::GitHubCopilot),
            "vercel" | "vercel-ai-gateway" => Some(Self::Vercel),
            "opencode" | "open-code" => Some(Self::OpenCode),
            _ => None,
        }
    }

    pub fn from_model(model: &str) -> Option<Self> {
        let (provider, _) = model.trim().split_once('/')?;
        Self::from_provider_id(provider)
    }

    pub fn source_rect(self) -> [f32; 4] {
        let index = self.index() as f32;
        [index / Self::COUNT, 0.0, (index + 1.0) / Self::COUNT, 1.0]
    }

    fn index(self) -> usize {
        match self {
            Self::OpenRouter => 0,
            Self::Anthropic => 1,
            Self::OpenAi => 2,
            Self::Google => 3,
            Self::GitHubCopilot => 4,
            Self::Vercel => 5,
            Self::OpenCode => 6,
        }
    }
}

pub fn register_provider_logo_atlas(
    sugarloaf: &mut Sugarloaf,
    color: [u8; 4],
) -> Option<u32> {
    let image_id = PROVIDER_LOGO_IMAGE_PREFIX
        | ((color[0] as u32) << 16)
        | ((color[1] as u32) << 8)
        | color[2] as u32;
    if sugarloaf.image_data.contains_key(&image_id) {
        return Some(image_id);
    }

    let encoded: String = PROVIDER_LOGO_ATLAS_BASE64.split_whitespace().collect();
    let bytes = base64::engine::general_purpose::STANDARD
        .decode(encoded)
        .ok()?;
    let mut image = image_rs::load_from_memory(&bytes).ok()?.to_rgba8();
    let expected_width = ProviderLogo::SOURCE_SIDE * ProviderLogo::COUNT as usize;
    if image.width() as usize != expected_width
        || image.height() as usize != ProviderLogo::SOURCE_SIDE
    {
        return None;
    }
    for pixel in image.pixels_mut() {
        pixel[0] = color[0];
        pixel[1] = color[1];
        pixel[2] = color[2];
        pixel[3] = ((pixel[3] as u16 * color[3] as u16) / 255) as u8;
    }
    let (width, height) = image.dimensions();
    sugarloaf.image_data.insert(
        image_id,
        GraphicDataEntry::from_graphic_data(GraphicData {
            id: GraphicId::new(image_id as u64),
            width: width as usize,
            height: height as usize,
            color_type: ColorType::Rgba,
            pixels: image.into_raw(),
            is_opaque: false,
            resize: None,
            display_width: None,
            display_height: None,
            transmit_time: Instant::now(),
        }),
    );
    Some(image_id)
}

// Bridge `AgentKind` into the shared `AgentLabel` trait so generic
// `BufferTabs<AgentKind>` can read tab titles without depending on the
// desktop fork.
impl crate::panels::buffer_tabs::AgentLabel for AgentKind {
    fn display_name(&self) -> &str {
        AgentKind::display_name(*self)
    }
}

// Keep the existing tab artwork as the single provider-logo source. These
// files remain in the desktop asset bundle for packaging compatibility, but
// registration lives here so the shared composer and web host use them too.
const CLAUDE_PNG: &[u8] = include_bytes!("../../../../desktop/assets/icons/claude.png");
const CODEX_PNG: &[u8] = include_bytes!("../../../../desktop/assets/icons/codex.png");
const OPENCODE_PNG: &[u8] =
    include_bytes!("../../../../desktop/assets/icons/opencode.png");
const NEOISM_PNG: &[u8] = include_bytes!("../../../assets/icons/neoism.png");
const PROVIDER_LOGO_ATLAS_BASE64: &str =
    include_str!("../../../assets/icons/provider-logos.png.b64");

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
    register_icon(sugarloaf, NEOISM_IMAGE_ID, NEOISM_PNG)
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

/// Side-panel overlays are immediate-mode on every host. Sugarloaf retains
/// the vectors between frames, so failing to clear here grows one image per
/// visible conversation on every repaint.
pub fn clear_side_panel_icon_overlays(sugarloaf: &mut Sugarloaf) {
    sugarloaf.clear_image_overlays_for(SIDE_PANEL_ICON_PANEL_ID);
}

pub fn push_icon_overlay_to_panel(
    sugarloaf: &mut Sugarloaf,
    kind: AgentKind,
    panel_id: usize,
    x: f32,
    y: f32,
    size: f32,
) {
    let scale = sugarloaf.scale_factor();
    sugarloaf.push_image_overlay(
        panel_id,
        GraphicOverlay {
            image_id: kind.image_id(),
            x: x * scale,
            y: y * scale,
            width: size * scale,
            height: size * scale,
            z_index: 1,
            source_rect: [0.0, 0.0, 1.0, 1.0],
        },
    );
}

pub fn register_agent_icons(sugarloaf: &mut Sugarloaf) -> bool {
    [
        (CLAUDE_IMAGE_ID, CLAUDE_PNG),
        (CODEX_IMAGE_ID, CODEX_PNG),
        (OPENCODE_IMAGE_ID, OPENCODE_PNG),
        (NEOISM_IMAGE_ID, NEOISM_PNG),
    ]
    .into_iter()
    .all(|(id, bytes)| register_icon(sugarloaf, id, bytes))
}

fn register_icon(sugarloaf: &mut Sugarloaf, id: u32, bytes: &[u8]) -> bool {
    if sugarloaf.image_data.contains_key(&id) {
        return true;
    }
    let Ok(image) = image_rs::load_from_memory(bytes) else {
        return false;
    };
    let image = image.to_rgba8();
    let (width, height) = image.dimensions();
    let entry = GraphicDataEntry::from_graphic_data(GraphicData {
        id: GraphicId::new(id as u64),
        width: width as usize,
        height: height as usize,
        color_type: ColorType::Rgba,
        pixels: image.into_raw(),
        is_opaque: false,
        resize: None,
        display_width: None,
        display_height: None,
        transmit_time: Instant::now(),
    });
    sugarloaf.image_data.insert(id, entry);
    true
}

#[cfg(test)]
mod tests {
    use super::{ProviderLogo, PROVIDER_LOGO_ATLAS_BASE64};
    use base64::Engine;

    #[test]
    fn resolves_known_model_providers_without_guessing_routed_models() {
        assert_eq!(
            ProviderLogo::from_model("anthropic/claude-sonnet-4"),
            Some(ProviderLogo::Anthropic)
        );
        assert_eq!(
            ProviderLogo::from_model("openai/gpt-5"),
            Some(ProviderLogo::OpenAi)
        );
        assert_eq!(
            ProviderLogo::from_model("openai/gpt-6-astra"),
            Some(ProviderLogo::OpenAi)
        );
        assert_eq!(
            ProviderLogo::from_model("opencode/big-pickle"),
            Some(ProviderLogo::OpenCode)
        );
        assert_eq!(
            ProviderLogo::from_model("openrouter/openai/gpt-5"),
            Some(ProviderLogo::OpenRouter)
        );
        assert_eq!(ProviderLogo::from_model("server default"), None);
    }

    #[test]
    fn provider_logo_atlas_decodes_to_expected_strip() {
        let encoded: String = PROVIDER_LOGO_ATLAS_BASE64.split_whitespace().collect();
        let bytes = base64::engine::general_purpose::STANDARD
            .decode(encoded)
            .expect("provider atlas base64");
        let image = image_rs::load_from_memory(&bytes).expect("provider atlas PNG");
        assert_eq!((image.width(), image.height()), (448, 64));
        let image = image.to_rgba8();
        for provider in [
            ProviderLogo::OpenRouter,
            ProviderLogo::Anthropic,
            ProviderLogo::OpenAi,
            ProviderLogo::Google,
            ProviderLogo::GitHubCopilot,
            ProviderLogo::Vercel,
            ProviderLogo::OpenCode,
        ] {
            let start_x = provider.index() * ProviderLogo::SOURCE_SIDE;
            assert!((0..ProviderLogo::SOURCE_SIDE).any(|y| {
                (0..ProviderLogo::SOURCE_SIDE)
                    .any(|x| image.get_pixel((start_x + x) as u32, y as u32)[3] != 0)
            }));
        }
    }
}
