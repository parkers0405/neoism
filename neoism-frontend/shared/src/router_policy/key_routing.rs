// Copyright (c) 2023-present, Raphael Amorim.
//
// This source code is licensed under the MIT license found in the
// LICENSE file in the root directory of this source tree.

//! Shared key-routing dispatch for the desktop/web router.
//!
//! Native `has_key_wait` is a thin executor over [`route_key_input`].
//! The dispatcher walks the overlay z-order (global shortcut → island
//! rename → diagnostics popup → git diff panel → universal modal →
//! finder → command palette → route-level decision) and returns a
//! [`KeyRouteAction`] describing exactly which side effect the native
//! shell should run. All overlay state mutations (palette query,
//! finder query, modal selection, etc.) happen inside this module so
//! the executor only owns winit-bound IO (clipboard, nvim bridge,
//! redraw scheduling).

use crate::panels::command_palette::actions::{PaletteAction, PaletteBufferTarget};
use crate::panels::command_palette::state::CommandPalette;
use crate::panels::diagnostics_popup::DiagnosticsPopup;
use crate::panels::finder::state::Finder;
use crate::panels::git_diff::GitDiffPanel;
use crate::widgets::island::{Island, IslandRenameKey};
use crate::widgets::modal::{ModalAction, UniversalModal};

use super::{
    classify_route_key, route_input_decision, RouteInputDecision, RouteKey, RouteScreen,
};

/// POD modifier state. Mirrors the winit `ModifiersState` flags the
/// dispatcher actually consults; keeping this as a bag of bools lets
/// web/wasm callers pass their own values without depending on winit.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct KeyRouteModifiers {
    pub control: bool,
    pub alt: bool,
    pub shift: bool,
    pub super_key: bool,
}

/// POD bag of everything the dispatcher needs to know about a single
/// key event. `is_*` flags collapse the winit `Key::Named` /
/// `Key::Character` variants the native side already pattern-matches
/// on, plus a couple of pre-classified hints (`enter`, `escape`,
/// `character_text`) so `classify_route_key` can reuse this struct.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct KeyRouteInput {
    /// True for `KeyEvent.state == Pressed`. Releases are mostly
    /// passed through with the same shape — overlays only act on
    /// presses, route-policy still consumes releases for assistant /
    /// confirm-quit modal correctness.
    pub pressed: bool,
    pub enter: bool,
    pub escape: bool,
    pub backspace: bool,
    pub tab: bool,
    pub arrow_up: bool,
    pub arrow_down: bool,
    pub page_up: bool,
    pub page_down: bool,
    /// `Key::Character(text)` payload, if any.
    pub character_text: Option<String>,
    /// `KeyEvent.text` (winit text with shift). Falls back used by
    /// palette / finder typing paths so the user sees the same char
    /// nvim would have received.
    pub text: Option<String>,
    /// `text_with_all_modifiers()` — preferred over `text` for palette
    /// typing so Ctrl-modified chars still register.
    pub text_with_all_modifiers: Option<String>,
    pub modifiers: KeyRouteModifiers,
}

impl KeyRouteInput {
    fn classified(&self) -> RouteKey {
        classify_route_key(self.enter, self.escape, self.character_text.as_deref())
    }

    fn effective_text(&self) -> Option<&str> {
        self.text_with_all_modifiers
            .as_deref()
            .or(self.text.as_deref())
    }
}

/// POD snapshot of state the dispatcher needs about the overall route
/// before it touches any of the `&mut` overlays. Used for the final
/// route-level decision (terminal vs welcome vs confirm-quit + the
/// assistant overlay short-circuit).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RouteContext {
    pub screen: RouteScreen,
    pub assistant_overlay_active: bool,
}

