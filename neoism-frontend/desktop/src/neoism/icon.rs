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

/// Look at the foreground process group on `main_fd` and decide whether
/// it's one of the supported agents. Reads `/proc/<pgid>/comm` and
/// `/proc/<pgid>/cmdline` once per call — cheap, but the caller should
/// throttle to avoid running every frame.
#[cfg(target_os = "linux")]
pub fn detect_agent(
    main_fd: std::os::unix::io::RawFd,
    _shell_pid: u32,
) -> Option<AgentKind> {
    use std::os::raw::c_int;

    // tcgetpgrp returns the foreground process group id for the
    // controlling tty. With multiple processes in the chain (npx → node
    // → native binary, as with codex), the pgid is the leader's pid;
    // we read both `comm` and `cmdline` so the agent matches whether
    // it's run directly or via a wrapper.
    let pgid: c_int = unsafe { libc::tcgetpgrp(main_fd) };
    if pgid <= 0 {
        return None;
    }
    let pgid = pgid as u32;

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
pub fn detect_agent(
    main_fd: std::os::unix::io::RawFd,
    _shell_pid: u32,
) -> Option<AgentKind> {
    use std::os::raw::c_int;

    let pgid: c_int = unsafe { libc::tcgetpgrp(main_fd) };
    if pgid <= 0 {
        return None;
    }

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

#[cfg(not(any(target_os = "linux", target_os = "macos")))]
pub fn detect_agent(_main_fd: i32, _shell_pid: u32) -> Option<AgentKind> {
    None
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
