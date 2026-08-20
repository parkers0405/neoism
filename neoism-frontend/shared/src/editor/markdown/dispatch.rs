//! Host-neutral markdown key dispatcher.
//!
//! This is the shared, full-breadth port of the desktop
//! `Screen::dispatch_markdown_key` match (operators, visual mode, table
//! editing, list indent, title editing, the `/` block-template menu,
//! wiki-link completion queries, incsearch entry, undo/redo) with every
//! host-only side effect lifted into a POD [`MarkdownDispatchEffects`]
//! plan. The desktop fork keeps its own copy for now (behavioral parity
//! is verified by tests here); the wasm/web host calls this directly so
//! both frontends converge on one keymap.
//!
//! Host responsibilities that stay outside:
//! - leader-key chords (`<Space>x` close-tab / `<Space>h` split) and the
//!   leader timer — the host passes `flushed_leader` in;
//! - clipboard IO (paste text comes in, `clipboard_out` goes out);
//! - opening menus / palettes / links (returned as plan fields);
//! - notebook & epub surfaces (hosts route those before calling).

use std::path::{Component, Path, PathBuf};

use super::bridge_policy::{
    markdown_ctrl_action, markdown_dispatch_finalize,
    markdown_flushed_leader_scrolls_normal_mode, markdown_normal_colon_opens_palette,
    markdown_super_z_intent, MarkdownBridgeModifiers, MarkdownCtrlAction,
    MarkdownCtrlKeyKind,
};
use super::rendered_inline_text;
use super::types::{MarkdownCursorLink, MarkdownMode, MarkdownPane};
use super::vim::{VimAction, VimKeyFeed, VimOperator, VimTarget};

// ---------------------------------------------------------------------------
// Key model
// ---------------------------------------------------------------------------

/// Non-character keys the dispatcher branches on. Mirrors the subset of
/// `NamedKey` the desktop match arms consult.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MarkdownNamedKey {
    Escape,
    Enter,
    Backspace,
    Delete,
    Tab,
    ArrowLeft,
    ArrowRight,
    ArrowUp,
    ArrowDown,
    Home,
    End,
    PageUp,
    PageDown,
    Space,
}

/// One logical key press, host-adapted. `Char` carries the produced
/// character (post-Shift, the way browsers report `event.key`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MarkdownDispatchKey<'a> {
    Named(MarkdownNamedKey),
    Char(&'a str),
}

/// Map a browser `KeyboardEvent.key` value onto the dispatcher's key
/// model. Returns `None` for keys the dispatcher has no arm for
/// (function keys, `Dead`, modifiers, ...), which the host reports as
/// unhandled.
pub fn parse_browser_markdown_key(key: &str) -> Option<MarkdownDispatchKey<'_>> {
    use MarkdownNamedKey::*;
    Some(match key {
        "Escape" => MarkdownDispatchKey::Named(Escape),
        "Enter" => MarkdownDispatchKey::Named(Enter),
        "Backspace" => MarkdownDispatchKey::Named(Backspace),
        "Delete" => MarkdownDispatchKey::Named(Delete),
        "Tab" => MarkdownDispatchKey::Named(Tab),
        "ArrowLeft" => MarkdownDispatchKey::Named(ArrowLeft),
        "ArrowRight" => MarkdownDispatchKey::Named(ArrowRight),
        "ArrowUp" => MarkdownDispatchKey::Named(ArrowUp),
        "ArrowDown" => MarkdownDispatchKey::Named(ArrowDown),
        "Home" => MarkdownDispatchKey::Named(Home),
        "End" => MarkdownDispatchKey::Named(End),
        "PageUp" => MarkdownDispatchKey::Named(PageUp),
        "PageDown" => MarkdownDispatchKey::Named(PageDown),
        " " => MarkdownDispatchKey::Named(Space),
        _ if key.chars().count() == 1
            && !key.chars().next().is_some_and(char::is_control) =>
        {
            MarkdownDispatchKey::Char(key)
        }
        _ => return None,
    })
}

// ---------------------------------------------------------------------------
// Effects plan
// ---------------------------------------------------------------------------

/// Everything a host must act on after one dispatched key. All fields
/// default to "nothing to do".
#[derive(Clone, Debug, Default, PartialEq)]
pub struct MarkdownDispatchEffects {
    /// At least one arm consumed the key.
    pub handled: bool,
    /// Reset the trail-cursor animation (desktop) / snap any cursor FX.
    pub snap_cursor: bool,
    /// Write this text to the system clipboard (unnamed-register sync).
    pub clipboard_out: Option<String>,
    /// Surface a "Yanked N lines" notification.
    pub yank_message: Option<String>,
    /// Enter pressed on a link in Normal mode — the host resolves and
    /// opens it (or yanks a `mailto:`/`tel:` contact value).
    pub open_cursor_link: Option<MarkdownCursorLink>,
    /// `/` typed at a block start in Insert mode — open the block
    /// template menu, anchored at `open_block_menu_at` when `Some`.
    pub open_block_menu: bool,
    pub open_block_menu_at: Option<[f32; 4]>,
    /// `/` (`Some(false)`) or `?` (`Some(true)`) in Normal mode — the
    /// pane's incsearch has been armed; open the host search modal.
    pub open_search: Option<bool>,
    /// Plain `<Space>` in Normal mode — the host arms its leader timer.
    pub arm_leader: bool,
    /// Plain `:` in Normal mode — open the command palette.
    pub open_palette: bool,
    /// A committed title edit awaits the host's file rename.
    pub title_rename: Option<String>,
    /// The frontmatter value picker accepted a row; `(doc path, fresh
    /// `icon:` value)` for hosts that mirror icons into a sidebar.
    pub value_picker_icon: Option<(PathBuf, Option<String>)>,
    /// Post-dispatch finalize: refresh the block + link-completion
    /// menus from the new cursor context.
    pub refresh_menus: bool,
    /// Post-dispatch finalize: re-sync the buffer-tab modified dot.
    pub sync_modified: bool,
}

