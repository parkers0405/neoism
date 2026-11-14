# Global shortcuts

```rust,ignore
let app = App::new(spec)
    .global_shortcut("cmd+shift+space", |state, tx| {
        state.quick_panel_open = true;
        if let Some(token) = tx.activation_token() {
            state.app_handle.activate(token);
        } else {
            state.app_handle.open();
        }
    })
    .windowless();
app.run();
```

The handler receives the same state as its `App`/`WindowSpec`, on the app thread.
It runs through `Runtime::update`, including animation and shell requests. Native
activation wakes the loop even while no window is visible.

Static builder failures print once and can be drained with
`handle.take_shortcut_errors()`. The builder defers registration until the app's
host exists, so `run_windows_with` factories can construct apps normally.

Runtime registration returns the actual result:

```rust,ignore
let id = handle.register_shortcut("ctrl+alt+k".parse()?, |state, tx| {
    state.open_quick_panel(tx.activation_token());
})?;
handle.unregister_shortcut(id)?;
```

After a `Conflict`, `handle.shortcut_owner(&shortcut)` names the holder when Rune or
the desktop knows it: this App or another Rune App in the process (its `AppSpec`
title), or a macOS system shortcut ("Spotlight", "Mission Control", "Input Sources",
"Screenshot", "App Switcher", "macOS" for other enabled symbolic hotkeys), at the chord
the user remapped it to. The macOS lookup answers only on the main thread. A chord
another Rune App in the process holds is refused with `Conflict` on every backend. Other apps'
grabs, Windows, X11 and the portal name nobody, so it returns None. Headless runs
answer only for their own virtual registrations. `ShortcutError::Conflict` itself is
unchanged.

A chord needs ⌘, ⌃ or ⌥ (Ctrl, Alt or Super elsewhere), or an F1–F24 key, because a
global Space or ⇧A would take that key from typing in every app. Both the builder and
`register_shortcut` refuse one without them with `InvalidAccelerator`
(`shortcut.require_modifier()` checks it), unless the App opts in with
`.allow_unmodified_shortcuts()`.

Call runtime registration after the host starts (an `on_window` or queued
`handle.update` callback works, including one on another window of the same
`run_windows`). Calls before startup or after shutdown return `NotRunning`. On macOS/Windows other threads may call it, but wait for the app
thread, so the app thread must not join those callers. Linux registration can
run off-thread. Dropping the host unregisters everything,
even if other threads retain `AppHandle`s.

## Desktop behavior

- macOS: Carbon `RegisterEventHotKey`, no Accessibility permission. Command is
  the Super modifier. Hardware key positions follow the US layout; F21–F24 have
  no Carbon key code and return `InvalidAccelerator`.
- Windows: `RegisterHotKey` on a message-only window owned by the event thread.
  Repeat is disabled. Windows-reserved combinations may fail registration.
- X11: checked passive grabs on every root, including CapsLock/NumLock variants.
  A failed variant rolls back the registration. Autorepeat is filtered, so a held
  chord fires once, as on macOS and Windows. Keys use the current keymap's
  base keysyms; restart/re-register after changing the keymap.
- Wayland: xdg-desktop-portal GlobalShortcuts. No portal returns `Unsupported`.
  The portal may ask for permission or let the user choose another trigger;
  `preferred_trigger` is a preference, not an exclusive-grab guarantee. Cancellation
  is `PermissionDenied`, not `Conflict`. Each shortcut owns a portal session so
  unregister also works with version 1. The activation token is available only
  during its handler, through `Tx::activation_token()`.

Registration is synchronous, including portal consent, with a two-minute bound.
Register startup shortcuts before showing windows; use a worker thread for
runtime Wayland binding so a consent dialog does not block the event thread.

Only an actual registration creates native resources: macOS/Windows need no
listener thread; X11/Wayland each create a listener that is joined on teardown.
No-shortcut apps never open a shortcut display or portal connection.

## Deterministic testing

Headless scripts register virtually and invoke the same handlers:

```json
[{"t": 0, "shortcut": "cmd+shift+space"}]
```

Unknown/unregistered shortcuts fail the script. Parsing is case-insensitive;
Display normalizes modifiers to `ctrl+alt+shift+super`, then the lowercase key.
`cmd`, `command`, `meta`, and `logo` alias `super`; `option` aliases `alt`.