/// Result of one `route_key_input` call. The native executor matches
/// on this enum to drive winit-bound side effects (nvim bridge,
/// clipboard, screen-level open_* methods, route-path mutation).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum KeyRouteAction {
    /// Nothing matched. The native executor returns `false` from
    /// `has_key_wait` so winit / nvim see the key.
    PassThrough,
    /// Key consumed silently — no redraw, no side effect. Currently
    /// only used by the route-policy `Consume` branch (e.g. confirm-
    /// quit swallowing a release event).
    Consume,
    /// Key consumed, executor should request an overlay redraw and
    /// return `true`.
    ConsumeAndRedraw,
    /// Global shortcut handled by the native side already. If
    /// `clear_assistant` is set the executor also clears the
    /// assistant overlay before redrawing.
    GlobalShortcutHandled { clear_assistant: bool },
    /// Island color-picker consumed the key (after the native rename-
    /// key translation succeeded and `handle_rename_input` returned
    /// true). Executor redraws + returns true.
    IslandConsumed,
    /// Diagnostics popup Enter activated a line — jump editor to it
    /// (after the dispatcher closed the popup).
    JumpToDiagnosticLine(u64),
    /// Git diff panel Esc — executor closes the panel.
    CloseGitDiffPanel,
    /// Modal Esc / Enter / hint dispatched an action. Executor runs
    /// `screen.execute_modal_action(action)` then redraws.
    ExecuteModalAction(ModalAction),
    /// Finder Enter — executor runs `screen.open_finder_selection`.
    OpenFinderSelection,
    /// Palette ex command resolved to `ThemePicker` — executor opens
    /// the theme picker.
    OpenThemePicker,
    /// Palette ex command resolved to `Shaders`.
    OpenShaders,
    /// Palette ex Enter with a non-intercepted command — executor
    /// first tries `screen.try_intercept_ex_command`; if it returns
    /// false the executor dispatches it as a real `vim_run_ex_command`
    /// to nvim.
    TryInterceptThenDispatchEx(String),
    /// Palette search Enter with a buffer-match location — dispatch
    /// `vim_search_commit_command` with the specific (lnum, col).
    DispatchNvimSearchCommit {
        query: String,
        location: Option<(u64, u64)>,
    },
    /// Palette closed from search mode — dispatch
    /// `vim_search_clear_command` so nvim drops `hlsearch`.
    DispatchNvimSearchClear,
    /// Palette typing (or Backspace) in search mode — dispatch a fresh
    /// `vim_search_query_command` so the lua side rebuilds the
    /// buffer-match list.
    DispatchNvimSearchQuery(String),
    /// Palette nav in search mode highlighted a buffer match — preview
    /// it in the editor via `vim_search_preview_command`.
    DispatchNvimSearchPreview { lnum: u64, col: u64, query: String },
    /// Palette Enter on a buffer-list row — executor calls
    /// `screen.activate_palette_buffer(target)`.
    ActivatePaletteBuffer(PaletteBufferTarget),
    /// Palette fonts-mode Enter — copy the family name to clipboard.
    CopyFontToClipboard(String),
    /// Palette `ListBuffers` action — executor opens the workspace
    /// buffers picker (lives in workspace HTTP land).
    OpenWorkspaceBuffersPicker,
    /// Palette `ListFonts` action — executor reads
    /// `sugarloaf.font_family_names()` and calls
    /// `palette.enter_fonts_mode(fonts)` to swap the palette contents.
    EnterPaletteFontsMode,
    /// Palette generic action — executor calls
    /// `screen.execute_palette_action(action, clipboard)` after the
    /// dispatcher already closed the palette.
    ExecutePaletteAction(PaletteAction),
    /// Palette Enter with no matching action — fall back to running
    /// the typed query as an ex query through `run_palette_ex_query`.
    RunPaletteExQuery(String),
    /// Route-policy assistant-overlay dismissal — executor clears both
    /// `self.assistant` and `renderer.assistant`.
    DismissAssistantOverlay,
    /// Route-policy confirm-quit cancel — executor sets path to
    /// `Terminal`.
    CancelConfirmQuit,
    /// Route-policy confirm-quit accept — executor exits.
    AcceptConfirmQuit,
    /// Route-policy welcome Enter — executor creates the config file
    /// and sets path to `Terminal`. Returns `false` (key not consumed
    /// — matches the existing behaviour where the welcome path falls
    /// through to terminal handling after creating the config).
    CreateConfigAndEnterTerminal,
}

/// Bundled `&mut` to every overlay the dispatcher mutates. Keeping
/// these together makes the call signature one parameter instead of
/// six and matches how the native side already holds them as fields
/// of `renderer`.
pub struct OverlayContext<'a> {
    pub island: Option<&'a mut Island>,
    pub diagnostics_popup: &'a mut DiagnosticsPopup,
    pub git_diff_panel: &'a mut GitDiffPanel,
    pub modal: &'a mut UniversalModal,
    pub finder: &'a mut Finder,
    pub command_palette: &'a mut CommandPalette,
}

/// Outcome of an attempted handler. `Consumed` carries the resulting
/// action and signals the executor to stop walking the overlay chain.
enum HandlerResult {
    /// Handler doesn't apply (overlay not visible / state didn't match).
    NotApplicable,
    /// Handler applied and produced an action.
    Consumed(KeyRouteAction),
}

