// Tab-strip icons for agent CLIs (Claude Code, OpenAI Codex, OpenCode).
// When one of these is the foreground program in the terminal tab, the
// generic terminal glyph is replaced by the tool's logo.
//
// The POD identity bits (the `AgentKind` enum, panel ids, image ids)
// live in the shared crate so the web frontend can speak the same
// agent vocabulary without dragging desktop-only dependencies in.
// This file owns the asset bytes, `image_rs` decode, `sugarloaf` image
// upload, and native foreground-process detection — none of which the
// web build needs or could compile.

#[cfg(any(target_os = "linux", target_os = "macos", target_os = "windows"))]
use neoism_backend::event::{EventProxy, RioEvent, RioEventType, WindowId};
use neoism_backend::sugarloaf::{
    ColorType, GraphicData, GraphicDataEntry, GraphicId, GraphicOverlay, Sugarloaf,
};

// Re-export the POD pieces so existing call sites
// (`crate::neoism::icon::AgentKind`, `ICON_PANEL_ID`, ...) keep
// resolving without any change.
pub use neoism_ui::panels::agent_pane::icon::{
    AgentKind, CLAUDE_IMAGE_ID, CODEX_IMAGE_ID, ICON_PANEL_ID, NEOISM_IMAGE_ID,
    OPENCODE_IMAGE_ID, SIDE_PANEL_ICON_PANEL_ID,
};

const CLAUDE_PNG: &[u8] = include_bytes!("../../assets/icons/claude.png");
const CODEX_PNG: &[u8] = include_bytes!("../../assets/icons/codex.png");
const OPENCODE_PNG: &[u8] = include_bytes!("../../assets/icons/opencode.png");
const NEOISM_PNG: &[u8] = include_bytes!("../../assets/icons/neoism.png");

/// Decode the embedded PNGs and upload them to sugarloaf's image
/// store. Returns `true` once all icons are registered. Idempotent —
/// safe to call every frame; subsequent calls return immediately.
pub fn register_agent_icons(sugarloaf: &mut Sugarloaf) -> bool {
    let entries: [(u32, &[u8]); 4] = [
        (CLAUDE_IMAGE_ID, CLAUDE_PNG),
        (CODEX_IMAGE_ID, CODEX_PNG),
        (OPENCODE_IMAGE_ID, OPENCODE_PNG),
        (NEOISM_IMAGE_ID, NEOISM_PNG),
    ];
    for (id, bytes) in entries {
        if sugarloaf.image_data.contains_key(&id) {
            continue;
        }
        let img = match image_rs::load_from_memory(bytes) {
            Ok(i) => i.to_rgba8(),
            Err(_) => return false,
        };
        let (w, h) = img.dimensions();
        let pixels = img.into_raw();
        let entry = GraphicDataEntry::from_graphic_data(GraphicData {
            id: GraphicId::new(id as u64),
            width: w as usize,
            height: h as usize,
            color_type: ColorType::Rgba,
            pixels,
            is_opaque: false,
            resize: None,
            display_width: None,
            display_height: None,
            transmit_time: std::time::Instant::now(),
        });
        sugarloaf.image_data.insert(id, entry);
    }
    true
}

pub fn push_cropped_icon_overlay(
    sugarloaf: &mut Sugarloaf,
    kind: AgentKind,
    x: f32,
    y: f32,
    width: f32,
    height: f32,
    source_rect: [f32; 4],
) {
    push_icon_overlay_to_panel_with_options(
        sugarloaf,
        ICON_PANEL_ID,
        kind,
        x,
        y,
        width,
        height,
        1,
        source_rect,
    );
}

#[allow(clippy::too_many_arguments)]
fn push_icon_overlay_to_panel_with_options(
    sugarloaf: &mut Sugarloaf,
    panel_id: usize,
    kind: AgentKind,
    x: f32,
    y: f32,
    width: f32,
    height: f32,
    z_index: i32,
    source_rect: [f32; 4],
) {
    let scale = sugarloaf.scale_factor();
    sugarloaf.push_image_overlay(
        panel_id,
        GraphicOverlay {
            image_id: kind.image_id(),
            x: x * scale,
            y: y * scale,
            width: width * scale,
            height: height * scale,
            z_index,
            source_rect,
        },
    );
}

pub fn clear_icon_overlays(sugarloaf: &mut Sugarloaf) {
    sugarloaf.clear_image_overlays_for(ICON_PANEL_ID);
}

pub fn clear_side_panel_icon_overlays(sugarloaf: &mut Sugarloaf) {
    sugarloaf.clear_image_overlays_for(SIDE_PANEL_ICON_PANEL_ID);
}