// ---------------------------------------------------------------------------
// Vim feed application (host-clipboard-free port of the desktop's
// `Screen::apply_markdown_vim_feed`)
// ---------------------------------------------------------------------------

/// Outcome of applying one resolved vim key feed to the pane.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct MarkdownVimFeedOutcome {
    pub handled: bool,
    pub snap_cursor: bool,
    /// Text the host should write into the system clipboard.
    pub clipboard_out: Option<String>,
    /// "Yanked N lines" notification text.
    pub yank_message: Option<String>,
}

/// Human message for a yank register payload — `"Yanked 3 lines"`.
pub fn markdown_yank_message(text: &str) -> String {
    let count = if text.is_empty() {
        0
    } else {
        text.split('\n').count() - usize::from(text.ends_with('\n'))
    }
    .max(1);
    let unit = if count == 1 { "line" } else { "lines" };
    format!("Yanked {count} {unit}")
}

/// Apply a resolved vim key feed to the pane, routing register traffic
/// through the returned outcome instead of a host clipboard handle.
/// `paste` is the host's current unnamed-register/system-clipboard text
/// (used only when the action wants a paste).
pub fn apply_markdown_vim_feed(
    pane: &mut MarkdownPane,
    paste: Option<&str>,
    feed: VimKeyFeed,
) -> MarkdownVimFeedOutcome {
    match feed {
        VimKeyFeed::Pending | VimKeyFeed::Cancelled => MarkdownVimFeedOutcome {
            handled: true,
            ..Default::default()
        },
        VimKeyFeed::Unhandled => MarkdownVimFeedOutcome::default(),
        VimKeyFeed::Action(action) => {
            let paste_text = if action.wants_paste() { paste } else { None };
            let applied = pane.apply_vim_action(&action, paste_text);
            let mut outcome = MarkdownVimFeedOutcome {
                handled: applied.handled,
                snap_cursor: applied.snap_cursor,
                ..Default::default()
            };
            if let Some(register) = applied.register {
                if applied.yank_notification {
                    outcome.yank_message = Some(markdown_yank_message(&register));
                }
                if applied.sync_clipboard {
                    outcome.clipboard_out = Some(if outcome.yank_message.is_some() {
                        rendered_inline_text(&register)
                    } else {
                        register
                    });
                }
            }
            // Macro replay: feed chars while replaying (same recursion
            // the desktop uses; nested register traffic keeps the LAST
            // clipboard payload, matching desktop's sequential sets).
            if let Some(keys) = applied.replay_keys {
                pane.vim.replaying_macro = true;
                for ch in keys.chars() {
                    if !matches!(pane.mode, MarkdownMode::Normal | MarkdownMode::Visual) {
                        break;
                    }
                    let visual = matches!(pane.mode, MarkdownMode::Visual);
                    let feed = pane.vim.feed(ch, visual);
                    let nested = apply_markdown_vim_feed(pane, paste, feed);
                    if nested.clipboard_out.is_some() {
                        outcome.clipboard_out = nested.clipboard_out;
                    }
                }
                pane.vim.replaying_macro = false;
            }
            outcome
        }
    }
}

// ---------------------------------------------------------------------------
// The dispatcher
// ---------------------------------------------------------------------------

fn single_char(s: &str) -> Option<char> {
    let mut chars = s.chars();
    match (chars.next(), chars.next()) {
        (Some(ch), None) => Some(ch),
        _ => None,
    }
}