/// Main entry point. `global_shortcut_handled` is the result of the
/// native `screen.handle_app_global_shortcut(key_event)` call — the
/// dispatcher needs it because shortcut routing depends on winit's
/// `LogicalKey` plus modifier resolution that isn't fully shared yet.
/// `island_rename_key` is the pre-translated key for the island color
/// picker (`None` when the winit key doesn't map to an
/// `IslandRenameKey` variant).
pub fn route_key_input(
    input: &KeyRouteInput,
    route_ctx: RouteContext,
    overlays: OverlayContext<'_>,
    global_shortcut_handled: bool,
    island_rename_key: Option<IslandRenameKey>,
) -> KeyRouteAction {
    let OverlayContext {
        island,
        diagnostics_popup,
        git_diff_panel,
        modal,
        finder,
        command_palette,
    } = overlays;

    if global_shortcut_handled {
        return KeyRouteAction::GlobalShortcutHandled {
            clear_assistant: input.pressed,
        };
    }

    if let Some(island) = island {
        if island.is_color_picker_open() {
            let consumed = island_rename_key
                .map(|key| island.handle_rename_input(key))
                .unwrap_or(false);
            if consumed {
                return KeyRouteAction::IslandConsumed;
            }
        }
    }

    if let HandlerResult::Consumed(action) =
        handle_diagnostics_popup(input, diagnostics_popup)
    {
        return action;
    }

    if let HandlerResult::Consumed(action) = handle_git_diff_panel(input, git_diff_panel)
    {
        return action;
    }

    if let HandlerResult::Consumed(action) = handle_modal(input, modal) {
        return action;
    }

    if let HandlerResult::Consumed(action) = handle_finder(input, finder) {
        return action;
    }

    if let HandlerResult::Consumed(action) =
        handle_command_palette(input, command_palette)
    {
        return action;
    }

    handle_route_decision(input, route_ctx)
}

fn handle_diagnostics_popup(
    input: &KeyRouteInput,
    popup: &mut DiagnosticsPopup,
) -> HandlerResult {
    if !(popup.is_visible() && popup.is_interactive() && input.pressed) {
        return HandlerResult::NotApplicable;
    }

    if input.escape {
        popup.close();
        return HandlerResult::Consumed(KeyRouteAction::ConsumeAndRedraw);
    }
    if input.arrow_up {
        popup.move_up();
        return HandlerResult::Consumed(KeyRouteAction::ConsumeAndRedraw);
    }
    if input.arrow_down {
        popup.move_down();
        return HandlerResult::Consumed(KeyRouteAction::ConsumeAndRedraw);
    }
    if input.enter {
        let lnum = popup.selected_lnum();
        popup.close();
        return HandlerResult::Consumed(match lnum {
            Some(lnum) => KeyRouteAction::JumpToDiagnosticLine(lnum),
            None => KeyRouteAction::ConsumeAndRedraw,
        });
    }
    HandlerResult::NotApplicable
}

fn handle_git_diff_panel(
    input: &KeyRouteInput,
    panel: &mut GitDiffPanel,
) -> HandlerResult {
    if !(panel.is_visible() && input.pressed && input.escape) {
        return HandlerResult::NotApplicable;
    }
    HandlerResult::Consumed(KeyRouteAction::CloseGitDiffPanel)
}

fn handle_modal(input: &KeyRouteInput, modal: &mut UniversalModal) -> HandlerResult {
    if !modal.is_active() {
        return HandlerResult::NotApplicable;
    }

    let blocking = modal.is_blocking();

    if !input.pressed {
        // Blocking modals still claim non-press events so they don't
        // leak through to nvim.
        return if blocking {
            HandlerResult::Consumed(KeyRouteAction::Consume)
        } else {
            HandlerResult::NotApplicable
        };
    }

    let modal_has_input = modal.has_input();

    if input.escape {
        if let Some(action) = modal.escape_action() {
            return HandlerResult::Consumed(KeyRouteAction::ExecuteModalAction(action));
        }
        modal.close();
        return HandlerResult::Consumed(KeyRouteAction::ConsumeAndRedraw);
    }
    if input.backspace && blocking && modal_has_input {
        modal.pop_input();
        return HandlerResult::Consumed(KeyRouteAction::ConsumeAndRedraw);
    }
    if input.arrow_up && blocking {
        modal.move_selection_up();
        return HandlerResult::Consumed(KeyRouteAction::ConsumeAndRedraw);
    }
    if input.arrow_down && blocking {
        modal.move_selection_down();
        return HandlerResult::Consumed(KeyRouteAction::ConsumeAndRedraw);
    }
    if input.page_up && blocking {
        modal.scroll_body_page(false);
        return HandlerResult::Consumed(KeyRouteAction::ConsumeAndRedraw);
    }
    if input.page_down && blocking {
        modal.scroll_body_page(true);
        return HandlerResult::Consumed(KeyRouteAction::ConsumeAndRedraw);
    }
    if blocking && input.enter {
        if let Some(action) = modal.selected_action() {
            return HandlerResult::Consumed(KeyRouteAction::ExecuteModalAction(action));
        }
        return HandlerResult::Consumed(KeyRouteAction::ConsumeAndRedraw);
    }
    if blocking && !modal_has_input {
        if let Some(text) = &input.character_text {
            if let Some(action) = modal.action_for_hint(text) {
                return HandlerResult::Consumed(KeyRouteAction::ExecuteModalAction(
                    action,
                ));
            }
        }
    }
    if blocking && modal_has_input {
        let mods = input.modifiers;
        if !mods.control && !mods.alt && !mods.super_key {
            if let Some(text) = input.text.as_deref() {
                if !text.is_empty() && text.chars().all(|c| !c.is_control()) {
                    modal.push_input(text);
                    return HandlerResult::Consumed(KeyRouteAction::ConsumeAndRedraw);
                }
            }
        }
    }

    // Blocking modals claim every other key so they don't leak through.
    if blocking {
        HandlerResult::Consumed(KeyRouteAction::Consume)
    } else {
        HandlerResult::NotApplicable
    }
}