/// A cheap tty ioctl used on the render thread. Native process metadata I/O
/// happens later on [`AgentDetectionWorker`], never while painting a frame.
#[cfg(any(target_os = "linux", target_os = "macos"))]
pub fn foreground_process_group(main_fd: std::os::unix::io::RawFd) -> Option<u32> {
    use std::os::raw::c_int;

    let pgid: c_int = unsafe { libc::tcgetpgrp(main_fd) };
    if pgid <= 0 {
        return None;
    }
    Some(pgid as u32)
}

/// Windows counterpart: a ConPTY has no foreground process group (no
/// `tcgetpgrp`), so the render-thread step is just handing over the
/// pane's shell PID. The Toolhelp descendant walk that resolves the
/// actual foreground process happens later on [`AgentDetectionWorker`],
/// never while painting a frame.
#[cfg(target_os = "windows")]
pub fn foreground_process_group(shell_pid: u32) -> Option<u32> {
    (shell_pid != 0).then_some(shell_pid)
}

/// Inspect a Linux foreground process group from the background detection
/// worker. With wrapper chains (npx → node → native binary), the process-group
/// leader's command line still carries the identifying package/binary name.
#[cfg(target_os = "linux")]
fn detect_agent_for_process_group(pgid: u32) -> Option<AgentKind> {
    let comm = std::fs::read_to_string(format!("/proc/{pgid}/comm"))
        .map(|s| s.trim().to_string())
        .unwrap_or_default();
    let cmdline_bytes =
        std::fs::read(format!("/proc/{pgid}/cmdline")).unwrap_or_default();
    let cmdline = String::from_utf8_lossy(&cmdline_bytes);

    detect_agent_from_process_identity(&comm, &cmdline)
}

/// macOS has no `/proc`, but the foreground process-group contract is the
/// same. Query every member of the foreground group through `ps`; this still
/// works when an `npx`/shell wrapper is the group leader and Node is the child.
/// `-ww` matters because the identifying package path is often near the end
/// of the command line.
#[cfg(target_os = "macos")]
fn detect_agent_for_process_group(pgid: u32) -> Option<AgentKind> {
    let pgid = pgid.to_string();
    let output = std::process::Command::new("/bin/ps")
        .args(["-ww", "-g", &pgid, "-o", "comm=", "-o", "args="])
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let identity = String::from_utf8_lossy(&output.stdout);
    detect_agent_from_process_identity("", &identity)
}

/// Windows: the probe carries the pane's shell PID (there is no ConPTY
/// process group) and `teletypewriter` walks the descendant tree
/// (Toolhelp snapshot) for the most recently created leaf. Its name
/// stands in for the Linux `comm`; the full exe image path stands in
/// for the command line, so a natively installed
/// `...\AppData\...\claude.exe` still identifies (`process_name_is`
/// splits on `\` and trims `.exe`).
#[cfg(target_os = "windows")]
fn detect_agent_for_process_group(shell_pid: u32) -> Option<AgentKind> {
    let comm = teletypewriter::foreground_process_name(shell_pid);
    if comm.is_empty() {
        return None;
    }
    let command = teletypewriter::foreground_process_path(shell_pid)
        .map(|path| path.display().to_string())
        .unwrap_or_default();
    detect_agent_from_process_identity(&comm, &command)
}

#[cfg(any(target_os = "linux", target_os = "macos", target_os = "windows"))]
#[derive(Clone, Copy, Debug)]
pub struct AgentProbe {
    pub route_id: usize,
    pub is_root: bool,
    /// Foreground process group on unix; the pane's shell PID on
    /// Windows (see the per-platform `foreground_process_group`).
    pub process_group: u32,
}

#[cfg(any(target_os = "linux", target_os = "macos", target_os = "windows"))]
struct AgentDetectionRequest {
    workspace_token: usize,
    probes: Vec<AgentProbe>,
    event_proxy: EventProxy,
    window_id: WindowId,
}

#[cfg(any(target_os = "linux", target_os = "macos", target_os = "windows"))]
pub struct AgentDetectionResult {
    pub workspace_token: usize,
    pub detected: Vec<(usize, bool, AgentKind)>,
}

/// One long-lived native process-inspection lane. Requests are bounded and
/// latest-wins at the caller, so a slow `ps` (or Toolhelp snapshot on
/// Windows) cannot accumulate work or block rendering. The worker
/// explicitly wakes winit after publishing a result.
#[cfg(any(target_os = "linux", target_os = "macos", target_os = "windows"))]
pub struct AgentDetectionWorker {
    request_tx: std::sync::mpsc::SyncSender<AgentDetectionRequest>,
    result_rx: std::sync::mpsc::Receiver<AgentDetectionResult>,
}

