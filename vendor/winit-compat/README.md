# winit compatibility patches

`winit-0.30.13` is the crates.io release with two changes, used by the Rune frontend
(`rune/app/Cargo.toml`, `[patch.crates-io]`).

## Wayland: an unchanged IME cursor rectangle is not committed again

`src/platform_impl/linux/wayland/window/state.rs`: `WindowState::set_ime_cursor_area` remembers
the last rectangle it committed to the `zwp_text_input_v3` objects and returns early when asked
for the same one. The cache resets when the IME is enabled or disabled and when a text input
is registered, so a fresh input still gets its rectangle.

Why: every `zwp_text_input_v3.commit` is answered by a `done`, and winit turns each `done`
with no composition pending into an `Ime::Preedit("")` window event. Rune re-sends the focused
field's caret area after every frame it draws, and redraws on every IME event, so with a text
field focused (the agent composer) the window committed, heard `done`, re-ran its views and
drew a frame on every vblank, forever: ~1.5 ms of CPU per 8.3 ms at rest plus a present per
vblank, measured with `WAYLAND_DEBUG=1` (one `done` per frame) and Rune's commit log
(`why=["input"]` every frame). With the cache the window rests between caret blinks.

## macOS: Option is Alt

`src/platform_impl/macos/window_delegate.rs`: a window's `option_as_alt` defaults to
`OptionAsAlt::Both` instead of `None`.

Why: every Alt shortcut the app has (Alt+E, Alt+A, Alt+N, Alt+C, Alt+1..9, ...) is Option on a
Mac. With the stock default, Option plus a letter types that letter's alternate character
(Option+A arrives as "å") and Option+E, N, U, I and ` are dead keys that produce no key event at
all, so those chords could not be heard. With this default the key arrives as the letter with
Alt held, on every layout. The cost is the one terminals with "Option as Alt" accept: Option no
longer types accented or special characters inside Neoism. Rune has no setting for it at the
pinned revision; `WindowExtMacOS::set_option_as_alt` still changes it per window.