fn handle_finder(input: &KeyRouteInput, finder: &mut Finder) -> HandlerResult {
    if !finder.is_enabled() {
        return HandlerResult::NotApplicable;
    }
    if !input.pressed {
        // Finder swallows non-press events too so they don't bleed.
        return HandlerResult::Consumed(KeyRouteAction::Consume);
    }
    let mods = input.modifiers;
    if input.tab && mods.control && !mods.alt && !mods.super_key {
        finder.cycle_search_mode();
        return HandlerResult::Consumed(KeyRouteAction::ConsumeAndRedraw);
    }
    if input.escape {
        finder.close();
        return HandlerResult::Consumed(KeyRouteAction::ConsumeAndRedraw);
    }
    if input.arrow_up {
        finder.move_selection_up();
        return HandlerResult::Consumed(KeyRouteAction::ConsumeAndRedraw);
    }
    if input.arrow_down {
        // Visible-rows count is approximate — the renderer recomputes
        // its real value from the overlay height each frame, but for
        // navigation bookkeeping a generous default is fine.
        finder.move_selection_down(18);
        return HandlerResult::Consumed(KeyRouteAction::ConsumeAndRedraw);
    }
    if input.enter {
        return HandlerResult::Consumed(KeyRouteAction::OpenFinderSelection);
    }
    if input.backspace {
        let current = finder.query.clone();
        if !current.is_empty() {
            let mut chars = current.chars().collect::<Vec<_>>();
            chars.pop();
            finder.set_query(chars.into_iter().collect());
            return HandlerResult::Consumed(KeyRouteAction::ConsumeAndRedraw);
        }
        return HandlerResult::Consumed(KeyRouteAction::Consume);
    }
    if let Some(text) = input.effective_text() {
        if !text.is_empty() && text.chars().all(|c| !c.is_control()) {
            let current = finder.query.clone();
            finder.set_query(format!("{}{}", current, text));
            return HandlerResult::Consumed(KeyRouteAction::ConsumeAndRedraw);
        }
    }
    HandlerResult::Consumed(KeyRouteAction::Consume)
}

fn handle_command_palette(
    input: &KeyRouteInput,
    palette: &mut CommandPalette,
) -> HandlerResult {
    if !palette.is_enabled() {
        return HandlerResult::NotApplicable;
    }
    if !input.pressed {
        return HandlerResult::Consumed(KeyRouteAction::Consume);
    }

    if input.escape {
        if let Some(action) = palette.system_settings_back_action() {
            return HandlerResult::Consumed(KeyRouteAction::ExecutePaletteAction(action));
        }
        let was_search = palette.is_search_mode();
        palette.set_enabled(false);
        return HandlerResult::Consumed(if was_search {
            KeyRouteAction::DispatchNvimSearchClear
        } else {
            KeyRouteAction::ConsumeAndRedraw
        });
    }
    if input.arrow_up {
        palette.move_selection_up();
        return HandlerResult::Consumed(maybe_preview_palette_search(palette));
    }
    if input.arrow_down {
        palette.move_selection_down();
        return HandlerResult::Consumed(maybe_preview_palette_search(palette));
    }
    if input.tab
        && input.modifiers.control
        && !input.modifiers.alt
        && !input.modifiers.super_key
    {
        if let Some(action) = palette.category_switch_action() {
            return HandlerResult::Consumed(KeyRouteAction::ExecutePaletteAction(action));
        }
    }
    if input.tab {
        let was_search = palette.is_search_mode();
        let completed = palette.tab_complete();
        if completed && was_search {
            let query = palette.query.clone();
            return HandlerResult::Consumed(KeyRouteAction::DispatchNvimSearchQuery(
                query,
            ));
        }
        if !completed {
            palette.move_selection_down();
            return HandlerResult::Consumed(maybe_preview_palette_search(palette));
        }
        return HandlerResult::Consumed(KeyRouteAction::ConsumeAndRedraw);
    }
    if input.enter {
        return HandlerResult::Consumed(palette_enter(palette));
    }
    if input.backspace {
        let current_query = palette.query.clone();
        if current_query.is_empty() {
            if let Some(action) = palette.system_settings_back_action() {
                return HandlerResult::Consumed(KeyRouteAction::ExecutePaletteAction(action));
            }
            return HandlerResult::Consumed(KeyRouteAction::Consume);
        }
        let mut chars = current_query.chars().collect::<Vec<_>>();
        chars.pop();
        let new_query: String = chars.into_iter().collect();
        let was_search = palette.is_search_mode();
        palette.set_query(new_query.clone());
        return HandlerResult::Consumed(if was_search {
            KeyRouteAction::DispatchNvimSearchQuery(new_query)
        } else {
            KeyRouteAction::ConsumeAndRedraw
        });
    }
    if let Some(text) = input.effective_text() {
        if !text.is_empty() && text.chars().all(|c| !c.is_control()) {
            let current_query = palette.query.clone();
            let new_query = format!("{}{}", current_query, text);
            let was_search = palette.is_search_mode();
            palette.set_query(new_query.clone());
            return HandlerResult::Consumed(if was_search {
                KeyRouteAction::DispatchNvimSearchQuery(new_query)
            } else {
                KeyRouteAction::ConsumeAndRedraw
            });
        }
    }
    HandlerResult::Consumed(KeyRouteAction::Consume)
}

