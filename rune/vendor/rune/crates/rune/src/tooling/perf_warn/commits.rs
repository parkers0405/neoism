//! The commit warnings: something in the app that commits the view on most frames of a scroll,
//! which a scroll itself never needs (Rune moves the content, mounts virtual rows and paints
//! hover without one); an app stepping its own scroll glide with an instant scroll every
//! frame; and hover kept in state on many rows, so each row the pointer crosses commits. Each
//! names its source: `poll`, a handler with its element and where it was set, or the layout
//! read that moved.

#[cfg(debug_assertions)]
use std::collections::HashMap;

/// Committing frames one source may take in a scroll, and the share of the scroll's frames
/// (one in this many), before it warns. A list loading its next page commits a few times.
#[cfg(debug_assertions)]
const COMMITS: u32 = 6;
#[cfg(debug_assertions)]
const SHARE: u32 = 4;
/// Frames in a row that commit and ask for an instant scroll before it warns.
#[cfg(debug_assertions)]
const INSTANT_FRAMES: u32 = 8;
/// Elements one hover handler (by where it was set) commits for before it warns.
#[cfg(debug_assertions)]
const HOVER_ROWS: usize = 8;

/// What made a commit, as far as the app goes.
#[cfg(debug_assertions)]
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) enum Cause {
    Poll,
    /// A handler, by the name the docs give it (`on_hover_change`).
    Handler(&'static str),
    /// A view's layout read (`Cx::layout`, `Cx::window_layout`) moved.
    Read,
}

/// One source that asked this frame for a commit.
#[cfg(debug_assertions)]
struct Source {
    /// What it warns once per: the handler's registration site, else its element's path
    /// without digits, so one list's rows count as one source.
    site: String,
    cause: Cause,
    /// The element's path or the read's key.
    subject: Option<String>,
    at: Option<&'static std::panic::Location<'static>>,
}

/// One window's commit meters. Release builds hold nothing.
#[derive(Default)]
pub(crate) struct CommitMeter {
    #[cfg(debug_assertions)]
    meter: Meter,
}

#[cfg(debug_assertions)]
#[derive(Default)]
struct Meter {
    sources: Vec<Source>,
    /// Frames of the scroll underway, and how many of them each site committed.
    frames: u32,
    counts: HashMap<String, u32>,
    /// Frames in a row that committed and asked for an instant scroll.
    instant: u32,
    /// The elements each hover handler committed for, by where it was set, until it warns.
    hover_rows: HashMap<&'static std::panic::Location<'static>, Vec<String>>,
}

impl CommitMeter {
    /// `poll` returned true.
    #[inline(always)]
    pub(crate) fn poll(&mut self) {
        #[cfg(debug_assertions)]
        self.meter.sources.push(Source { site: "poll".into(), cause: Cause::Poll, subject: None, at: None });
    }

    /// Handler `name` on the element at `path`, set at `at`, asked for a commit.
    #[inline(always)]
    #[cfg_attr(not(debug_assertions), allow(unused_variables))]
    pub(crate) fn handler(&mut self, name: &'static str, path: Option<&str>, at: Option<&'static std::panic::Location<'static>>) {
        #[cfg(debug_assertions)]
        {
            let site = match (at, path) {
                (Some(at), _) => format!("{name} {at}"),
                (None, Some(path)) => super::handler_site(name, path),
                (None, None) => name.to_owned(),
            };
            if let (Some(at), Some(path), "on_hover" | "on_hover_change") = (at, path, name) { self.hover_row(name, at, path); }
            self.meter.sources.push(Source { site, cause: Cause::Handler(name), subject: path.map(str::to_owned), at });
        }
    }

    #[cfg(debug_assertions)]
    fn hover_row(&mut self, name: &'static str, at: &'static std::panic::Location<'static>, path: &str) {
        let rows = self.meter.hover_rows.entry(at).or_default();
        if rows.len() >= HOVER_ROWS || rows.iter().any(|p| p == path) { return; }
        rows.push(path.to_owned());
        if rows.len() < HOVER_ROWS { return; }
        let example = rows[0].clone();
        super::warn_once(format!("hover rows {at}"), || format!(
            "`{name}` set at {at} committed the view for {HOVER_ROWS} elements (as {example:?}): each one the pointer crosses \
             runs the whole view; {}", handler_fix(name)
        ));
    }

    /// A view's layout read of `key`, made at `at`, moved, so the view runs again.
    #[cfg(debug_assertions)]
    pub(crate) fn read(&mut self, key: &str, at: Option<&'static std::panic::Location<'static>>) {
        self.meter.sources.push(Source { site: format!("read {key}"), cause: Cause::Read, subject: Some(key.to_owned()), at });
    }

    /// The frame's commits are done. `scrolling`: a hand, momentum or a glide moves a viewport;
    /// `committed`: the frame committed; `instant`: the viewport an instant scroll this frame
    /// asked to move, if any.
    #[inline(always)]
    #[cfg_attr(not(debug_assertions), allow(unused_variables))]
    pub(crate) fn frame_ends(&mut self, scrolling: bool, committed: bool, instant: Option<&str>) {
        #[cfg(debug_assertions)]
        {
            let m = &mut self.meter;
            let sources = std::mem::take(&mut m.sources);
            match instant.filter(|_| committed) {
                Some(key) => {
                    m.instant += 1;
                    if m.instant == INSTANT_FRAMES { super::warn_once(format!("instant scroll {key}"), || instant_message(key)); }
                }
                None => m.instant = 0,
            }
            if !scrolling {
                m.frames = 0;
                m.counts.clear();
                return;
            }
            m.frames += 1;
            if !committed { return; }
            let mut seen: Vec<&str> = Vec::new();
            for s in &sources {
                if seen.contains(&s.site.as_str()) { continue; }
                seen.push(&s.site);
                let count = m.counts.entry(s.site.clone()).or_default();
                *count += 1;
                if *count >= COMMITS && *count * SHARE >= m.frames {
                    let (count, frames) = (*count, m.frames);
                    super::warn_once(format!("scroll commit {}", s.site), || message(s, count, frames));
                }
            }
        }
    }
}

/// The warning for source `s`, which committed `count` of a scroll's `frames` frames.
#[cfg(debug_assertions)]
fn message(s: &Source, count: u32, frames: u32) -> String {
    let at = s.at.map_or_else(String::new, |at| format!(" (set at {at})"));
    let subject = s.subject.as_deref().unwrap_or("");
    let (who, fix) = match s.cause {
        Cause::Poll => ("`poll` returned true and".to_owned(),
            "return true only when visible state changed; to act on where a scroll stops, wait until it \
             has been still a moment (`Tx::wake_after`), and follow it frame by frame with \
             `El::on_scroll_offset` and live values"),
        Cause::Read => {
            let window = crate::runtime::window_read_key(subject);
            let at = s.at.map_or_else(String::new, |at| format!(" at {at}"));
            let read = match window {
                Some(key) => format!("the view's `Cx::window_layout` read of {key:?}{at}"),
                None => format!("the view's layout read of {subject:?}{at}"),
            };
            (format!("{read} moved and"),
             if window.is_some() { "read `Cx::layout_size` or `Cx::layout` (unscrolled) unless the view needs where the \
                 element is in the window, and then only through `window_layout_with` and what it computes" }
             else { "read only what the view needs (`Cx::layout_size`, `Cx::layout_with`), or follow it with \
                 `El::on_layout_tx` and a live value" })
        }
        Cause::Handler(name) => (format!("`{name}` on {subject:?}{at}"), handler_fix(name)),
    };
    format!("{who} committed the view on {count} of {frames} frames of a scroll: {fix}")
}

#[cfg(debug_assertions)]
fn handler_fix(name: &str) -> &'static str {
    match name {
        "on_hover" | "on_hover_change" => "paint hover with `El::hover`, `hover_group` and `group_*` styles (`hover_alone` for a \
            target inside a lit row) instead of state, or say `Tx::live_only` when the handler only sets live values",
        "on_layout_tx" | "on_drawn_layout_tx" | "on_pointer_move_local" | "on_wheel" => "when the handler only moves live values \
            or bookkeeping, say `Tx::live_only` (`on_layout` and `on_drawn_layout` take one in their `_tx` forms)",
        "on_visible_rows" => "return true only when the visible range changed what the view shows",
        _ => "say `Tx::live_only` when the handler changes nothing the view reads",
    }
}

#[cfg(debug_assertions)]
fn instant_message(key: &str) -> String {
    format!(
        "an instant scroll (`Tx::scroll_to_instant`, `scroll_to_y`, `scroll_key_to`) moved {key:?} on {INSTANT_FRAMES} frames \
         in a row, each with a commit: an app stepping its own glide this way runs the view every frame; glide with `Tx::scroll_to_with(key, x, y, spring)`, \
         which runs on Rune's clock with no commits"
    )
}