/// Dispatch one key press into the markdown pane. `text` is the
/// printable text produced by the key ("" when none); `paste` is the
/// host clipboard/unnamed-register content for `p`/`P`; the host has
/// already run its leader chord machinery and passes `flushed_leader`.
pub fn dispatch_markdown_pane_key(
    pane: &mut MarkdownPane,
    key: MarkdownDispatchKey<'_>,
    text: &str,
    mods: MarkdownBridgeModifiers,
    viewport: f32,
    paste: Option<&str>,
    flushed_leader: bool,
) -> MarkdownDispatchEffects {
    use MarkdownDispatchKey as K;
    use MarkdownNamedKey as N;

    let mut fx = MarkdownDispatchEffects::default();
    let classes = mods.classify();
    let plain = classes.plain;
    let ctrl_only = classes.ctrl_only;
    let viewport = viewport.max(1.0);

    // ------ virtual title line ---------------------------------------
    // Normal-mode commands that move through the document leave the
    // title line first, then fall through to the regular dispatcher.
    let title_normal =
        pane.title_edit.is_some() && matches!(pane.mode, MarkdownMode::Normal);
    if title_normal {
        let exits_title = ctrl_only
            || matches!(key, K::Named(N::PageUp) | K::Named(N::PageDown))
            || (plain && matches!(key, K::Char(":") | K::Char("/") | K::Char("?")));
        if exits_title {
            pane.cancel_title_edit();
        }
    }
    if pane.title_edit.is_some() {
        let insert_mode = matches!(pane.mode, MarkdownMode::Insert);
        let mut title_handled = true;
        match key {
            K::Named(N::Enter) => pane.commit_title_edit(),
            K::Named(N::Escape) => {
                if insert_mode {
                    pane.enter_normal();
                    pane.title_edit_move(-1);
                } else {
                    pane.cancel_title_edit();
                }
            }
            K::Named(N::ArrowDown) => pane.cancel_title_edit(),
            K::Named(N::Backspace) => {
                if insert_mode {
                    pane.title_edit_backspace();
                } else {
                    pane.title_edit_move(-1);
                }
            }
            K::Named(N::Delete) => pane.title_edit_delete(),
            K::Named(N::ArrowLeft) => pane.title_edit_move(-1),
            K::Named(N::ArrowRight) => pane.title_edit_move(1),
            K::Named(N::Home) => pane.title_edit_home(),
            K::Named(N::End) => pane.title_edit_end(),
            _ if !insert_mode => {
                if plain {
                    match key {
                        K::Char("h") => pane.title_edit_move(-1),
                        K::Char("l") => pane.title_edit_move(1),
                        K::Char("0") | K::Char("^") => pane.title_edit_home(),
                        K::Char("$") => pane.title_edit_end(),
                        K::Char("i") => pane.enter_insert(),
                        K::Char("a") => {
                            pane.title_edit_move(1);
                            pane.enter_insert();
                        }
                        K::Char("I") => {
                            pane.title_edit_home();
                            pane.enter_insert();
                        }
                        K::Char("A") => {
                            pane.title_edit_end();
                            pane.enter_insert();
                        }
                        K::Char("x") => pane.title_edit_delete(),
                        K::Char("j") => pane.cancel_title_edit(),
                        _ => {}
                    }
                }
                // Swallow plain keys so they can't leak into the buffer.
                title_handled = plain;
            }
            _ => {
                let mut inserted = false;
                if plain && !text.is_empty() && text.chars().all(|c| !c.is_control()) {
                    pane.title_edit_insert(text);
                    inserted = true;
                }
                title_handled = inserted || plain;
            }
        }
        if title_handled {
            fx.handled = true;
            fx.title_rename = pane.take_pending_title_rename();
            return fx;
        }
    } else if plain
        && pane.cursor_line == 0
        && (matches!(key, K::Named(N::ArrowUp))
            || (matches!(pane.mode, MarkdownMode::Normal) && matches!(key, K::Char("k"))))
    {
        pane.begin_title_edit();
        fx.handled = true;
        return fx;
    }

    // ------ frontmatter value picker ---------------------------------
    if plain && pane.value_picker.is_some() {
        let mut picker_handled = false;
        match key {
            K::Named(N::ArrowDown) | K::Named(N::Tab) => {
                pane.value_picker_move(1);
                picker_handled = true;
            }
            K::Named(N::Enter) => {
                picker_handled = pane.value_picker_accept();
                if picker_handled {
                    fx.value_picker_icon =
                        Some((pane.path.clone(), pane.frontmatter_property("icon")));
                }
            }
            K::Named(N::ArrowUp) => {
                pane.value_picker_move(-1);
                picker_handled = true;
            }
            _ => {}
        }
        if picker_handled {
            fx.handled = true;
            return fx;
        }
    }

    // ------ `:` opens the command palette ----------------------------
    if markdown_normal_colon_opens_palette(
        Some(pane.mode),
        classes,
        matches!(key, K::Char(":")),
    ) {
        pane.vim.clear_pending();
        fx.handled = true;
        fx.open_palette = true;
        return fx;
    }

    // ------ Super+Z / Shift+Super+Z ----------------------------------
    let is_z = matches!(key, K::Char(ch) if ch.eq_ignore_ascii_case("z"));
    if let Some(redo) = markdown_super_z_intent(classes, is_z, mods.shift) {
        fx.handled = if redo { pane.redo() } else { pane.undo() };
        if fx.handled {
            fx.snap_cursor = true;
            fx.sync_modified = true;
        }
        return fx;
    }

    // ------ leader flushed → page scroll -----------------------------
    if markdown_flushed_leader_scrolls_normal_mode(Some(pane.mode), flushed_leader) {
        pane.scroll_by_content_pixels(viewport * 0.86, viewport);
    }

    let mut handled = true;
    let mut snap_cursor = false;

    // ------ Ctrl-only bindings ---------------------------------------
    if ctrl_only {
        let kind = match key {
            K::Char(ch) if ch.eq_ignore_ascii_case("d") => {
                Some(MarkdownCtrlKeyKind::CharD)
            }
            K::Char(ch) if ch.eq_ignore_ascii_case("u") => {
                Some(MarkdownCtrlKeyKind::CharU)
            }
            K::Char(ch) if ch.eq_ignore_ascii_case("e") => {
                Some(MarkdownCtrlKeyKind::CharE)
            }
            K::Char(ch) if ch.eq_ignore_ascii_case("y") => {
                Some(MarkdownCtrlKeyKind::CharY)
            }
            K::Char(ch) if ch.eq_ignore_ascii_case("r") => {
                Some(MarkdownCtrlKeyKind::CharR)
            }
            K::Char(ch) if ch.eq_ignore_ascii_case("v") => {
                Some(MarkdownCtrlKeyKind::CharV)
            }
            K::Char(ch) if ch.eq_ignore_ascii_case("o") => {
                Some(MarkdownCtrlKeyKind::CharO)
            }
            K::Char(ch) if ch.eq_ignore_ascii_case("i") => {
                Some(MarkdownCtrlKeyKind::CharI)
            }
            K::Named(N::Tab) => Some(MarkdownCtrlKeyKind::CharI),
            K::Named(N::ArrowUp) => Some(MarkdownCtrlKeyKind::ArrowUp),
            K::Named(N::ArrowDown) => Some(MarkdownCtrlKeyKind::ArrowDown),
            K::Named(N::ArrowLeft) => Some(MarkdownCtrlKeyKind::ArrowLeft),
            K::Named(N::ArrowRight) => Some(MarkdownCtrlKeyKind::ArrowRight),
            _ => None,
        };
        let action = kind.and_then(|kind| markdown_ctrl_action(classes, kind));
        match action {
            Some(MarkdownCtrlAction::ScrollCursorDownHalfPage) => {
                pane.scroll_cursor_by_content_pixels(viewport * 0.5, viewport);
            }
            Some(MarkdownCtrlAction::ScrollCursorUpHalfPage) => {
                pane.scroll_cursor_by_content_pixels(-(viewport * 0.5), viewport);
            }
            Some(MarkdownCtrlAction::ScrollCursorDownLine) => {
                pane.scroll_cursor_by_lines(1, viewport);
            }
            Some(MarkdownCtrlAction::ScrollCursorUpLine) => {
                pane.scroll_cursor_by_lines(-1, viewport);
            }
            Some(MarkdownCtrlAction::MoveTableRowUp) => {
                handled = pane.move_table_row_fast(false);
                snap_cursor = handled;
            }
            Some(MarkdownCtrlAction::MoveTableRowDown) => {
                handled = pane.move_table_row_fast(true);
                snap_cursor = handled;
            }
            Some(MarkdownCtrlAction::MoveTableCellPrev) => {
                handled = pane.move_table_cell(true);
                snap_cursor = handled;
            }
            Some(MarkdownCtrlAction::MoveTableCellNext) => {
                handled = pane.move_table_cell(false);
                snap_cursor = handled;
            }
            Some(MarkdownCtrlAction::Redo) => {
                let visual = matches!(pane.mode, MarkdownMode::Visual);
                let feed = pane.vim.feed_ctrl('r', visual);
                let outcome = apply_markdown_vim_feed(pane, paste, feed);
                handled = outcome.handled;
                snap_cursor = outcome.snap_cursor;
                fx.clipboard_out = outcome.clipboard_out;
                fx.yank_message = outcome.yank_message;
                if handled {
                    snap_cursor = true;
                    fx.sync_modified = true;
                }
            }
            Some(MarkdownCtrlAction::VimBlockVisual) => {
                let visual = matches!(pane.mode, MarkdownMode::Visual);
                let feed = pane.vim.feed_ctrl('v', visual);
                let outcome = apply_markdown_vim_feed(pane, paste, feed);
                handled = outcome.handled;
                snap_cursor = outcome.snap_cursor;
                fx.clipboard_out = outcome.clipboard_out;
                fx.yank_message = outcome.yank_message;
            }
            Some(MarkdownCtrlAction::VimJumpBack) => {
                let feed = pane.vim.feed_ctrl('o', false);
                let outcome = apply_markdown_vim_feed(pane, paste, feed);
                handled = outcome.handled;
                snap_cursor = outcome.snap_cursor;
            }
            Some(MarkdownCtrlAction::VimJumpForward) => {
                let feed = pane.vim.feed_ctrl('i', false);
                let outcome = apply_markdown_vim_feed(pane, paste, feed);
                handled = outcome.handled;
                snap_cursor = outcome.snap_cursor;
            }
            None => handled = false,
        }
        fx.handled = handled;
        fx.snap_cursor |= snap_cursor;
        return fx;
    }

    // ------ per-mode dispatch ----------------------------------------
    match pane.mode {
        MarkdownMode::Insert => match key {
            K::Named(N::Escape) => {
                pane.enter_normal();
                snap_cursor = true;
            }
            K::Named(N::Enter) => {
                if !(mods.shift && pane.insert_table_row(false)) {
                    pane.insert_newline();
                }
                snap_cursor = true;
            }
            K::Named(N::Backspace) => {
                pane.backspace();
                snap_cursor = true;
            }
            K::Named(N::Delete) => {
                pane.delete_forward();
                snap_cursor = true;
            }
            K::Named(N::Tab) if plain => {
                if pane.move_table_cell(mods.shift) {
                    snap_cursor = true;
                } else if pane.indent_list_item(mods.shift) {
                    snap_cursor = true;
                } else if !mods.shift {
                    pane.insert_text("  ");
                    snap_cursor = true;
                } else {
                    handled = false;
                }
            }
            K::Named(N::ArrowLeft) => pane.move_left(),
            K::Named(N::ArrowRight) => pane.move_right(),
            K::Named(N::ArrowUp) => pane.move_up(),
            K::Named(N::ArrowDown) => pane.move_down(),
            K::Named(N::Home) => pane.move_line_start(),
            K::Named(N::End) => pane.move_line_end(),
            K::Named(N::Space) if plain => {
                pane.insert_text(" ");
                snap_cursor = true;
            }
            K::Char("/") if plain => {
                // Inside a wiki link (`[[…]]`) a slash is part of the
                // path being typed — the link-completion menu owns the
                // popup there, not the `/` block menu.
                let in_wiki_link = pane.wiki_link_query_before_cursor().is_some();
                pane.insert_text("/");
                snap_cursor = true;
                if !in_wiki_link {
                    fx.open_block_menu = true;
                    fx.open_block_menu_at = pane.cursor_rect;
                }
            }
            _ if plain && !text.is_empty() => {
                pane.insert_text(text);
                snap_cursor = true;
            }
            _ => handled = false,
        },
        MarkdownMode::Normal => match key {
            K::Named(N::Escape) => {
                handled = pane.vim.clear_pending();
            }
            K::Named(N::ArrowLeft) => pane.move_left(),
            K::Named(N::ArrowRight) => pane.move_right(),
            K::Named(N::ArrowUp) => pane.move_up(),
            K::Named(N::ArrowDown) => pane.move_down(),
            K::Named(N::Home) => pane.move_line_start(),
            K::Named(N::End) => pane.move_line_end(),
            K::Named(N::Tab) if plain => {
                if pane.move_table_cell(mods.shift) || pane.indent_list_item(mods.shift) {
                    snap_cursor = true;
                } else if !mods.shift {
                    pane.insert_text("  ");
                    snap_cursor = true;
                } else {
                    handled = false;
                }
            }
            K::Named(N::Enter) if mods.shift => {
                handled = pane.insert_table_row(false);
                snap_cursor = handled;
            }
            // Links take precedence; task rows retain their keyboard
            // checkbox toggle when no link is under the cursor.
            K::Named(N::Enter) if plain => {
                fx.open_cursor_link = pane.link_at_cursor();
                handled = fx.open_cursor_link.is_some() || pane.toggle_task_at_cursor();
            }
            K::Named(N::PageUp) => {
                pane.scroll_by_content_pixels(-(viewport * 0.86), viewport)
            }
            K::Named(N::PageDown) => {
                pane.scroll_by_content_pixels(viewport * 0.86, viewport)
            }
            K::Named(N::Space) if mods.shift => {
                pane.scroll_by_content_pixels(-(viewport * 0.86), viewport)
            }
            K::Named(N::Space) => {
                fx.arm_leader = true;
            }
            // `/` and `?` arm incsearch; the host opens its search UI.
            K::Char("/") if plain => {
                pane.search_begin(false);
                fx.open_search = Some(false);
            }
            K::Char("?") if plain => {
                pane.search_begin(true);
                fx.open_search = Some(true);
            }
            K::Char(ch) if plain => {
                if let Some(ch) = single_char(ch) {
                    let feed = pane.vim.feed(ch, false);
                    let outcome = apply_markdown_vim_feed(pane, paste, feed);
                    handled = outcome.handled;
                    snap_cursor |= outcome.snap_cursor;
                    if outcome.clipboard_out.is_some() {
                        fx.clipboard_out = outcome.clipboard_out;
                    }
                    if outcome.yank_message.is_some() {
                        fx.yank_message = outcome.yank_message;
                    }
                } else {
                    handled = false;
                }
            }
            _ => handled = false,
        },
        MarkdownMode::Visual => match key {
            K::Named(N::Escape) => {
                pane.enter_normal();
                snap_cursor = true;
            }
            K::Named(N::ArrowLeft) => pane.move_left(),
            K::Named(N::ArrowRight) => pane.move_right(),
            K::Named(N::ArrowUp) => pane.move_up(),
            K::Named(N::ArrowDown) => pane.move_down(),
            K::Named(N::Home) => pane.move_line_start(),
            K::Named(N::End) => pane.move_line_end(),
            K::Named(N::Delete) | K::Named(N::Backspace) => {
                let feed = VimKeyFeed::Action(VimAction::Operate {
                    op: VimOperator::Delete,
                    target: VimTarget::Selection,
                    count: 1,
                });
                let outcome = apply_markdown_vim_feed(pane, paste, feed);
                handled = outcome.handled;
                snap_cursor |= outcome.snap_cursor;
                fx.clipboard_out = outcome.clipboard_out;
            }
            K::Char(ch) if plain => {
                if let Some(ch) = single_char(ch) {
                    let feed = pane.vim.feed(ch, true);
                    let outcome = apply_markdown_vim_feed(pane, paste, feed);
                    handled = outcome.handled;
                    snap_cursor |= outcome.snap_cursor;
                    if outcome.clipboard_out.is_some() {
                        fx.clipboard_out = outcome.clipboard_out;
                    }
                    if outcome.yank_message.is_some() {
                        fx.yank_message = outcome.yank_message;
                    }
                } else {
                    handled = false;
                }
            }
            _ => handled = false,
        },
    }

    fx.handled = handled;
    fx.snap_cursor |= snap_cursor;
    if let Some(finalize) =
        markdown_dispatch_finalize(handled, flushed_leader, fx.snap_cursor)
    {
        fx.refresh_menus = finalize.refresh_menus;
        fx.snap_cursor |= finalize.reset_trail_cursor;
        fx.sync_modified |= finalize.sync_active_modified;
    }
    fx
}