fn maybe_preview_palette_search(palette: &CommandPalette) -> KeyRouteAction {
    if let Some((lnum, col)) = palette.selected_buffer_match_location() {
        let query = palette.query.clone();
        KeyRouteAction::DispatchNvimSearchPreview { lnum, col, query }
    } else {
        KeyRouteAction::ConsumeAndRedraw
    }
}

fn palette_enter(palette: &mut CommandPalette) -> KeyRouteAction {
    let ex = palette.is_ex_mode();
    let search = palette.is_search_mode();

    if ex || search {
        // Search mode: if selected row is an actual buffer match
        // location, commit it before falling back to the generic
        // path.
        if search {
            let typed = palette.query.clone();
            let selected_location = palette.selected_buffer_match_location();
            if let Some(location) = selected_location {
                palette.set_enabled(false);
                if !typed.is_empty() {
                    palette.push_recent_search(typed.clone());
                    return KeyRouteAction::DispatchNvimSearchCommit {
                        query: typed,
                        location: Some(location),
                    };
                }
                return KeyRouteAction::ConsumeAndRedraw;
            }
        }
        // Search: empty query + selected recent dispatches that term.
        // Ex: no-arg query + selected suggestion dispatches the
        // canonical command name, so `lspinfo` / `lsp` + Enter runs
        // `LspInfo` instead of forwarding a lowercase command nvim
        // would reject.
        let typed = palette.query.clone();
        let selected_recent = if search && typed.is_empty() {
            palette.get_selected_search_term()
        } else {
            None
        };
        let selected_ex =
            if ex && !typed.trim().is_empty() && !typed.contains(char::is_whitespace) {
                palette.get_selected_ex_command()
            } else {
                None
            };
        let payload = selected_recent
            .or(selected_ex)
            .unwrap_or_else(|| typed.clone());
        palette.set_enabled(false);
        let ex_payload = payload.trim().to_string();
        if ex
            && (ex_payload.eq_ignore_ascii_case("ThemePicker")
                || ex_payload.eq_ignore_ascii_case("theme picker"))
        {
            return KeyRouteAction::OpenThemePicker;
        }
        if ex
            && (ex_payload.eq_ignore_ascii_case("Shaders")
                || ex_payload.eq_ignore_ascii_case("ShaderPicker")
                || ex_payload.eq_ignore_ascii_case("shader picker"))
        {
            return KeyRouteAction::OpenShaders;
        }
        if ex && !ex_payload.is_empty() {
            return KeyRouteAction::TryInterceptThenDispatchEx(ex_payload);
        }
        if search && !payload.is_empty() {
            palette.push_recent_search(payload.clone());
            return KeyRouteAction::DispatchNvimSearchCommit {
                query: payload,
                location: None,
            };
        }
        return KeyRouteAction::ConsumeAndRedraw;
    }

    // Snapshot what the palette wants to do FIRST.
    let selected_font = palette.get_selected_font();
    let selected_buffer = palette.get_selected_buffer_target();
    let selected_action = palette.get_selected_action();

    if let Some(target) = selected_buffer {
        palette.set_enabled(false);
        return KeyRouteAction::ActivatePaletteBuffer(target);
    }

    // Fonts mode Enter: copy the family name to clipboard.
    if let Some(font) = selected_font {
        palette.set_enabled(false);
        return KeyRouteAction::CopyFontToClipboard(font);
    }

    if palette.systems_mode() || palette.system_settings_id().is_some() {
        return selected_action.map(KeyRouteAction::ExecutePaletteAction)
            .unwrap_or(KeyRouteAction::ConsumeAndRedraw);
    }

    match selected_action {
        Some(PaletteAction::ListFonts) => {
            // Stays inside the palette — executor reads fonts then
            // calls `palette.enter_fonts_mode(...)`.
            KeyRouteAction::EnterPaletteFontsMode
        }
        Some(PaletteAction::ListBuffers) => KeyRouteAction::OpenWorkspaceBuffersPicker,
        Some(action) => {
            palette.set_enabled(false);
            KeyRouteAction::ExecutePaletteAction(action)
        }
        None => {
            let query = palette.query.clone();
            palette.set_enabled(false);
            KeyRouteAction::RunPaletteExQuery(query)
        }
    }
}