`scripts/check-global-shortcut-macos.py <shortcut_probe>` checks two-process
conflicts and real CuaDriver input on a GUI test Mac, under an exclusive GUI lock file.
The probe stays windowless and verifies handler thread identity.

## Shortcut recorder

`rune_kit::ShortcutRecorder` uses app-owned `ShortcutRecorderState` and renders the
existing `Keybind` component:

```rust,ignore
ShortcutRecorder::new("shortcut", &state.shortcut, |s: &mut State| &mut s.shortcut)
    .on_change(|state, shortcut, tx| state.replace_shortcut(shortcut, tx))
    .build(&kit)
```

The change callback returns `Result<(), ShortcutError>`. A registration failure
retains the old binding; the recorder keeps showing the refused chord with a message
until another modifier starts the next chord. Unchanged chords skip the callback,
unless `state.mark_unregistered()` says the shortcut isn't registered (it failed at
launch): then recording it again retries it through the callback. The
default message names the chord ("⌃J is already in use."); `.error_message(|error,
chord| ...)` rewords it, falling back on `rune_kit::shortcut_error_message`, and
`ShortcutRecorderState::refused()` reads the chord. A chord without a modifier (other
than F1–F24) is refused before the callback with "Add ⌘, ⌃ or ⌥." unless
`.allow_unmodified()`.
Click or keyboard activation starts recording, held modifiers preview immediately,
Escape cancels, Backspace clears, and focus loss or disabling cancels recording.
Repeated key presses are consumed without registering again.
`scripts/shortcut-recorder.json` records a chord headlessly.

`Shortcut` stores a `PhysicalKeyCode`. Parsing and Display use canonical US physical
names; `keycaps()` is for display only. On the macOS main thread, printable labels
use `TISCopyCurrentKeyboardLayoutInputSource` and `UCKeyTranslate` without changing
the input source. Other platforms, background callers and unavailable layouts use
US labels. Persist the canonical shortcut, not its translated keycaps.
`menu_keycaps()` is the label a menu shows: the key the accelerator names on every
layout (`cmd+b` is ⌘B on Dvorak, where `keycaps()` says ⌘X). Use it for menu items and
hints of keys an app matches by character, `keycaps()` for global shortcuts.

Physical input and its logical fallback are processed together in event order.
Focused capture runs before modal Escape, text editing and paste. A consumed chord
never reads the clipboard. Otherwise eligible Cmd-V requests the asynchronous
clipboard worker and consumes the command even for empty/image-only clipboards.
The captured paste target prevents a later focus change from receiving the reply.
Single-line and secure editors retain their ordinary editing and copy protections.
Key-taking launcher panels retain winit's input view/delegate and use the window
input path; overlays and Wayland layer surfaces also forward physical input.

## Native application and Dock menus

```rust,ignore
let app = App::new(spec)
    .menu_item(MenuItem::new("Open Quick Panel", open_panel).id("quick")
        .key_equivalent("cmd+shift+space".parse()?))
    .menu_item(MenuItem::new("Preferences", preferences).menu("Tools"))
    .menu_item(MenuItem::new("Open Quick Panel", open_panel).id("dock-quick").dock())
    .menu_item(MenuItem::new("New Window", new_window).menu("File"))
    .menu_item(MenuItem::separator().menu("File"))
    .menu_item(MenuItem::new("Close Tab", close_tab).menu("File"));

handle.set_menu([
    MenuItem::new("Open Quick Panel", open_panel).id("quick")
        .enabled(true).checked(true),
])?;
```

Actions are `fn(&mut S, &mut Tx)` queued on the app thread. `set_menu` replaces all
custom items for that App; an empty list removes them. It validates atomically and
queues installation. `take_menu_errors()` reports deferred startup/installation
errors. Replaced, disabled or removed items invalidate stale queued actions.
`MenuItem::separator()` is a separator line (`NSMenuItem.separatorItem` on macOS), placed
with `menu` or `dock` in list order like any item. It needs no id, takes no part in
id/title lookup and can't be fired.

