//! Desktop-owned HTML artifact worker adapter. No Servo/window dependency crosses
//! the host API; requests and frame snapshots are reconciled on the UI thread.
use super::super::Screen;
use neoism_backend::event::{RioEvent, RioEventType};
use neoism_interactive_artifacts::{
    ArtifactColors, ArtifactDocument, ArtifactFrame, ArtifactStyles, Theme, Viewport,
};
use neoism_ui::primitives::ide_theme::IdeTheme;
use neoism_window::event::ElementState;
use neoism_window::keyboard::{Key, KeyCode, NamedKey, PhysicalKey};
use std::collections::{HashMap, HashSet};

// Each Screen owns a worker connection. The callback only schedules a UI pump.
#[cfg(feature = "servo-artifacts")]
pub(crate) struct DesktopHtmlArtifacts {
    host: neoism_interactive_artifacts::servo_host::Host,
    slots: std::collections::HashMap<String, DesktopHtmlSlot>,
    next_image: u32,
    stream: u64,
    focused: Option<String>,
    captured: Option<String>,
    hovered: Option<String>,
    buttons: Vec<neoism_interactive_artifacts::PointerButton>,
    keys: std::collections::HashMap<String, neoism_interactive_artifacts::KeyboardEvent>,
    composing: bool,
    clock: u64,
    next_revision: u64,
    error: Option<String>,
    diagnostics: HashSet<String>,
    wake_enabled: std::sync::Arc<std::sync::atomic::AtomicBool>,
}

#[cfg(feature = "servo-artifacts")]
struct DesktopHtmlSlot {
    route: usize,
    owner: String,
    visible: bool,
    last_used: u64,
    sequence: u64,
    awaiting_frame: bool,
    source: String,
    html: String,
    revision: u64,
    image: u32,
    viewport: [f32; 4],
    visible_rect: [f32; 4],
    scale: f32,
    css_scale: f32,
    theme: neoism_interactive_artifacts::Theme,
    styles: ArtifactStyles,
}

const MAX_VISIBLE: usize = 8;
const MAX_RETAINED: usize = 16;

pub(crate) struct ArtifactFailure {
    sources: HashMap<String, String>,
}

/// Map the resolved renderer palette, not a theme name or a canned web palette.
/// Shadcn-style input means control border; accent means the hover surface.
fn artifact_styles(
    palette: &IdeTheme,
    sans: &str,
    mono: &str,
    radius: f32,
) -> ArtifactStyles {
    ArtifactStyles {
        colors: ArtifactColors {
            background: palette.bg,
            foreground: palette.fg,
            card: palette.surface,
            card_foreground: palette.fg,
            popover: palette.surface,
            popover_foreground: palette.fg,
            muted: palette.muted,
            muted_foreground: palette.dim,
            border: palette.border,
            input: palette.border,
            primary: palette.accent,
            primary_foreground: contrast_foreground(palette.accent, palette),
            accent: palette.hover,
            accent_foreground: palette.fg,
            destructive: palette.red,
            destructive_foreground: contrast_foreground(palette.red, palette),
            success: palette.green,
            warning: palette.yellow,
            info: palette.blue,
            chart_1: palette.blue,
            chart_2: palette.cyan,
            chart_3: palette.green,
            chart_4: palette.yellow,
            chart_5: palette.magenta,
            chart_6: palette.red,
        },
        font_sans: sans.to_owned(),
        font_mono: mono.to_owned(),
        radius,
    }
}

fn relative_luminance(color: u32) -> f32 {
    let linear = |v: u32| {
        let s = (v & 255) as f32 / 255.0;
        if s <= 0.04045 {
            s / 12.92
        } else {
            ((s + 0.055) / 1.055).powf(2.4)
        }
    };
    0.2126 * linear(color >> 16) + 0.7152 * linear(color >> 8) + 0.0722 * linear(color)
}

/// Pick among this palette's actual foreground/background extremes. No injected
/// hardcoded white/black palette; light and custom themes use their own tokens.
fn contrast_foreground(background: u32, palette: &IdeTheme) -> u32 {
    let l = relative_luminance(background);
    [palette.fg, palette.bg, palette.white, palette.black]
        .into_iter()
        .max_by(|a, b| {
            let contrast = |color| {
                let c = relative_luminance(color);
                (l.max(c) + 0.05) / (l.min(c) + 0.05)
            };
            contrast(*a).total_cmp(&contrast(*b))
        })
        .unwrap_or(palette.fg)
}

fn owner_key(route: usize, session: &str) -> String {
    format!("{route}:{}:{session}", session.len())
}

/// Removed owners go immediately. Otherwise evict only hidden least-recently
/// used views; scrolling away never by itself destroys a document.
fn lifecycle_removals(
    slots: &HashMap<String, DesktopHtmlSlot>,
    owners: &HashSet<String>,
    visible: &HashSet<String>,
) -> Vec<String> {
    let mut removed: Vec<_> = slots
        .iter()
        .filter(|(_, slot)| !owners.contains(&slot.owner))
        .map(|(key, _)| key.clone())
        .collect();
    let retained = slots.len() - removed.len();
    let new_count = visible
        .iter()
        .filter(|key| !slots.contains_key(*key))
        .count();
    let eviction_count = (retained + new_count).saturating_sub(MAX_RETAINED);
    let mut inactive: Vec<_> = slots
        .iter()
        .filter(|(key, slot)| owners.contains(&slot.owner) && !visible.contains(*key))
        .map(|(key, slot)| (slot.last_used, key.clone()))
        .collect();
    inactive.sort_unstable();
    removed.extend(
        inactive
            .into_iter()
            .take(eviction_count)
            .map(|(_, key)| key),
    );
    removed
}

fn raster_viewport(rect: [f32; 4], os_scale: f32, ui_scale: f32) -> Option<Viewport> {
    if rect.iter().any(|v| !v.is_finite())
        || rect[2] <= 0.0
        || rect[3] <= 0.0
        || !os_scale.is_finite()
        || os_scale <= 0.0
    {
        return None;
    }
    let width = (rect[2] * os_scale).ceil();
    let height = (rect[3] * os_scale).ceil();
    if !(1.0..=4096.0).contains(&width) || !(1.0..=4096.0).contains(&height) {
        return None;
    }
    let viewport = Viewport {
        width: width as u32,
        height: height as u32,
        scale: os_scale * ui_scale,
    };
    viewport.validate().ok()?;
    Some(viewport)
}