// ---------------------------------------------------------------------------
// Wiki-link completion ranking (path-list variant of the desktop's
// filesystem-scanning `markdown_link_suggestions`; hosts supply the
// candidate paths from whatever index they have)
// ---------------------------------------------------------------------------

/// Strip a trailing `-123` line suffix off a completion query (matches
/// the desktop's `markdown_link_match_query`).
pub fn markdown_link_match_query(query: &str) -> &str {
    let query = query.trim();
    if let Some((target, line)) = query.rsplit_once('-') {
        if !target.trim().is_empty() && line.chars().all(|ch| ch.is_ascii_digit()) {
            return target.trim();
        }
    }
    query
}

fn path_components_for_relative(path: &Path) -> Vec<String> {
    path.components()
        .filter_map(|component| match component {
            Component::Normal(part) => Some(part.to_string_lossy().into_owned()),
            Component::ParentDir => Some("..".to_string()),
            _ => None,
        })
        .collect()
}

/// `base_dir`-relative link target for `path` — `../notes/foo.md` style,
/// forward slashes. Port of the desktop's `relative_markdown_link_target`.
pub fn relative_markdown_link_target(base_dir: &Path, path: &Path) -> String {
    let from = path_components_for_relative(base_dir);
    let to = path_components_for_relative(path);
    let mut common = 0usize;
    while common < from.len() && common < to.len() && from[common] == to[common] {
        common += 1;
    }
    let mut parts = Vec::new();
    for _ in common..from.len() {
        parts.push("..".to_string());
    }
    parts.extend(to.into_iter().skip(common));
    if parts.is_empty() {
        path.file_name()
            .and_then(|name| name.to_str())
            .unwrap_or_default()
            .to_string()
    } else {
        parts.join("/")
    }
}

