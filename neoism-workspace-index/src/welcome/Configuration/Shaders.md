# Shaders

Neoism can run GPU post-process effects over the whole surface. Two knobs, both under `[renderer]` in [[Configuration|config.json]].

## Built-in filters

```toml
[renderer]
filters = ["crt_curve"]
```

`filters` is a list of full-screen filter chains, applied live. Built-in names:

- **`crt_curve`** — a classic curved CRT-TV look. Aliases: `crt-curve`, `crtcurve`, `classic_crt_tv`, `classic-crt-tv`.
- **`newpixiecrt`** — a high-contrast scanline CRT.

Any other value is treated as a path to your own RetroArch `.slangp` preset. Filters need a GPU backend that supports copy operations; if yours doesn't, Neoism logs a warning and skips them.

## Custom shader overlays

```toml
[renderer]
shader-overlays = ["/path/to/effect.glsl"]
```

`shader-overlays` are your own GLSL overlay shaders. Once configured, the command palette's **Shaders** entry lists them (plus a **None** option to turn them off) so you can switch live. With none configured, that picker just tells you to add paths here first.

> `filters` (RetroArch presets) and `shader-overlays` (your GLSL) are independent — set filters in config; switch overlays from the palette.