#[cfg(any(target_os = "linux", target_os = "macos", target_os = "windows"))]
impl AgentDetectionWorker {
    pub fn spawn() -> Option<Self> {
        let (request_tx, request_rx) =
            std::sync::mpsc::sync_channel::<AgentDetectionRequest>(1);
        let (result_tx, result_rx) = std::sync::mpsc::channel::<AgentDetectionResult>();
        std::thread::Builder::new()
            .name("neoism-agent-detect".into())
            .spawn(move || {
                while let Ok(mut request) = request_rx.recv() {
                    // If a workspace switch arrived while the prior result was
                    // waiting to be consumed, inspect only the newest view.
                    while let Ok(newer) = request_rx.try_recv() {
                        request = newer;
                    }
                    let detected = request
                        .probes
                        .into_iter()
                        .filter_map(|probe| {
                            detect_agent_for_process_group(probe.process_group)
                                .map(|agent| (probe.route_id, probe.is_root, agent))
                        })
                        .collect();
                    if result_tx
                        .send(AgentDetectionResult {
                            workspace_token: request.workspace_token,
                            detected,
                        })
                        .is_err()
                    {
                        break;
                    }
                    request.event_proxy.send_event(
                        RioEventType::Rio(RioEvent::Render),
                        request.window_id,
                    );
                }
            })
            .ok()?;
        Some(Self {
            request_tx,
            result_rx,
        })
    }

    pub fn request(
        &self,
        workspace_token: usize,
        probes: Vec<AgentProbe>,
        event_proxy: EventProxy,
        window_id: WindowId,
    ) -> bool {
        self.request_tx
            .try_send(AgentDetectionRequest {
                workspace_token,
                probes,
                event_proxy,
                window_id,
            })
            .is_ok()
    }

    pub fn try_result(&self) -> Option<AgentDetectionResult> {
        self.result_rx.try_recv().ok()
    }
}

fn detect_agent_from_process_identity(comm: &str, command: &str) -> Option<AgentKind> {
    let comm = comm.trim().to_ascii_lowercase();
    let command = command.to_ascii_lowercase();

    if process_name_is(&comm, "claude")
        || command_args_contain_process(&command, "claude")
        || command.contains("@anthropic-ai/claude-code")
    {
        return Some(AgentKind::Claude);
    }
    if process_name_is(&comm, "opencode")
        || command_args_contain_process(&command, "opencode")
    {
        return Some(AgentKind::OpenCode);
    }
    if process_name_is(&comm, "codex")
        || command_args_contain_process(&command, "codex")
        || command.contains("@openai/codex")
    {
        return Some(AgentKind::Codex);
    }
    None
}

fn command_args_contain_process(command: &str, name: &str) -> bool {
    command
        .split(|ch: char| ch == '\0' || ch.is_whitespace())
        .any(|arg| process_name_is(arg, name))
}

fn process_name_is(value: &str, name: &str) -> bool {
    let value = value.trim_matches(|ch: char| {
        matches!(ch, '\'' | '"' | '(' | ')' | '[' | ']' | ',' | ';')
    });
    let basename = value
        .rsplit(['/', '\\'])
        .next()
        .unwrap_or(value)
        .trim_end_matches(".exe");
    basename == name || basename == format!("{name}.js")
}

#[cfg(test)]
mod tests {
    use super::{detect_agent_from_process_identity, AgentKind};

    #[test]
    fn classifies_direct_agent_processes() {
        assert_eq!(
            detect_agent_from_process_identity("claude", "claude"),
            Some(AgentKind::Claude)
        );
        assert_eq!(
            detect_agent_from_process_identity("opencode", "/opt/homebrew/bin/opencode"),
            Some(AgentKind::OpenCode)
        );
        assert_eq!(
            detect_agent_from_process_identity("codex", "/usr/local/bin/codex"),
            Some(AgentKind::Codex)
        );
    }

    #[test]
    fn classifies_node_package_agent_processes() {
        assert_eq!(
            detect_agent_from_process_identity(
                "node",
                "node /opt/homebrew/lib/node_modules/@anthropic-ai/claude-code/cli.js"
            ),
            Some(AgentKind::Claude)
        );
        assert_eq!(
            detect_agent_from_process_identity(
                "node",
                "node /opt/homebrew/lib/node_modules/@openai/codex/bin/codex.js"
            ),
            Some(AgentKind::Codex)
        );
    }

    #[test]
    fn does_not_match_agent_words_inside_unrelated_names() {
        assert_eq!(
            detect_agent_from_process_identity(
                "node",
                "node /tmp/my-codex-notes/server.js"
            ),
            None
        );
    }
}