fn handle_route_decision(input: &KeyRouteInput, ctx: RouteContext) -> KeyRouteAction {
    let decision = route_input_decision(
        ctx.screen,
        ctx.assistant_overlay_active,
        input.pressed,
        &input.classified(),
    );
    match decision {
        RouteInputDecision::PassThrough => KeyRouteAction::PassThrough,
        RouteInputDecision::Consume => KeyRouteAction::Consume,
        RouteInputDecision::DismissAssistantOverlay => {
            KeyRouteAction::DismissAssistantOverlay
        }
        RouteInputDecision::CancelConfirmQuit => KeyRouteAction::CancelConfirmQuit,
        RouteInputDecision::AcceptConfirmQuit => KeyRouteAction::AcceptConfirmQuit,
        RouteInputDecision::CreateConfigAndEnterTerminal => {
            KeyRouteAction::CreateConfigAndEnterTerminal
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::widgets::modal::{ModalButton, ModalSpec};
    use std::path::PathBuf;

    fn key_input_pressed() -> KeyRouteInput {
        KeyRouteInput {
            pressed: true,
            enter: false,
            escape: false,
            backspace: false,
            tab: false,
            arrow_up: false,
            arrow_down: false,
            page_up: false,
            page_down: false,
            character_text: None,
            text: None,
            text_with_all_modifiers: None,
            modifiers: KeyRouteModifiers::default(),
        }
    }

    fn empty_overlays<'a>(
        diagnostics: &'a mut DiagnosticsPopup,
        git_diff: &'a mut GitDiffPanel,
        modal: &'a mut UniversalModal,
        finder: &'a mut Finder,
        palette: &'a mut CommandPalette,
    ) -> OverlayContext<'a> {
        OverlayContext {
            island: None,
            diagnostics_popup: diagnostics,
            git_diff_panel: git_diff,
            modal,
            finder,
            command_palette: palette,
        }
    }

    fn route_ctx_terminal() -> RouteContext {
        RouteContext {
            screen: RouteScreen::Terminal,
            assistant_overlay_active: false,
        }
    }

    struct Fixture {
        diagnostics: DiagnosticsPopup,
        git_diff: GitDiffPanel,
        modal: UniversalModal,
        finder: Finder,
        palette: CommandPalette,
    }

    impl Fixture {
        fn new() -> Self {
            Self {
                diagnostics: DiagnosticsPopup::new(),
                git_diff: GitDiffPanel::new(),
                modal: UniversalModal::new(),
                finder: Finder::new(),
                palette: CommandPalette::default(),
            }
        }

        fn dispatch(
            &mut self,
            input: &KeyRouteInput,
            ctx: RouteContext,
            global_shortcut: bool,
            island_key: Option<IslandRenameKey>,
        ) -> KeyRouteAction {
            route_key_input(
                input,
                ctx,
                empty_overlays(
                    &mut self.diagnostics,
                    &mut self.git_diff,
                    &mut self.modal,
                    &mut self.finder,
                    &mut self.palette,
                ),
                global_shortcut,
                island_key,
            )
        }
    }

    #[test]
    fn global_shortcut_short_circuits_with_clear_assistant_on_press() {
        let mut fx = Fixture::new();
        let action = fx.dispatch(&key_input_pressed(), route_ctx_terminal(), true, None);
        assert_eq!(
            action,
            KeyRouteAction::GlobalShortcutHandled {
                clear_assistant: true
            }
        );
    }

    #[test]
    fn global_shortcut_handled_on_release_does_not_request_clear() {
        let mut fx = Fixture::new();
        let mut input = key_input_pressed();
        input.pressed = false;
        let action = fx.dispatch(&input, route_ctx_terminal(), true, None);
        assert_eq!(
            action,
            KeyRouteAction::GlobalShortcutHandled {
                clear_assistant: false
            }
        );
    }

    #[test]
    fn palette_search_escape_dispatches_clear_and_closes_palette() {
        let mut fx = Fixture::new();
        fx.palette.set_enabled(true);
        fx.palette.enter_search_mode();
        let mut input = key_input_pressed();
        input.escape = true;
        let action = fx.dispatch(&input, route_ctx_terminal(), false, None);
        assert_eq!(action, KeyRouteAction::DispatchNvimSearchClear);
        assert!(!fx.palette.is_enabled());
    }

    #[test]
    fn palette_non_search_escape_just_consumes_and_redraws() {
        let mut fx = Fixture::new();
        fx.palette.set_enabled(true);
        let mut input = key_input_pressed();
        input.escape = true;
        let action = fx.dispatch(&input, route_ctx_terminal(), false, None);
        assert_eq!(action, KeyRouteAction::ConsumeAndRedraw);
        assert!(!fx.palette.is_enabled());
    }

    #[test]
    fn palette_typed_text_in_search_mode_dispatches_query() {
        let mut fx = Fixture::new();
        fx.palette.set_enabled(true);
        fx.palette.enter_search_mode();
        let mut input = key_input_pressed();
        input.text = Some("foo".into());
        let action = fx.dispatch(&input, route_ctx_terminal(), false, None);
        assert_eq!(
            action,
            KeyRouteAction::DispatchNvimSearchQuery("foo".into())
        );
        assert_eq!(fx.palette.query.as_str(), "foo");
    }

    #[test]
    fn palette_typed_text_in_command_mode_consumes_and_redraws() {
        let mut fx = Fixture::new();
        fx.palette.set_enabled(true);
        let mut input = key_input_pressed();
        input.text = Some("the".into());
        let action = fx.dispatch(&input, route_ctx_terminal(), false, None);
        assert_eq!(action, KeyRouteAction::ConsumeAndRedraw);
        assert_eq!(fx.palette.query.as_str(), "the");
    }

    #[test]
    fn palette_backspace_in_empty_query_consumes_silently() {
        let mut fx = Fixture::new();
        fx.palette.set_enabled(true);
        let mut input = key_input_pressed();
        input.backspace = true;
        let action = fx.dispatch(&input, route_ctx_terminal(), false, None);
        assert_eq!(action, KeyRouteAction::Consume);
    }

    #[test]
    fn finder_enter_returns_open_selection() {
        let mut fx = Fixture::new();
        fx.finder.open_files(PathBuf::from("/tmp"));
        let mut input = key_input_pressed();
        input.enter = true;
        let action = fx.dispatch(&input, route_ctx_terminal(), false, None);
        assert_eq!(action, KeyRouteAction::OpenFinderSelection);
    }

    #[test]
    fn finder_backspace_with_empty_query_consumes_silently() {
        let mut fx = Fixture::new();
        fx.finder.open_files(PathBuf::from("/tmp"));
        let mut input = key_input_pressed();
        input.backspace = true;
        let action = fx.dispatch(&input, route_ctx_terminal(), false, None);
        assert_eq!(action, KeyRouteAction::Consume);
    }

    #[test]
    fn finder_typed_text_appends_to_query_and_consumes() {
        let mut fx = Fixture::new();
        fx.finder.open_files(PathBuf::from("/tmp"));
        let mut input = key_input_pressed();
        input.text = Some("bar".into());
        let action = fx.dispatch(&input, route_ctx_terminal(), false, None);
        assert_eq!(action, KeyRouteAction::ConsumeAndRedraw);
        assert_eq!(fx.finder.query, "bar");
    }

    #[test]
    fn system_category_ctrl_tab_emits_intent_without_completing_query() {
        let mut palette = CommandPalette::default();
        palette.enter_systems_mode(Vec::new());
        palette.set_query("my computer".into());
        palette.set_query_cursor(2);
        let mut input = key_input_pressed();
        input.tab = true;
        input.modifiers.control = true;
        assert!(matches!(
            handle_command_palette(&input, &mut palette),
            HandlerResult::Consumed(KeyRouteAction::ExecutePaletteAction(
                PaletteAction::ShowServers
            ))
        ));
        assert_eq!(palette.query, "my computer");
        assert_eq!(palette.query_cursor(), 2);
        palette.enter_servers_mode(Vec::new());
        assert!(matches!(
            handle_command_palette(&input, &mut palette),
            HandlerResult::Consumed(KeyRouteAction::ExecutePaletteAction(
                PaletteAction::ShowSystems
            ))
        ));
        input.modifiers.control = false;
        assert!(matches!(
            handle_command_palette(&input, &mut palette),
            HandlerResult::Consumed(KeyRouteAction::ConsumeAndRedraw)
        ));
        assert_eq!(palette.query, "+ Add Server");
    }

    #[test]
    fn systems_enter_opens_details_and_settings_escape_backspace_emit_back() {
        use crate::panels::command_palette::{PaletteSystemEntry, SystemConnectionStatus};
        let mut palette = CommandPalette::default();
        let entry = PaletteSystemEntry { id: "remote".into(), name: "Remote".into(),
            address: "user@host".into(), local: false, enabled: false,
            status: SystemConnectionStatus::Disconnected };
        palette.enter_systems_mode(vec![entry.clone()]);
        let mut input = key_input_pressed();
        input.enter = true;
        assert!(matches!(handle_command_palette(&input, &mut palette),
            HandlerResult::Consumed(KeyRouteAction::ExecutePaletteAction(PaletteAction::ShowSystemSettings { id })) if id == "remote"));
        assert!(palette.is_enabled());
        palette.enter_system_settings_mode(entry);
        assert!(matches!(handle_command_palette(&input, &mut palette),
            HandlerResult::Consumed(KeyRouteAction::ExecutePaletteAction(PaletteAction::SetSystemEnabled { enabled: true, .. }))));
        assert!(palette.is_enabled());
        input.enter = false;
        input.escape = true;
        assert!(matches!(handle_command_palette(&input, &mut palette),
            HandlerResult::Consumed(KeyRouteAction::ExecutePaletteAction(PaletteAction::ShowSystems))));
        assert!(palette.is_enabled());
        input.escape = false;
        input.backspace = true;
        assert!(matches!(handle_command_palette(&input, &mut palette),
            HandlerResult::Consumed(KeyRouteAction::ExecutePaletteAction(PaletteAction::ShowSystems))));
        palette.set_query("abc".into());
        assert!(matches!(handle_command_palette(&input, &mut palette),
            HandlerResult::Consumed(KeyRouteAction::ExecutePaletteAction(PaletteAction::ShowSystems))));
        assert_eq!(palette.query, "", "system settings have no editable query");
    }

    #[test]
    fn system_settings_local_enter_never_dispatches_an_ex_query() {
        use crate::panels::command_palette::{PaletteSystemEntry, SystemConnectionStatus};
        let mut palette = CommandPalette::default();
        palette.enter_system_settings_mode(PaletteSystemEntry { id: "local".into(), name: "Local".into(),
            address: String::new(), local: true, enabled: true, status: SystemConnectionStatus::Connected });
        let mut input = key_input_pressed();
        input.enter = true;
        assert!(matches!(handle_command_palette(&input, &mut palette),
            HandlerResult::Consumed(KeyRouteAction::ConsumeAndRedraw)));
        assert!(palette.is_enabled());
    }

    #[test]
    fn finder_ctrl_tab_cycles_mode() {
        let mut fx = Fixture::new();
        fx.finder.open_files(PathBuf::from("/tmp"));
        let mut input = key_input_pressed();
        input.tab = true;
        input.modifiers.control = true;
        let action = fx.dispatch(&input, route_ctx_terminal(), false, None);
        assert_eq!(action, KeyRouteAction::ConsumeAndRedraw);
    }

    #[test]
    fn modal_blocking_escape_runs_action_when_available() {
        let mut fx = Fixture::new();
        fx.modal.open(ModalSpec {
            title: "T".into(),
            body: "B".into(),
            meta: String::new(),
            input: None,
            buttons: vec![ModalButton::new("OK", "Esc", ModalAction::Close)],
            busy: false,
            blocking: true,
        });
        let mut input = key_input_pressed();
        input.escape = true;
        let action = fx.dispatch(&input, route_ctx_terminal(), false, None);
        // `escape_action` only fires for a button whose hint is "Esc";
        // modals without one just close on Escape.
        assert_eq!(
            action,
            KeyRouteAction::ExecuteModalAction(ModalAction::Close)
        );
    }

    #[test]
    fn modal_blocking_swallows_release_events() {
        let mut fx = Fixture::new();
        fx.modal.open(ModalSpec {
            title: "T".into(),
            body: "B".into(),
            meta: String::new(),
            input: None,
            buttons: vec![ModalButton::new("OK", "o", ModalAction::Close)],
            busy: false,
            blocking: true,
        });
        let mut input = key_input_pressed();
        input.pressed = false;
        let action = fx.dispatch(&input, route_ctx_terminal(), false, None);
        assert_eq!(action, KeyRouteAction::Consume);
    }

    #[test]
    fn route_decision_falls_through_when_no_overlay_active() {
        let mut fx = Fixture::new();
        let action = fx.dispatch(&key_input_pressed(), route_ctx_terminal(), false, None);
        assert_eq!(action, KeyRouteAction::PassThrough);
    }

    #[test]
    fn welcome_route_enter_creates_config_and_enters_terminal() {
        let mut fx = Fixture::new();
        let mut input = key_input_pressed();
        input.enter = true;
        let action = fx.dispatch(
            &input,
            RouteContext {
                screen: RouteScreen::Welcome,
                assistant_overlay_active: false,
            },
            false,
            None,
        );
        assert_eq!(action, KeyRouteAction::CreateConfigAndEnterTerminal);
    }

    #[test]
    fn assistant_overlay_dismisses_on_enter_press() {
        let mut fx = Fixture::new();
        let mut input = key_input_pressed();
        input.enter = true;
        let action = fx.dispatch(
            &input,
            RouteContext {
                screen: RouteScreen::Welcome,
                assistant_overlay_active: true,
            },
            false,
            None,
        );
        assert_eq!(action, KeyRouteAction::DismissAssistantOverlay);
    }

    #[test]
    fn confirm_quit_yes_accepts() {
        let mut fx = Fixture::new();
        let mut input = key_input_pressed();
        input.character_text = Some("y".into());
        let action = fx.dispatch(
            &input,
            RouteContext {
                screen: RouteScreen::ConfirmQuit,
                assistant_overlay_active: false,
            },
            false,
            None,
        );
        assert_eq!(action, KeyRouteAction::AcceptConfirmQuit);
    }
}