fn local_point(point: [f32; 2], viewport: [f32; 4], os_scale: f32) -> [f32; 2] {
    [
        (point[0] - viewport[0]) * os_scale,
        (point[1] - viewport[1]) * os_scale,
    ]
}

/// Treat frame replies as untrusted/maybe stale worker data. Missing generation
/// (revision/sequence zero), wrong viewport, truncated rows and oversize buffers
/// never become a GPU upload. IPC request-generation matching is also the Host's
/// responsibility; this API exposes document revision and frame sequence only.
fn frame_pixels(frame: &ArtifactFrame, slot: &DesktopHtmlSlot) -> Option<Vec<u8>> {
    let expected =
        raster_viewport(slot.viewport, slot.scale, slot.css_scale / slot.scale)?;
    if frame.revision == 0
        || frame.revision != slot.revision
        || frame.sequence == 0
        || frame.sequence <= slot.sequence
        || frame.width != expected.width
        || frame.height != expected.height
    {
        return None;
    }
    let row_bytes = (frame.width as usize).checked_mul(4)?;
    let total = row_bytes.checked_mul(frame.height as usize)?;
    if frame.stride < row_bytes || total > neoism_interactive_artifacts::MAX_FRAME_BYTES {
        return None;
    }
    let required = frame
        .stride
        .checked_mul(frame.height.saturating_sub(1) as usize)?
        .checked_add(row_bytes)?;
    if frame.rgba.len() < required {
        return None;
    }
    let mut pixels = Vec::with_capacity(total);
    for y in 0..frame.height as usize {
        let start = y.checked_mul(frame.stride)?;
        let row = &frame.rgba[start..start + row_bytes];
        // Bulk-copy the row first: opaque pixels need no arithmetic. Convert
        // only partial-alpha pixels in-place, excluding IPC row padding.
        let offset = pixels.len();
        pixels.extend_from_slice(row);
        unpremultiply_rgba(&mut pixels[offset..]);
    }
    Some(pixels)
}

fn unpremultiply_rgba(pixels: &mut [u8]) {
    for px in pixels.chunks_exact_mut(4) {
        match px[3] {
            255 => {}
            0 => px[..3].fill(0),
            alpha => {
                let a = u32::from(alpha);
                for c in &mut px[..3] {
                    *c = ((u32::from(*c) * 255 + a / 2) / a).min(255) as u8;
                }
            }
        }
    }
}

// Only the first image publication needs another projection pass. Already
// projected overlays reference the same image ID and see fresh pixels now.
fn needs_snapshot_projection(visible: bool, previously_available: bool) -> bool {
    visible && !previously_available
}

static NEXT_STREAM: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);

impl DesktopHtmlSlot {
    /// Screen translation, clipping and reactivation do not change the engine
    /// framebuffer. Compare raster dimensions, not floating timeline geometry.
    fn update_geometry(
        &mut self,
        viewport: [f32; 4],
        visible_rect: [f32; 4],
        os_scale: f32,
        ui_scale: f32,
    ) -> bool {
        let previous =
            raster_viewport(self.viewport, self.scale, self.css_scale / self.scale);
        let next = raster_viewport(viewport, os_scale, ui_scale);
        self.viewport = viewport;
        self.visible_rect = visible_rect;
        self.scale = os_scale;
        self.css_scale = ui_scale * os_scale;
        let resized = previous != next;
        if resized {
            self.awaiting_frame = true;
        }
        resized
    }
    /// Theme-only updates deliberately leave revision, HTML, sequence and focus
    /// readiness untouched. The worker updates existing root CSS properties.
    fn update_styles(&mut self, theme: Theme, styles: &ArtifactStyles) -> bool {
        if self.theme == theme && self.styles == *styles {
            return false;
        }
        self.theme = theme;
        self.styles = styles.clone();
        true
    }

    fn document(&self, key: String) -> ArtifactDocument {
        ArtifactDocument {
            key,
            html: self.html.clone(),
            revision: self.revision,
            viewport: raster_viewport(
                self.viewport,
                self.scale,
                self.css_scale / self.scale,
            )
            .expect("validated artifact viewport"),
            visible: self.visible,
            theme: self.theme,
            styles: self.styles.clone(),
        }
    }
}

