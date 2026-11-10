# winit compatibility patches

`winit-0.30.13` is the crates.io release with one change, used by the Rune frontend
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