On macOS, one App owns the process-wide custom menus. Default Quit/Hide items and
existing named-menu items are preserved. The Dock adapter adds an absent
`applicationDockMenu:` selector to winit's existing delegate class, retaining the
delegate and all existing methods. An existing non-Rune selector is an error; the
Rune hook returns nil after teardown. Key equivalents are native menu equivalents,
not global registrations. AppKit matches them by the character typed, so an item's
equivalent is the character its accelerator names: `cmd+b` is "b" with ⌘ on every
layout, `cmd+shift+=` is "=" with ⇧⌘, and named keys use AppKit's characters (↩ ⇥ ⎋ ⌫ ⌦,
arrows, F-keys). App menu, named menus, Dock and context menus all show the same key.
When two items in the menu bar share an equivalent, AppKit gives it to the first and
the later one shows none, so one key never runs two items. Rune never sets the
function-key flag; the 🌐F in a menu titled View is AppKit's own Enter Full Screen.

An accessory app that calls `set_dock_icon(true)` gets winit's standard app menu
if `NSApp.mainMenu` is absent: About, Services, Hide, Hide Others, Show All and Quit.
Creating custom menu items first uses that same default root; adding them later
inserts alongside the defaults. Existing roots are never replaced, and clearing
custom items or calling `set_dock_icon(false)` leaves the default menu intact.
A regular app whose menu bar is still winit's default also gets the standard items
below; an accessory app gets them only with `standard_menus()`.

### Standard menus

`App::standard_menus()` adds an Edit menu (Undo ⌘Z, Redo ⇧⌘Z, Cut ⌘X, Copy ⌘C, Paste
⌘V, Select All ⌘A) and a Window menu (Minimize ⌘M, Close ⌘W) to the process menu bar
when the first window of that App starts. A regular (non-accessory) app gets the same
items without asking, as long as its menu bar is still winit's default when its first
window starts; an app that set its own menu bar is left alone. Rune never adds Quit:
an accessory app that later shows its Dock icon gets winit's app menu (with Quit) as
before. Items go into existing menus of the same title, an item whose action is
already there is skipped, and custom `MenuItem`s keep working in either order. A
custom item with a standard item's equivalent (Close Tab ⌘W) takes it: the standard
item shows none until the custom one goes, whichever was set first. New custom menus
take the Mac order: File before Edit, the app's other menus before Window, Help last.
Clearing custom items keeps a menu the standard items share. An accessory app's menu
bar is hidden, but AppKit still matches its key equivalents.

Every item has a nil target, so AppKit sends it to the key window's first responder:

- An AppKit window in the process (an open panel, an updater's window) gets the
  normal `copy:`, `paste:`, `selectAll:`, `undo:`, `performMiniaturize:` and
  `performClose:` of its focused field or window.
- A Rune window's first responder is winit's view. AppKit consumes a matched key
  equivalent even when no responder implements its action, which is why a plain Edit
  menu would swallow ⌘C before Rune sees it. So the view gains the eight actions (added
  to winit's view class, never replacing a method it has), and each hands Rune the key:
  the very event that fired the item, so text fields, `on_key` and shortcuts see exactly
  the `Key::Command` they did without the menu, once. A click on the item sends a
  made-up press and release of its key instead.
- An IME composition (or a dead key's accent) in the focused field ends first, as in an
  NSTextView: the input method commits it into the field, then the command acts. One
  that doesn't commit is dropped, so the key still reaches Rune.
- ⌘W and ⌘M reach Rune the same way first. If no handler takes them and the window has
  that title-bar button, the window then closes or minimizes as a click on the button
  would: `on_close` and `keep_running` hiding apply. A window without the button
  (borderless, an overlay or nonactivating panel) only gets the key. An app that binds ⌘W itself keeps it.

Without `standard_menus`, an accessory app's keys don't change.

Native Windows/Linux return `MenuError::Unsupported`; startup builders report it
through `take_menu_errors()`. Headless virtual menus work on every platform:
`{"menu":"Open Quick Panel"}` invokes by exact id or unambiguous title. Unknown,
disabled and ambiguous items fail the script.

`scripts/check-native-menu-macos.py <menu_probe>` exercises the app-menu click,
key equivalent, named-menu click, runtime enabled/checked updates, preservation of
Quit/Hide and the Dock delegate selector on a GUI test Mac, under the same lock.
`scripts/check-dock-menu-macos.py <dock_menu_probe>` checks the accessory-to-Regular
default menu in both orders. `scripts/menu.json` exercises title/id dispatch headlessly.