impl Screen<'_> {
    fn resolved_html_artifact_styles(&self) -> (Theme, ArtifactStyles) {
        let palette = self.renderer.styled_theme(neoism_lua::selector::AGENT_CHAT);
        let theme = if palette.is_dark() {
            Theme::Dark
        } else {
            Theme::Light
        };
        // Font 0 is Sugarloaf's live selected config primary/regular face (not
        // a list-of-system-fonts lookup). Release this read before any host call.
        let font_mono = self
            .sugarloaf
            .font_library()
            .inner
            .read()
            .inner
            .get(&0)
            .and_then(|font| font.family_name())
            .unwrap_or("monospace")
            .to_owned();
        let mut style = self.renderer.style(neoism_lua::selector::APP);
        style.overlay(Some(&self.renderer.style(neoism_lua::selector::AGENT_CHAT)));
        let font_sans = style
            .font_family
            .clone()
            .or_else(neoism_ui::primitives::look::markdown_font_family)
            .unwrap_or_else(|| font_mono.clone());
        let radius = style
            .radius
            .filter(|r| r.is_finite())
            .unwrap_or(8.0)
            .clamp(0.0, 64.0);
        (
            theme,
            artifact_styles(&palette, &font_sans, &font_mono, radius),
        )
    }

    /// Clear all queues, not just painted panes. Hidden grids and a frame with
    /// zero Agent panels must not replay a request from a previous draw pass.
    pub(crate) fn begin_html_artifact_frame(&mut self) {
        // Do not project old-palette snapshots into a new-theme draw pass while
        // waiting for asynchronous worker paint. Source/JS state stays intact.
        let stale_styles = self.html_artifacts.as_ref().is_some_and(|browser| {
            let (theme, styles) = self.resolved_html_artifact_styles();
            browser
                .slots
                .values()
                .any(|slot| slot.theme != theme || slot.styles != styles)
        });
        for grid in self.context_manager.all_grids_mut() {
            for item in grid.contexts_mut().values_mut() {
                if let Some(agent) = item.val.neoism_agent.as_mut() {
                    agent.html_artifact_requests.clear();
                    if stale_styles {
                        agent.html_artifact_frames.clear();
                    }
                }
            }
        }
    }

    /// Drain worker damage after agent projection. Returns only a one-shot
    /// projection/notification follow-up, never browser animation ownership:
    /// worker rAF is independently driven and new frame mail wakes the GUI.
    pub(crate) fn sync_html_artifacts(&mut self) -> bool {
        use neoism_backend::sugarloaf::{
            ColorType, GraphicData, GraphicDataEntry, GraphicId,
        };
        use neoism_ui::panels::agent_pane::view::markdown::HtmlArtifactFrame;
        use neoism_ui::panels::notifications::NotificationLevel;
        use std::sync::{
            atomic::{AtomicBool, Ordering},
            Arc,
        };
        let enabled = std::env::var("NEOISM_SERVO_ARTIFACTS").as_deref() == Ok("1");
        let window_scale = self.sugarloaf.scale_factor();
        let (theme, styles) = self.resolved_html_artifact_styles();
        let mut requests = Vec::new();
        let mut owners = HashSet::new();
        let mut visible = HashSet::new();
        for grid in self.context_manager.all_grids_mut() {
            for item in grid.contexts_mut().values_mut() {
                if let Some(agent) = item.val.neoism_agent.as_mut() {
                    let session = agent.session_id_str().unwrap_or("draft").to_owned();
                    let owner = owner_key(item.val.route_id, &session);
                    owners.insert(owner.clone());
                    for request in agent.html_artifact_requests.drain(..) {
                        let key = format!("{owner}:{}", request.key);
                        if enabled
                            && visible.len() < MAX_VISIBLE
                            && raster_viewport(
                                request.viewport,
                                window_scale,
                                request.scale,
                            )
                            .is_some()
                            && request.visible_rect.iter().all(|v| v.is_finite())
                            && request.visible_rect[2] > 0.0
                            && request.visible_rect[3] > 0.0
                            && visible.insert(key.clone())
                        {
                            requests.push((
                                key,
                                owner.clone(),
                                item.val.route_id,
                                request,
                            ));
                        }
                    }
                    agent.html_artifact_frames.clear();
                }
            }
        }
        if !enabled {
            if let Some(browser) = self.html_artifacts.take() {
                browser.wake_enabled.store(false, Ordering::Release);
                for slot in browser.slots.values() {
                    self.sugarloaf.image_data.remove(&slot.image);
                }
                // Drop closes this window's worker; a later opt-in starts a fresh one.
            }
            self.html_artifacts_failure = None;
            self.html_artifact_retry_count = 0;
            return false;
        }
        if let Some(failure) = &self.html_artifacts_failure {
            // Only an edit to a failed source retries, not viewport/scroll changes.
            let edited = requests.iter().any(|(key, _, _, request)| {
                failure
                    .sources
                    .get(key)
                    .is_some_and(|html| html != &request.html)
            });
            if edited && self.html_artifact_retry_count < 2 {
                self.html_artifact_retry_count += 1;
                self.html_artifacts_failure = None;
            } else {
                return false;
            }
        }
        if self.html_artifacts.is_none() && !requests.is_empty() {
            let proxy = self.context_manager.event_proxy();
            let window = self.context_manager.window_id();
            let route = self.context_manager.current().route_id;
            let wake_enabled = Arc::new(AtomicBool::new(true));
            let gate = wake_enabled.clone();
            let wake = Arc::new(move || {
                if gate.load(Ordering::Acquire) {
                    proxy.send_event(
                        RioEventType::Rio(RioEvent::RenderRoute(route)),
                        window,
                    );
                }
            });
            match neoism_interactive_artifacts::servo_host::Host::new(wake) {
                Ok(host) => {
                    self.html_artifacts = Some(DesktopHtmlArtifacts {
                        host,
                        slots: Default::default(),
                        next_image: 0xB8000000,
                        stream: NEXT_STREAM.fetch_add(1, Ordering::Relaxed),
                        focused: None,
                        captured: None,
                        hovered: None,
                        buttons: Vec::new(),
                        keys: Default::default(),
                        composing: false,
                        clock: 0,
                        next_revision: 0,
                        error: None,
                        diagnostics: HashSet::new(),
                        wake_enabled,
                    })
                }
                Err(error) => {
                    self.html_artifacts_failure = Some(ArtifactFailure {
                        sources: requests
                            .into_iter()
                            .map(|(key, _, _, r)| (key, r.html))
                            .collect(),
                    });
                    self.renderer.notifications.push(format!("HTML artifact worker unavailable: {error}. Edit the HTML to retry (up to twice), or reopen the window."), NotificationLevel::Error);
                    tracing::warn!(?error, "HTML artifact worker unavailable");
                    return true;
                }
            }
        }
        let keyboard_blocked = self.html_artifact_keyboard_blocked();
        let Some(browser) = self.html_artifacts.as_mut() else {
            return false;
        };
        browser.clock = browser.clock.saturating_add(1);
        // Keep lifetime wakes enabled even with no visible views: the mailbox
        // rejects hidden frames, but worker failures/diagnostics still need a UI
        // pump. Status and successful writes no longer create wake traffic.
        if browser
            .focused
            .as_ref()
            .is_some_and(|key| !visible.contains(key))
            || keyboard_blocked
        {
            browser.blur();
        }
        if browser
            .hovered
            .as_ref()
            .is_some_and(|key| !visible.contains(key))
        {
            browser.hovered = None;
        }
        for key in lifecycle_removals(&browser.slots, &owners, &visible) {
            if let Some(slot) = browser.slots.remove(&key) {
                browser.host.destroy(&key);
                self.sugarloaf.image_data.remove(&slot.image);
            }
        }
        for (key, slot) in browser.slots.iter_mut() {
            if visible.contains(key) {
                continue;
            }
            let hide = slot.visible;
            let restyle = slot.update_styles(theme, &styles);
            if restyle {
                self.sugarloaf.image_data.remove(&slot.image);
            }
            slot.visible = false;
            slot.visible_rect = [0.0; 4];
            if hide || restyle {
                if let Err(error) = browser.host.reconcile(slot.document(key.clone())) {
                    browser
                        .error
                        .get_or_insert_with(|| format!("Hide/restyle document: {error}"));
                }
            }
        }
        for (key, owner, route, request) in requests {
            let is_new = !browser.slots.contains_key(&key);
            if is_new {
                let image = browser.next_image;
                browser.next_image = 0xB8000000 | ((image + 1) & 0x00ffffff);
                browser.slots.insert(
                    key.clone(),
                    DesktopHtmlSlot {
                        route,
                        owner,
                        source: request.key.clone(),
                        html: String::new(),
                        revision: 0,
                        image,
                        viewport: request.viewport,
                        visible_rect: request.visible_rect,
                        scale: window_scale,
                        css_scale: request.scale * window_scale,
                        theme,
                        styles: styles.clone(),
                        visible: false,
                        last_used: browser.clock,
                        sequence: 0,
                        awaiting_frame: true,
                    },
                );
            }
            let slot = browser.slots.get_mut(&key).expect("inserted artifact");
            let source_changed = is_new || slot.html != request.html;
            let visibility_changed = !slot.visible;
            let resized = slot.update_geometry(
                request.viewport,
                request.visible_rect,
                window_scale,
                request.scale,
            );
            let style_changed = slot.update_styles(theme, &styles);
            if style_changed {
                self.sugarloaf.image_data.remove(&slot.image);
            }
            let reconcile_needed =
                source_changed || visibility_changed || resized || style_changed;
            if source_changed {
                slot.awaiting_frame = true;
            }
            if source_changed {
                browser.next_revision = browser
                    .next_revision
                    .checked_add(1)
                    .expect("artifact revision exhausted");
                slot.revision = browser.next_revision;
                slot.sequence = 0;
                slot.html = request.html;
                self.sugarloaf.image_data.remove(&slot.image);
            }
            slot.visible = true;
            slot.last_used = browser.clock;
            slot.theme = theme;
            if reconcile_needed {
                if let Err(error) = browser.host.reconcile(slot.document(key)) {
                    browser
                        .error
                        .get_or_insert_with(|| format!("Reconcile document: {error}"));
                }
            }
        }
        if browser.focused.as_ref().is_some_and(|key| {
            browser
                .slots
                .get(key)
                .is_none_or(|slot| slot.awaiting_frame)
        }) {
            browser.blur();
        }
        let mut changed = false;
        if browser.error.is_none() {
            match browser.host.pump() {
                Ok(output) => {
                    for diagnostic in output.diagnostics {
                        // Bound diagnostic memory and display each distinct warning once.
                        if browser.diagnostics.len() < 16
                            && browser.diagnostics.insert(diagnostic.clone())
                        {
                            changed = true;
                            self.renderer.notifications.push(
                                format!("HTML artifact: {diagnostic}"),
                                NotificationLevel::Warn,
                            );
                        }
                    }
                    for frame in output.frames {
                        let Some(slot) = browser.slots.get_mut(&frame.key) else {
                            continue;
                        };
                        let Some(pixels) = frame_pixels(&frame, slot) else {
                            continue;
                        };
                        slot.sequence = frame.sequence;
                        slot.awaiting_frame = false;
                        changed |= needs_snapshot_projection(
                            slot.visible,
                            self.sugarloaf.image_data.contains_key(&slot.image),
                        );
                        self.sugarloaf.image_data.insert(
                            slot.image,
                            GraphicDataEntry::from_stream_graphic_data(
                                GraphicData {
                                    id: GraphicId::new(slot.image as u64),
                                    width: frame.width as usize,
                                    height: frame.height as usize,
                                    color_type: ColorType::Rgba,
                                    pixels,
                                    is_opaque: false,
                                    resize: None,
                                    display_width: None,
                                    display_height: None,
                                    transmit_time: web_time::Instant::now(),
                                },
                                (browser.stream, slot.image as u64),
                                (slot.revision, frame.sequence),
                            ),
                        );
                    }
                }
                Err(error) => {
                    browser.error = Some(format!("Pump worker: {error}"));
                }
            }
        }
        if let Some(error) = browser.error.take() {
            browser.wake_enabled.store(false, Ordering::Release);
            let sources = browser
                .slots
                .iter()
                .map(|(key, slot)| (key.clone(), slot.html.clone()))
                .collect();
            for slot in browser.slots.values() {
                self.sugarloaf.image_data.remove(&slot.image);
            }
            self.html_artifacts_failure = Some(ArtifactFailure { sources });
            self.html_artifacts = None;
            self.renderer.notifications.push(format!("HTML artifact worker stopped: {error}. Edit the HTML to retry (up to twice), or reopen the window."), NotificationLevel::Error);
            tracing::warn!(%error, "HTML artifact worker stopped");
            return true;
        }
        for grid in self.context_manager.all_grids_mut() {
            for item in grid.contexts_mut().values_mut() {
                let Some(agent) = item.val.neoism_agent.as_mut() else {
                    continue;
                };
                let session = agent.session_id_str().unwrap_or("draft").to_owned();
                let owner = owner_key(item.val.route_id, &session);
                for slot in browser.slots.values().filter(|slot| slot.owner == owner) {
                    if let Some(image) = self.sugarloaf.image_data.get(&slot.image) {
                        let source_key =
                            format!("{}:{}:{}", session.len(), session, slot.source);
                        agent.html_artifact_frames.insert(
                            source_key,
                            HtmlArtifactFrame {
                                image_id: slot.image,
                                width: image.width as u32,
                                height: image.height as u32,
                            },
                        );
                    }
                }
            }
        }
        changed
    }
}