/// Rank wiki-link (`[[`) note suggestions from a host-supplied candidate
/// path list. Same scoring/limits as the desktop's filesystem scan.
pub fn markdown_link_suggestions_from_paths(
    candidates: &[PathBuf],
    base_dir: &Path,
    current_doc: &Path,
    query: &str,
) -> Vec<String> {
    const LIMIT: usize = 12;

    let query = markdown_link_match_query(query);
    let query_lower = query.to_ascii_lowercase();
    let mut scored = Vec::new();
    for path in candidates {
        if path.as_path() == current_doc {
            continue;
        }
        let target = relative_markdown_link_target(base_dir, path);
        if target.is_empty() {
            continue;
        }
        let target_lower = target.to_ascii_lowercase();
        let file_lower = path
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or(target.as_str())
            .to_ascii_lowercase();
        let score = if query_lower.is_empty() {
            20
        } else if file_lower.starts_with(&query_lower) {
            0
        } else if target_lower.starts_with(&query_lower) {
            2
        } else if file_lower.contains(&query_lower) {
            4
        } else if target_lower.contains(&query_lower) {
            6
        } else {
            continue;
        };
        scored.push((score, target.len(), target));
    }
    scored.sort_by(|a, b| a.0.cmp(&b.0).then(a.1.cmp(&b.1)).then(a.2.cmp(&b.2)));
    scored
        .into_iter()
        .take(LIMIT)
        .map(|(_, _, target)| target)
        .collect()
}

