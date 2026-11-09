//! Tool calls in the agent transcript, drawn as GPUiX's shared painter drew them
//! (`agent_pane/view/tool_message/*`): a 30 pt header (status dot or orbiting spinner, service
//! symbol, compact title, `+/-` counts, status, disclosure) over an optional body: output text,
//! read-group children, per-file diff accordions, a checklist, or a shell transcript.
//!
//! Geometry is the original's, in points × `Type::scale()`, measured from the row's left edge,
//! which sits 18 pt left of assistant text (`ASSISTANT_TEXT_PAD_LEFT`).

mod card;
mod diff;
mod files;
mod group;
mod icons;
mod shell;
mod text;
mod todos;

pub use group::{group_at, visible};
pub use icons::Icons;

use crate::app::App;
use crate::ui::theme::{Palette, rgb};
use neoism_protocol::agent::HistoryMessage;
use rune::{Color, El, Tx, el};
use std::collections::{HashMap, HashSet};
use text::Text;

/// The row's left edge sits this far left of assistant text.
const TEXT_PAD_LEFT: f32 = 18.0;
/// The row runs this far past the right edge of assistant text (its 30 pt right margin).
const TEXT_PAD_RIGHT: f32 = 30.0;
const HEADER_H: f32 = 30.0;
const GROUP_BODY_Y: f32 = 36.0;
const BODY_ROW_H: f32 = 20.0;
const BODY_MAX_ROWS: usize = 12;
/// Width of the transcript text column before the first layout reports it.
const FALLBACK_TEXT_WIDTH: f32 = 860.0;

/// Expansion, child selection and the measured column, kept across frames.
#[derive(Default)]
pub struct ToolState {
    expanded: HashSet<String>,
    selected_child: HashMap<String, String>,
    /// Assistant text width, from the latest tool row layout.
    text_width: f32,
    /// Frame clock (seconds) for the running spinner.
    pub clock: f64,
    pub icons: Icons,
}

impl ToolState {
    pub fn expanded(&self, id: &str) -> bool { self.expanded.contains(id) }

    /// Open a row (or `"{id}:{n}"` diff section) without a press, e.g. for headless scenes.
    pub fn expand(&mut self, id: &str) { self.expanded.insert(id.to_owned()); }

    fn toggle(&mut self, id: &str) {
        if !self.expanded.remove(id) { self.expanded.insert(id.to_owned()); }
    }

    fn toggle_child(&mut self, group: &str, child: &str) {
        if self.selected_child.get(group).is_some_and(|c| c == child) {
            self.selected_child.remove(group);
        } else {
            self.selected_child.insert(group.to_owned(), child.to_owned());
        }
    }
}

/// Whether a visible tool in `messages` is running, so `poll` keeps the spinner ticking.
pub fn animating(messages: &[HistoryMessage], live_from: usize) -> bool {
    messages.iter().enumerate().skip(live_from).any(|(i, m)| {
        visible(m, i, live_from) && (is_running(&m.status) || m.composition_children.iter().any(|c| is_running(&c.status)))
    })
}

fn is_running(status: &str) -> bool { matches!(status, "running" | "streaming") }

impl App {
    /// Whether the open conversation shows a running tool (its spinner needs frames).
    pub fn running_tools(&self) -> bool {
        let crate::tabs::TabKind::Agent { session: Some(id) } = &self.active_tab().kind else { return false };
        self.sessions.get(id).is_some_and(|s| animating(&s.messages, s.live_from.unwrap_or(usize::MAX)))
    }
}

/// Per-row drawing context: theme, scale, row width.
pub(crate) struct Cx<'a> {
    app: &'a App,
    p: &'static Palette,
    s: f32,
    /// The row's full width (assistant text width plus both pads).
    w: f32,
}

impl<'a> Cx<'a> {
    fn new(app: &'a App) -> Cx<'a> {
        let s = app.ty.scale();
        let text_w = if app.tools.text_width > 0.0 { app.tools.text_width } else { FALLBACK_TEXT_WIDTH };
        Cx { app, p: app.pal, s, w: text_w + (TEXT_PAD_LEFT + TEXT_PAD_RIGHT) * s }
    }

    fn narrower(&self, by: f32) -> Cx<'a> {
        Cx { app: self.app, p: self.p, s: self.s, w: (self.w - by).max(1.0) }
    }

    fn text(&self, content: impl Into<String>, points: f32) -> Text { Text::new(content, points * self.s, self.app) }
    fn pt(&self, points: f32) -> f32 { points * self.s }
    fn color(&self, c: u32) -> Color { rgb(c) }
}

/// One transcript tool row (a tool call, a folded group, or a composition with its children).
pub fn row(app: &App, message: &HistoryMessage) -> El {
    let cx = Cx::new(app);
    let (body, height) = card::message(&cx, message, false);
    // The measured outer width is the assistant text width; the row hangs past it on both sides.
    el().key(format!("tool:{}", message.id)).w_full().h(height).mt(cx.pt(18.0))
        .on_layout_tx(|a: &mut App, [_, _, w, _]: [f32; 4], tx: &mut Tx| {
            if (a.tools.text_width - w).abs() > 0.5 { a.tools.text_width = w; } else { tx.live_only(); }
        })
        .child(body.absolute(-cx.pt(TEXT_PAD_LEFT), 0.0))
}

/// `Name(args)` reads `Name  args`; control characters become spaces.
fn compact_title(title: &str) -> String {
    let title = title.trim();
    let title = if title.is_empty() { "Tool" } else { title };
    let compact = match title.split_once('(') {
        Some((name, args)) => match args.strip_suffix(')') {
            Some(args) => format!("{}  {}", name.trim_end(), args),
            None => title.to_owned(),
        },
        None => title.to_owned(),
    };
    compact.replace(['\r', '\n', '\t'], " ")
}

fn accent(p: &Palette, status: &str) -> u32 {
    match status {
        "error" | "interrupted" => p.red,
        "completed" => p.green,
        _ => p.yellow,
    }
}


#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn titles_compact_their_arguments() {
        assert_eq!(compact_title("Read(src/a.rs)"), "Read  src/a.rs");
        assert_eq!(compact_title("Task(@explore \u{b7} Check parser)"), "Task  @explore \u{b7} Check parser");
        assert_eq!(compact_title(""), "Tool");
        assert_eq!(compact_title("Bash(echo\nhi)"), "Bash  echo hi");
    }

    #[test]
    fn task_rows_find_their_child_session() {
        assert_eq!(crate::agent::family::task_id("task_id: ses_abc (use this to check)\nstatus: running", ""), Some("ses_abc".into()));
        assert_eq!(crate::agent::family::task_id("no id here", ""), None);
    }
}
