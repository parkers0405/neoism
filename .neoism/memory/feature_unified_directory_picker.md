---
name: "Unified in-app directory and picture picker"
description: "Modern enlarged in-app picker: colored tree icons, real address caret, single-select/double-open, bounded animations, direct+picture, pointer cursors; host-aware directory routing"
type: "feature"
scope: "project"
origin: "neoism-agent"
created: "2026-10-15"
updated: "2026-10-15"
---

# Unified in-app directory and picture picker

The user chose the existing in-app picker for local and joined workspaces, NOT OS-native dialogs. Current entry points: splash Change Directory, status cwd, and clickable cwd below agent composer open directory mode; composer `+` DIRECTLY opens picture mode. The earlier Attach picture / Change directory plus menu was explicitly rejected and removed. Keep workspace-root changes separate from agent session/draft cwd changes.

Shared modal: `neoism-frontend/shared/src/panels/file_browser.rs`. Desktop cap960px wide with16useful32px rows (~664px tall), responsive/safe-area bounded and mobile path/filter reflow. Use `file_tree::icons::folder_icon(false)` and `icon_for_file` for identical glyph/color/look overrides to main file tree. Directory action text is exactly `Select`, not `Select folder`. Footer buttons have measured-width geometry, minimum128px accept/96px cancel where space permits, reserved padding, and font-fit fallback under narrow/scaled constraints.

Native picker interaction: no implicit first-row selection; single-click explicitly highlights a row and Select confirms the selected folder, or the current viewed folder if no row is selected. Double-click opens the same previously clicked/selected folder; guard against an animation/async update moving a different row under the pointer. Directory mode filters files. Confirmation requires successful listing, and host-side stat/list revalidation remains in controllers. Nested starting cwd is preserved within authorized roots, and prior host locations are cleared on open.

Top path field autofocuses with visible steady caret. Both address and filter have measured click-to-caret placement, scrolling, UTF8/grapheme-safe Left/Right/Home/End/Backspace/Delete, Shift selection and Ctrl/Meta+A. Enter in path navigates; invalid Enter preserves editing focus. TextCursor snaps to resulting grapheme boundaries after edits (combining marks/ZWJ/flag merges). Late location replies preserve dirty path/filter drafts AND cursor/selection even if path text has not changed or focus moved. Sidebar hits must enforce the same full-row visibility as painting.

Animations use `web_time::Instant`, not epoch f32: open180ms (card translation/scrim), directory list entry160ms (horizontal slide/fade after accepted listing), click160ms pulse/press. `is_animating()` is owned by desktop Renderer::redraw_reason in host/state.rs and shared Chrome::animations_active in chrome/content.rs. `pointer_kind_at(x,y)->widgets::modal::ModalPointerKind` is the authoritative picker cursor classifier. Desktop mouse modal priority forwards PointerMove, requests redraw, and maps default/text/pointer; status/footer/+ directory hits use pointer. WASM frame.rs exports pointer_cursor_at through JS adapter pointerCursorAt; TerminalPanel sets CSS cursor in normal and modal-overlay early-return hover paths and resets on leave, excluding touch/drag ownership.

Important desktop input contract: shared handle_key consumes caret/shortcut keys but printable commits arrive separately via UiEvent::Text. Native router/route.rs must forward BOTH logical KeyDescriptor and printable KeyEvent.text (fallback logical Character), otherwise typing silently does nothing. Ctrl/Meta+V reads clipboard and inserts via input_text. Ctrl/Meta shortcuts must not also insert their letters. Shared event-pair regression ensures commits insert once.

Desktop controller: desktop/src/screen/bridges/file_browser.rs. Captured purpose/host/workspace/root/pane/backend/session/draft identity plus modal generation; local FS only for local workspaces. Joined browsing exclusively Files Browser* protocol, directory BrowserStat -> BrowserListDir revalidation before commit, no guest fallback. Request IDs are daemon-connection-local. Discard stale navigation successes/errors, surface send errors/15s timeout; remote images check20MiB before/after read. Local recents are not imported into joined browsing. Shared composer_interactions now only owns clipped cwd rectangles/action_at; menu helpers/paint/keyboard state were deleted. Use action_at rather than cwd_rect alone because occlusions may split hits.

Web TerminalPanel captures adapter/client/connection generation/workspace/root/pane/session, queues every drained browser request and validates host stat/list before commit. fileBrowserHost.ts stores host-scoped recents and opaque lexical host parents. WASM rendered/file_browser.rs and wasm/src/agent_directory.rs support per-pane blank directory drafts without creating backend sessions, submitting/replacing composer text, or changing workspace. JS BufferTab.agentDraftDirectory survives reattach and does not leak into another blank tab; first CreateThread uses pane execution directory. Session/pane changes or pending session creation reject late selection.

Verification for latest pass: picker39 tests passed; composer footer3 passed; npm typecheck and full web suite passed; native cargo check --tests and wasm32 cargo check passed. Rustfmt picker check and git diff --check clean. No release build, live visual test, or two-machine validation. Preserve concurrent project-chat/navigator/wordmark worktree changes.