#[cfg(feature = "servo-artifacts")]
impl DesktopHtmlArtifacts {
    fn send(&mut self, key: &str, input: neoism_interactive_artifacts::ArtifactInput) {
        if self.error.is_some() {
            return;
        }
        if let Err(error) = self.host.input(key, input) {
            self.error = Some(format!("Input worker: {error}"));
        }
    }

    fn blur(&mut self) {
        use neoism_interactive_artifacts::{ArtifactInput, ButtonState, KeyState};
        if let Some(key) = self.focused.take() {
            for (_, mut event) in std::mem::take(&mut self.keys) {
                event.state = KeyState::Up;
                event.repeat = false;
                self.send(&key, ArtifactInput::Key(event));
            }
            for button in std::mem::take(&mut self.buttons) {
                self.send(
                    &key,
                    ArtifactInput::PointerButton {
                        x: 0.0,
                        y: 0.0,
                        button,
                        state: ButtonState::Up,
                    },
                );
            }
            self.send(&key, ArtifactInput::ImeDismissed);
            self.send(&key, ArtifactInput::Focus(false));
        }
        self.composing = false;
        self.captured = None;
    }
}

#[cfg(feature = "servo-artifacts")]
impl Screen<'_> {
    fn html_artifact_modal_blocked(&self) -> bool {
        std::env::var("NEOISM_SERVO_ARTIFACTS").as_deref() != Ok("1")
            || self.renderer.modal.is_active()
            || self.renderer.settings.is_active()
            || self.renderer.file_browser.is_active()
            || self.renderer.assistant.is_active()
            || self.renderer.command_palette.is_enabled()
            || self.renderer.context_menu.rect().is_some()
            || self
                .context_manager
                .current()
                .neoism_agent
                .as_ref()
                .is_some_and(|agent| {
                    agent.picker().is_some()
                        || agent.pending_permission().is_some()
                        || agent.pending_question().is_some()
                })
    }

    fn html_artifact_keyboard_blocked(&self) -> bool {
        self.html_artifact_modal_blocked()
            || self.renderer.file_tree.is_focused()
            || self.renderer.notes_sidebar.is_focused()
            || self.renderer.git_diff_panel.is_focused()
    }

    pub(crate) fn leave_html_artifact(&mut self) {
        if let Some(browser) = self.html_artifacts.as_mut() {
            if let Some(key) = browser.hovered.take() {
                browser.send(
                    &key,
                    neoism_interactive_artifacts::ArtifactInput::PointerLeave,
                );
            }
        }
    }

    pub(crate) fn blur_html_artifact(&mut self) {
        if let Some(browser) = self.html_artifacts.as_mut() {
            browser.blur();
        }
    }

    fn html_artifact_hit(&self) -> Option<String> {
        if self.html_artifact_modal_blocked() {
            return None;
        }
        let scale = self.sugarloaf.scale_factor();
        let x = self.mouse.x as f32 / scale;
        let y = self.mouse.y as f32 / scale;
        let size = self.sugarloaf.window_size();
        if self
            .renderer
            .active_text_occlusion_rects(size.width, size.height, scale)
            .iter()
            .any(|r| x >= r[0] && y >= r[1] && x < r[0] + r[2] && y < r[1] + r[3])
        {
            return None;
        }
        let browser = self.html_artifacts.as_ref()?;
        if browser.error.is_some() {
            return None;
        }
        browser.slots.iter().find_map(|(key, slot)| {
            if !slot.visible
                || slot.awaiting_frame
                || !self.sugarloaf.image_data.contains_key(&slot.image)
            {
                return None;
            }
            let grid = self.context_manager.current_grid();
            let current_owner = grid.contexts().iter().any(|(node, item)| {
                grid.is_context_visible(*node)
                    && item.val.route_id == slot.route
                    && item.val.neoism_agent.as_ref().is_some_and(|agent| {
                        owner_key(slot.route, agent.session_id_str().unwrap_or("draft"))
                            == slot.owner
                    })
            });
            if !current_owner {
                return None;
            }
            let r = slot.visible_rect;
            (x >= r[0] && y >= r[1] && x < r[0] + r[2] && y < r[1] + r[3])
                .then(|| key.clone())
        })
    }

    pub(crate) fn html_artifact_pointer(
        &mut self,
        button: Option<(neoism_window::event::MouseButton, ElementState)>,
    ) -> bool {
        use neoism_interactive_artifacts::{ArtifactInput, ButtonState, PointerButton};
        let hit = self.html_artifact_hit();
        if button.is_some_and(|(_, state)| state == ElementState::Pressed) {
            if let Some(route) = hit.as_ref().and_then(|key| {
                self.html_artifacts
                    .as_ref()?
                    .slots
                    .get(key)
                    .map(|slot| slot.route)
            }) {
                if let Some(node) =
                    self.context_manager.current_grid().node_by_route_id(route)
                {
                    self.context_manager
                        .current_grid_mut()
                        .set_current_node(node, &mut self.sugarloaf);
                    self.context_manager.select_route_from_current_grid();
                }
                self.renderer.file_tree.set_focused(false);
                self.renderer.notes_sidebar.set_focused(false);
                self.renderer.git_diff_panel.set_focused(false);
                if let Some(agent) =
                    self.context_manager.current_mut().neoism_agent.as_mut()
                {
                    agent.side_panel_mut().set_focused(false);
                    agent.detail_panel_mut().set_focused(false);
                }
            }
        }
        let blocked = self.html_artifact_modal_blocked();
        let window_scale = self.sugarloaf.scale_factor();
        let Some(browser) = self.html_artifacts.as_mut() else {
            return false;
        };
        if blocked {
            browser.blur();
            return false;
        }
        if browser.hovered != hit {
            if let Some(old) = browser.hovered.take() {
                browser.send(&old, ArtifactInput::PointerLeave);
            }
            browser.hovered = hit.clone();
        }
        let mapped = button.and_then(|(button, state)| {
            Some((
                match button {
                    neoism_window::event::MouseButton::Left => PointerButton::Left,
                    neoism_window::event::MouseButton::Middle => PointerButton::Middle,
                    neoism_window::event::MouseButton::Right => PointerButton::Right,
                    neoism_window::event::MouseButton::Back => PointerButton::Back,
                    neoism_window::event::MouseButton::Forward => PointerButton::Forward,
                    _ => return None,
                },
                state,
            ))
        });
        if mapped.is_some_and(|(_, state)| state == ElementState::Pressed)
            && hit != browser.focused
        {
            browser.blur();
            browser.focused = hit.clone();
            if let Some(key) = &hit {
                browser.send(key, ArtifactInput::Focus(true));
            }
        }
        let target = browser.captured.clone().or(hit);
        let Some(key) = target else {
            return false;
        };
        let Some(slot) = browser.slots.get(&key) else {
            return false;
        };
        let [x, y] = local_point(
            [
                self.mouse.x as f32 / window_scale,
                self.mouse.y as f32 / window_scale,
            ],
            slot.viewport,
            slot.scale,
        );
        if let Some((button, state)) = mapped {
            if state == ElementState::Pressed {
                browser.captured = Some(key.clone());
                if !browser.buttons.contains(&button) {
                    browser.buttons.push(button);
                }
            } else {
                browser.buttons.retain(|b| *b != button);
                if browser.buttons.is_empty() {
                    browser.captured = None;
                }
            }
            browser.send(
                &key,
                ArtifactInput::PointerButton {
                    x,
                    y,
                    button,
                    state: if state == ElementState::Pressed {
                        ButtonState::Down
                    } else {
                        ButtonState::Up
                    },
                },
            );
        } else {
            browser.send(&key, ArtifactInput::PointerMove { x, y });
        }
        true
    }

    pub(crate) fn html_artifact_wheel(
        &mut self,
        delta: &neoism_window::event::MouseScrollDelta,
    ) -> bool {
        use neoism_interactive_artifacts::{ArtifactInput, WheelUnit};
        let Some(key) = self.html_artifact_hit() else {
            return false;
        };
        let window_scale = self.sugarloaf.scale_factor();
        let Some(browser) = self.html_artifacts.as_mut() else {
            return false;
        };
        let slot = &browser.slots[&key];
        let [x, y] = local_point(
            [
                self.mouse.x as f32 / window_scale,
                self.mouse.y as f32 / window_scale,
            ],
            slot.viewport,
            slot.scale,
        );
        let (dx, dy) = match delta {
            neoism_window::event::MouseScrollDelta::PixelDelta(p) => (-p.x, -p.y),
            neoism_window::event::MouseScrollDelta::LineDelta(x, y) => (
                -*x as f64 * 32.0 * slot.scale as f64,
                -*y as f64 * 32.0 * slot.scale as f64,
            ),
        };
        browser.send(
            &key,
            ArtifactInput::Wheel {
                x,
                y,
                delta_x: dx,
                delta_y: dy,
                unit: WheelUnit::Pixel,
            },
        );
        true
    }

    // Releases for keys we sent must run BEFORE preedit/shortcut/modal early
    // returns. New key-downs run only after the app shortcut precedence chain.
    pub(crate) fn html_artifact_key(
        &mut self,
        event: &neoism_window::event::KeyEvent,
    ) -> bool {
        use neoism_interactive_artifacts::{
            ArtifactInput, Key as BrowserKey, KeyState, KeyboardEvent, Modifiers,
        };
        let blocked = self.html_artifact_keyboard_blocked();
        let mods = self.modifiers.state();
        let Some(browser) = self.html_artifacts.as_mut() else {
            return false;
        };
        let physical = format!("{:?}", event.physical_key);
        if event.state == ElementState::Released {
            if let (Some(key), Some(mut previous)) =
                (browser.focused.clone(), browser.keys.remove(&physical))
            {
                previous.state = KeyState::Up;
                previous.repeat = false;
                previous.modifiers = Modifiers::empty();
                if mods.shift_key() {
                    previous.modifiers |= Modifiers::SHIFT;
                }
                if mods.control_key() {
                    previous.modifiers |= Modifiers::CONTROL;
                }
                if mods.alt_key() {
                    previous.modifiers |= Modifiers::ALT;
                }
                if mods.super_key() {
                    previous.modifiers |= Modifiers::META;
                }
                browser.send(&key, ArtifactInput::Key(previous));
                return true;
            }
            return false;
        }
        if blocked {
            browser.blur();
            return false;
        }
        let Some(key) = browser.focused.clone() else {
            return false;
        };
        let logical = match &event.logical_key {
            Key::Character(text) => BrowserKey::Character(text.to_string()),
            Key::Named(NamedKey::Space) => BrowserKey::Character(" ".to_owned()),
            Key::Named(NamedKey::Super) => {
                neoism_interactive_artifacts::NamedKey::Meta.into()
            }
            Key::Named(name) => format!("{name:?}").parse().unwrap_or_default(),
            _ => BrowserKey::default(),
        };
        let code = match event.physical_key {
            PhysicalKey::Code(KeyCode::SuperLeft) => {
                neoism_interactive_artifacts::Code::MetaLeft
            }
            PhysicalKey::Code(KeyCode::SuperRight) => {
                neoism_interactive_artifacts::Code::MetaRight
            }
            PhysicalKey::Code(code) => format!("{code:?}").parse().unwrap_or_default(),
            _ => Default::default(),
        };
        let mut modifiers = Modifiers::empty();
        if mods.shift_key() {
            modifiers |= Modifiers::SHIFT;
        }
        if mods.control_key() {
            modifiers |= Modifiers::CONTROL;
        }
        if mods.alt_key() {
            modifiers |= Modifiers::ALT;
        }
        if mods.super_key() {
            modifiers |= Modifiers::META;
        }
        let translated = KeyboardEvent {
            state: KeyState::Down,
            key: logical,
            code,
            modifiers,
            repeat: event.repeat,
            location: match event.location {
                neoism_window::keyboard::KeyLocation::Left => {
                    neoism_interactive_artifacts::Location::Left
                }
                neoism_window::keyboard::KeyLocation::Right => {
                    neoism_interactive_artifacts::Location::Right
                }
                neoism_window::keyboard::KeyLocation::Numpad => {
                    neoism_interactive_artifacts::Location::Numpad
                }
                _ => neoism_interactive_artifacts::Location::Standard,
            },
            is_composing: browser.composing,
        };
        browser.keys.insert(physical, translated.clone());
        browser.send(&key, ArtifactInput::Key(translated));
        true
    }

    pub(crate) fn html_artifact_ime(&mut self, ime: &neoism_window::event::Ime) -> bool {
        use neoism_interactive_artifacts::{
            ArtifactInput, CompositionEvent, CompositionState,
        };
        if self.html_artifact_keyboard_blocked() {
            self.blur_html_artifact();
            return false;
        }
        let Some(browser) = self.html_artifacts.as_mut() else {
            return false;
        };
        let Some(key) = browser.focused.clone() else {
            return false;
        };
        match ime {
            neoism_window::event::Ime::Preedit(text, _) => {
                if text.is_empty() {
                    if browser.composing {
                        browser.send(&key, ArtifactInput::ImeDismissed);
                    }
                    browser.composing = false;
                    return true;
                }
                if !browser.composing {
                    browser.send(
                        &key,
                        ArtifactInput::Ime(CompositionEvent {
                            state: CompositionState::Start,
                            data: String::new(),
                        }),
                    );
                    browser.composing = true;
                }
                browser.send(
                    &key,
                    ArtifactInput::Ime(CompositionEvent {
                        state: CompositionState::Update,
                        data: text.clone(),
                    }),
                );
            }
            neoism_window::event::Ime::Commit(text) => {
                if !browser.composing {
                    browser.send(
                        &key,
                        ArtifactInput::Ime(CompositionEvent {
                            state: CompositionState::Start,
                            data: String::new(),
                        }),
                    );
                }
                browser.send(
                    &key,
                    ArtifactInput::Ime(CompositionEvent {
                        state: CompositionState::End,
                        data: text.clone(),
                    }),
                );
                browser.composing = false;
            }
            neoism_window::event::Ime::Disabled => {
                browser.send(&key, ArtifactInput::ImeDismissed);
                browser.composing = false;
            }
            _ => {}
        }
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn slot(owner: &str, last_used: u64) -> DesktopHtmlSlot {
        DesktopHtmlSlot {
            route: 1,
            owner: owner.into(),
            visible: false,
            last_used,
            sequence: 0,
            awaiting_frame: false,
            source: "markdown:body:html:0".into(),
            html: "<button>state</button>".into(),
            revision: 7,
            image: 0xB8000000,
            viewport: [10.0, 20.0, 2.0, 2.0],
            visible_rect: [0.0; 4],
            scale: 1.0,
            css_scale: 1.0,
            theme: Theme::Dark,
            styles: ArtifactStyles::default(),
        }
    }

    fn frame() -> ArtifactFrame {
        ArtifactFrame {
            key: "view".into(),
            revision: 7,
            sequence: 1,
            width: 2,
            height: 2,
            stride: 12,
            rgba: vec![
                64, 32, 0, 128, 9, 8, 7, 0, 99, 99, 99, 99, 1, 2, 3, 255, 255, 128, 64,
                255,
            ]
            .into(),
        }
    }

    #[test]
    fn custom_light_and_dark_palettes_supply_actual_css_tokens() {
        let mut light = IdeTheme::default();
        // The nominal theme name remains dark: mapping must use resolved colors.
        light.bg = 0xf4ebde;
        light.fg = 0x213243;
        light.surface = 0xe2d9cb;
        light.hover = 0xd0c7bb;
        light.muted = 0xc8bfaa;
        light.dim = 0x675e51;
        light.border = 0xb8af9a;
        light.accent = 0x8a4f18;
        light.red = 0xa13220;
        light.green = 0x346122;
        light.yellow = 0xa78219;
        light.blue = 0x254a83;
        light.cyan = 0x236478;
        light.magenta = 0x7a336a;
        assert!(!light.is_dark());
        let mapped = artifact_styles(&light, "Look Sans", "Config Mono", 4.5);
        let vars = mapped.css_variables().unwrap();
        assert_eq!(vars["--background"], "#f4ebde");
        assert_eq!(vars["--foreground"], "#213243");
        assert_eq!(vars["--card"], "#e2d9cb");
        assert_eq!(vars["--popover"], "#e2d9cb");
        assert_eq!(vars["--accent"], "#d0c7bb");
        assert_eq!(vars["--primary"], "#8a4f18");
        assert_eq!(vars["--muted"], "#c8bfaa");
        assert_eq!(vars["--muted-foreground"], "#675e51");
        assert_eq!(vars["--border"], "#b8af9a");
        assert_eq!(vars["--input"], "#b8af9a");
        for (key, expected) in [
            ("--success", light.green),
            ("--warning", light.yellow),
            ("--info", light.blue),
            ("--destructive", light.red),
            ("--chart-1", light.blue),
            ("--chart-2", light.cyan),
            ("--chart-3", light.green),
            ("--chart-4", light.yellow),
            ("--chart-5", light.magenta),
            ("--chart-6", light.red),
        ] {
            assert_eq!(vars[key], format!("#{expected:06x}"));
        }
        assert_eq!(vars["--font-sans"], "\"Look Sans\", sans-serif");
        assert_eq!(vars["--font-mono"], "\"Config Mono\", monospace");
        assert_eq!(vars["--radius"], "4.5px");
        let mut dark = light;
        dark.bg = 0x01020f;
        dark.fg = 0xc8c6c0;
        dark.surface = 0x111927;
        assert!(dark.is_dark());
        let dark_vars = artifact_styles(&dark, "Look Sans", "Config Mono", 4.5)
            .css_variables()
            .unwrap();
        assert_eq!(dark_vars["--background"], "#01020f");
        assert_eq!(dark_vars["--foreground"], "#c8c6c0");
        assert_eq!(dark_vars["--card"], "#111927");
    }

    #[test]
    fn resolved_lua_style_overrides_are_not_replaced_by_base_palette() {
        let palette = neoism_ui::customization::styled_ide_theme(
            IdeTheme::default(),
            &neoism_lua::StylePatch {
                background: Some("#112233".into()),
                foreground: Some("#abcdef".into()),
                accent: Some("#eebbaa".into()),
                border_color: Some("#445566".into()),
                ..Default::default()
            },
        );
        let vars = artifact_styles(&palette, "Styled Sans", "Config Mono", 9.0)
            .css_variables()
            .unwrap();
        assert_eq!(vars["--background"], "#112233");
        assert_eq!(vars["--foreground"], "#abcdef");
        assert_eq!(vars["--primary"], "#eebbaa");
        assert_eq!(vars["--border"], "#445566");
    }

    #[test]
    fn scroll_translation_clipping_and_reactivation_retain_frame_readiness() {
        let mut view = slot("owner", 1);
        view.sequence = 12;
        view.visible = true;
        let original = view.document("view".into());
        for y in [20.5, -10.0, 900.25, 20.0] {
            assert!(!view.update_geometry(
                [10.0, y, 2.0, 2.0],
                [10.0, y, 2.0, 0.5],
                1.0,
                1.0
            ));
            assert!(!view.awaiting_frame);
            assert_eq!(view.sequence, 12);
            assert_eq!(view.document("view".into()).viewport, original.viewport);
            assert!(frame_pixels(
                &{
                    let mut f = frame();
                    f.sequence = 13;
                    f
                },
                &view
            )
            .is_some());
        }
        view.visible = false;
        assert!(!view.update_geometry(
            [10.0, 20.0, 1.99, 1.99],
            [10.0, 20.0, 1.0, 1.0],
            1.0,
            1.0
        ));
        assert!(!view.awaiting_frame); // Same rounded engine size on return.
        assert_eq!(view.revision, original.revision);
        assert_eq!(view.html, original.html);
        assert!(view.update_geometry(
            [10.0, 20.0, 3.0, 2.0],
            [10.0, 20.0, 3.0, 2.0],
            1.0,
            1.0
        ));
        assert!(view.awaiting_frame);
    }

    #[test]
    fn live_style_update_keeps_source_revision_and_js_view_identity() {
        let mut view = slot("owner", 1);
        view.sequence = 12;
        let old = view.document("view".into());
        let mut palette = IdeTheme::default();
        palette.accent = 0x1a2b3c; // Same dark/light mode; styles must still change.
        let styles = artifact_styles(&palette, "Changed Sans", "Mono", 6.0);
        assert!(view.update_styles(Theme::Dark, &styles));
        let changed = view.document("view".into());
        assert_eq!(changed.revision, old.revision);
        assert_eq!(changed.key, old.key);
        assert_eq!(changed.html, old.html);
        assert_eq!(changed.viewport, old.viewport);
        assert_eq!(view.sequence, 12);
        assert!(!view.awaiting_frame); // Theme-only updates do not blur browser focus.
        assert_eq!(changed.styles, styles);
        assert!(!view.update_styles(Theme::Dark, &styles));
        assert!(view.update_styles(Theme::Light, &styles));
        assert_eq!(view.revision, old.revision);
    }

    #[test]
    fn ui_scale_affects_css_density_not_physical_pointer_coordinates() {
        let viewport = raster_viewport([10.0, 20.0, 100.0, 50.0], 2.0, 1.5).unwrap();
        assert_eq!(
            (viewport.width, viewport.height, viewport.scale),
            (200, 100, 3.0)
        );
        assert_eq!(
            local_point([15.0, 23.0], [10.0, 20.0, 100.0, 50.0], 2.0),
            [10.0, 6.0]
        );
        assert!(raster_viewport([0.0, 0.0, f32::NAN, 20.0], 1.0, 1.0).is_none());
        assert!(raster_viewport([0.0, 0.0, 4097.0, 20.0], 1.0, 1.0).is_none());
    }

    #[test]
    fn snapshot_followup_only_for_first_visible_publication() {
        assert!(needs_snapshot_projection(true, false));
        assert!(!needs_snapshot_projection(true, true));
        assert!(!needs_snapshot_projection(false, false));
        assert!(!needs_snapshot_projection(false, true));
    }

    #[test]
    fn opaque_pixels_are_exact_and_partial_alpha_matches_reference() {
        let mut opaque: Vec<u8> =
            (0..=255).flat_map(|c| [c, 255 - c, c / 2, 255]).collect();
        let expected = opaque.clone();
        unpremultiply_rgba(&mut opaque);
        assert_eq!(opaque, expected);
        for alpha in 0..=255u8 {
            for channel in 0..=255u8 {
                let mut pixel = [channel, channel, channel, alpha];
                unpremultiply_rgba(&mut pixel);
                let expected = if alpha == 0 {
                    0
                } else {
                    ((u32::from(channel) * 255 + u32::from(alpha) / 2) / u32::from(alpha))
                        .min(255) as u8
                };
                assert_eq!(pixel, [expected, expected, expected, alpha]);
            }
        }
    }

    #[test]
    fn packs_padded_rows_and_unpremultiplies_without_dividing_zero() {
        let pixels = frame_pixels(&frame(), &slot("owner", 1)).unwrap();
        assert_eq!(
            pixels,
            [128, 64, 0, 128, 0, 0, 0, 0, 1, 2, 3, 255, 255, 128, 64, 255]
        );
    }

    #[test]
    fn invalid_or_stale_worker_replies_fail_closed() {
        let slot = slot("owner", 1);
        for mutate in [
            (0, 1, 2, 12), // Missing revision.
            (7, 0, 2, 12), // Missing sequence.
            (6, 1, 2, 12), // Old source generation.
            (7, 1, 3, 12), // Wrong viewport.
            (7, 1, 2, 0),  // Zero stride, not a chunks(0) panic.
        ] {
            let mut f = frame();
            f.revision = mutate.0;
            f.sequence = mutate.1;
            f.width = mutate.2;
            f.stride = mutate.3;
            assert!(frame_pixels(&f, &slot).is_none());
        }
        let mut truncated = frame();
        truncated.rgba = vec![0; 19].into();
        assert!(frame_pixels(&truncated, &slot).is_none());
        let mut consumed = slot;
        consumed.sequence = 1;
        assert!(frame_pixels(&frame(), &consumed).is_none());
    }

    #[test]
    fn scrolling_away_preserves_document_revision_and_html() {
        let slots = HashMap::from([("a".into(), slot("owner", 1))]);
        let owners = HashSet::from(["owner".into()]);
        assert!(lifecycle_removals(&slots, &owners, &HashSet::new()).is_empty());
        let hidden = slots["a"].document("a".into());
        assert!(!hidden.visible);
        assert_eq!(hidden.revision, 7);
        assert_eq!(hidden.html, "<button>state</button>");
    }

    #[test]
    fn removes_old_session_and_evicts_inactive_lru_not_visible_views() {
        let mut slots: HashMap<_, _> = (0..16)
            .map(|i| (format!("v{i}"), slot("owner", i)))
            .collect();
        let owners = HashSet::from(["owner".into()]);
        let visible = HashSet::from(["v0".into(), "new".into()]);
        assert_eq!(lifecycle_removals(&slots, &owners, &visible), ["v1"]);
        slots.get_mut("v2").unwrap().owner = "old-session".into();
        assert_eq!(lifecycle_removals(&slots, &owners, &visible), ["v2"]);
        assert!(lifecycle_removals(&slots, &HashSet::new(), &HashSet::new()).len() == 16);
    }

    #[test]
    fn owner_identity_distinguishes_sessions_and_routes() {
        assert_ne!(owner_key(1, "a:b"), owner_key(1, "a"));
        assert_ne!(owner_key(1, "a"), owner_key(2, "a"));
    }
}