fn sanitize_markdown_note_segment(segment: &str) -> String {
    let mut out = String::new();
    let mut last_dash = false;
    for ch in segment.trim().chars() {
        if ch.is_ascii_alphanumeric() || matches!(ch, ' ' | '-' | '_' | '.') {
            out.push(ch);
            last_dash = false;
        } else if !last_dash {
            out.push('-');
            last_dash = true;
        }
    }
    out.trim_matches('-').trim().to_string()
}

/// The synthetic "create this note" completion target for a `[[` query
/// (port of the desktop's `markdown_create_note_target`).
pub fn markdown_create_note_target(query: &str) -> Option<String> {
    let query = query.trim().trim_matches('/');
    if query.is_empty()
        || query.starts_with('@')
        || query.starts_with('#')
        || query.contains("..")
    {
        return None;
    }
    let mut parts = Vec::new();
    for part in query.replace('\\', "/").split('/') {
        let sanitized = sanitize_markdown_note_segment(part);
        if sanitized.is_empty() || sanitized == "." || sanitized == ".." {
            return None;
        }
        parts.push(sanitized);
    }
    let last = parts.last_mut()?;
    let last_path = Path::new(last);
    if last_path.extension().is_none() {
        last.push_str(".md");
    }
    Some(parts.join("/"))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pane(source: &str) -> MarkdownPane {
        MarkdownPane::from_source(PathBuf::from("/tmp/test-dispatch.md"), source)
    }

    fn plain_mods() -> MarkdownBridgeModifiers {
        MarkdownBridgeModifiers::default()
    }

    fn feed_key(pane: &mut MarkdownPane, key: &str) -> MarkdownDispatchEffects {
        let parsed = parse_browser_markdown_key(key).expect("parsable key");
        let text = if key.chars().count() == 1 { key } else { "" };
        dispatch_markdown_pane_key(pane, parsed, text, plain_mods(), 600.0, None, false)
    }

    #[test]
    fn browser_key_parsing_covers_named_and_chars() {
        assert_eq!(
            parse_browser_markdown_key("Enter"),
            Some(MarkdownDispatchKey::Named(MarkdownNamedKey::Enter))
        );
        assert_eq!(
            parse_browser_markdown_key(" "),
            Some(MarkdownDispatchKey::Named(MarkdownNamedKey::Space))
        );
        assert_eq!(
            parse_browser_markdown_key("x"),
            Some(MarkdownDispatchKey::Char("x"))
        );
        assert_eq!(parse_browser_markdown_key("F5"), None);
        assert_eq!(parse_browser_markdown_key("Dead"), None);
    }

    #[test]
    fn dd_deletes_line_and_syncs_clipboard() {
        let mut pane = pane("# T\n\nalpha\nbeta\n");
        pane.mode = MarkdownMode::Normal;
        pane.cursor_line = 2;
        pane.cursor_col = 0;
        let fx = feed_key(&mut pane, "d");
        assert!(fx.handled); // pending operator
        let fx = feed_key(&mut pane, "d");
        assert!(fx.handled);
        assert!(!pane.lines.iter().any(|l| l == "alpha"));
        assert!(fx.clipboard_out.is_some());
    }

    #[test]
    fn yy_then_p_pastes_from_internal_register() {
        let mut pane = pane("# T\n\nalpha\n");
        pane.mode = MarkdownMode::Normal;
        pane.cursor_line = 2;
        let fx = feed_key(&mut pane, "y");
        assert!(fx.handled);
        let fx = feed_key(&mut pane, "y");
        assert!(fx.handled);
        assert!(fx.yank_message.is_some());
        let before = pane.lines.len();
        let fx = feed_key(&mut pane, "p");
        assert!(fx.handled);
        assert_eq!(pane.lines.len(), before + 1);
    }

    #[test]
    fn visual_selection_delete_via_v_then_backspace() {
        let mut pane = pane("# T\n\nalpha beta\n");
        pane.mode = MarkdownMode::Normal;
        pane.cursor_line = 2;
        pane.cursor_col = 0;
        let fx = feed_key(&mut pane, "v");
        assert!(fx.handled);
        assert_eq!(pane.mode, MarkdownMode::Visual);
        feed_key(&mut pane, "l");
        feed_key(&mut pane, "l");
        let fx = feed_key(&mut pane, "Backspace");
        assert!(fx.handled);
        assert_eq!(pane.mode, MarkdownMode::Normal);
        assert!(pane.lines[2].starts_with("ha beta"));
    }

    #[test]
    fn colon_opens_palette_in_normal_mode_only() {
        let mut pane = pane("# T\n\nalpha\n");
        pane.mode = MarkdownMode::Normal;
        pane.cursor_line = 2;
        let fx = feed_key(&mut pane, ":");
        assert!(fx.handled);
        assert!(fx.open_palette);

        pane.mode = MarkdownMode::Insert;
        let fx = feed_key(&mut pane, ":");
        assert!(fx.handled);
        assert!(!fx.open_palette);
        assert!(pane.lines[2].contains(':'));
    }

    #[test]
    fn slash_in_insert_opens_block_menu_outside_wiki_links() {
        let mut pane = pane("# T\n\n\n");
        pane.mode = MarkdownMode::Insert;
        pane.cursor_line = 2;
        pane.cursor_col = 0;
        let fx = feed_key(&mut pane, "/");
        assert!(fx.handled);
        assert!(fx.open_block_menu);

        // Inside `[[` the slash belongs to the query — no menu.
        let mut pane = self::tests::pane("# T\n\n[[notes\n");
        pane.mode = MarkdownMode::Insert;
        pane.cursor_line = 2;
        pane.cursor_col = "[[notes".len();
        let fx = feed_key(&mut pane, "/");
        assert!(fx.handled);
        assert!(!fx.open_block_menu);
    }

    #[test]
    fn slash_in_normal_arms_incsearch() {
        let mut pane = pane("# T\n\nalpha\n");
        pane.mode = MarkdownMode::Normal;
        pane.cursor_line = 2;
        let fx = feed_key(&mut pane, "/");
        assert!(fx.handled);
        assert_eq!(fx.open_search, Some(false));
        assert!(pane.search_active());

        let fx = feed_key(&mut pane, "?");
        assert_eq!(fx.open_search, Some(true));
    }

    #[test]
    fn space_arms_leader_in_normal_mode() {
        let mut pane = pane("# T\n\nalpha\n");
        pane.mode = MarkdownMode::Normal;
        pane.cursor_line = 2;
        let fx = feed_key(&mut pane, " ");
        assert!(fx.handled);
        assert!(fx.arm_leader);
    }

    #[test]
    fn enter_on_wiki_link_reports_open_intent() {
        let mut pane = pane("# T\n\nsee [[other-note]] here\n");
        pane.mode = MarkdownMode::Normal;
        pane.cursor_line = 2;
        pane.cursor_col = 6;
        let fx = feed_key(&mut pane, "Enter");
        assert!(fx.handled);
        assert!(matches!(
            fx.open_cursor_link,
            Some(MarkdownCursorLink::Internal { .. })
        ));
    }

    #[test]
    fn ctrl_r_redoes_after_undo() {
        let mut pane = pane("# T\n\nalpha\n");
        pane.mode = MarkdownMode::Normal;
        pane.cursor_line = 2;
        feed_key(&mut pane, "d");
        feed_key(&mut pane, "d");
        assert!(!pane.lines.iter().any(|l| l == "alpha"));
        let fx = feed_key(&mut pane, "u");
        assert!(fx.handled);
        assert!(pane.lines.iter().any(|l| l == "alpha"));
        let parsed = parse_browser_markdown_key("r").unwrap();
        let mods = MarkdownBridgeModifiers {
            control: true,
            ..Default::default()
        };
        let fx =
            dispatch_markdown_pane_key(&mut pane, parsed, "r", mods, 600.0, None, false);
        assert!(fx.handled);
        assert!(!pane.lines.iter().any(|l| l == "alpha"));
    }

    #[test]
    fn tab_indents_list_item_in_insert_mode() {
        let mut pane = pane("# T\n\n- one\n- two\n");
        pane.mode = MarkdownMode::Insert;
        pane.cursor_line = 3;
        pane.cursor_col = 5;
        let fx = feed_key(&mut pane, "Tab");
        assert!(fx.handled);
        assert!(pane.lines[3].starts_with("  - "));
        // Shift+Tab outdents again.
        let parsed = parse_browser_markdown_key("Tab").unwrap();
        let mods = MarkdownBridgeModifiers {
            shift: true,
            ..Default::default()
        };
        let fx =
            dispatch_markdown_pane_key(&mut pane, parsed, "", mods, 600.0, None, false);
        assert!(fx.handled);
        assert!(pane.lines[3].starts_with("- "));
    }

    #[test]
    fn arrow_up_at_top_enters_title_edit_and_typing_renames() {
        let mut pane = pane("# Title\n\nalpha\n");
        pane.mode = MarkdownMode::Normal;
        pane.cursor_line = 0;
        let fx = feed_key(&mut pane, "ArrowUp");
        assert!(fx.handled);
        assert!(pane.title_edit.is_some());
        // `i` → insert mode on the title, type, commit with Enter.
        feed_key(&mut pane, "i");
        feed_key(&mut pane, "X");
        let fx = feed_key(&mut pane, "Enter");
        assert!(fx.handled);
        assert!(fx.title_rename.is_some());
    }

    #[test]
    fn insert_typing_inserts_text() {
        let mut pane = pane("# T\n\nalpha\n");
        pane.mode = MarkdownMode::Insert;
        pane.cursor_line = 2;
        pane.cursor_col = 0;
        let fx = feed_key(&mut pane, "Z");
        assert!(fx.handled);
        assert!(fx.refresh_menus);
        assert!(pane.lines[2].starts_with('Z'));
    }

    #[test]
    fn create_note_target_sanitizes_and_appends_md() {
        assert_eq!(
            markdown_create_note_target("My Topic"),
            Some("My Topic.md".to_string())
        );
        assert_eq!(
            markdown_create_note_target("a/b"),
            Some("a/b.md".to_string())
        );
        assert_eq!(markdown_create_note_target("@code"), None);
        assert_eq!(markdown_create_note_target("../evil"), None);
    }

    #[test]
    fn link_suggestions_rank_filename_prefix_first() {
        let candidates = vec![
            PathBuf::from("/v/notes/alpha.md"),
            PathBuf::from("/v/other/nested/alpha-two.md"),
            PathBuf::from("/v/notes/beta.md"),
        ];
        let out = markdown_link_suggestions_from_paths(
            &candidates,
            Path::new("/v/notes"),
            Path::new("/v/notes/current.md"),
            "alp",
        );
        assert_eq!(out.first().map(String::as_str), Some("alpha.md"));
        assert!(out.iter().any(|t| t == "../other/nested/alpha-two.md"));
        assert!(!out.iter().any(|t| t == "beta.md"));
    }

    #[test]
    fn relative_target_walks_up_and_down() {
        assert_eq!(
            relative_markdown_link_target(
                Path::new("/v/notes/sub"),
                Path::new("/v/other/x.md")
            ),
            "../../other/x.md"
        );
        assert_eq!(
            relative_markdown_link_target(Path::new("/v"), Path::new("/v/x.md")),
            "x.md"
        );
    }
}
