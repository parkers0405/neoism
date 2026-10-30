# Appearance

Neoism uses GPUIX for its window, editor, workspace panels, and terminal display. Open **Settings** or edit `~/.config/neoism/config.json` to change supported appearance settings. The configuration accepts JSONC comments and trailing commas.

## Themes and fonts

Choose a bundled theme from **Themes**, or configure the theme and primary font directly:

```jsonc
{
  "appearance": {
    "theme": "pastel_dark",
    "fonts": { "family": "Cascadia Code NF", "size": 14, "weight": 400 },
    "splash-auto-animate": false
  }
}
```

The font applies across the workspace. Clicking the Home or Agent wordmark plays an animation; `splash-auto-animate` controls automatic playback. Missing glyphs usually require a font with broader symbol coverage.

## Backgrounds

Set an image path and opacity under `ui.window`. GPUIX centers the image and scales it to cover the window. Opacity affects the wallpaper only.

```jsonc
{
  "ui": {
    "window": {
      "background-image": { "path": "/absolute/path/to/painting.png", "opacity": 0.35 },
      "background-image-disabled": false
    },
    "styles": { "composer.agent": { "opacity": 0.85 } }
  }
}
```

Set `background-image-disabled` to `true` to hide the wallpaper. `composer.agent.opacity` controls the Agent composer's background without fading its text.

## Cursors

Terminal cursor blinking is configured under `terminal.cursor.blinking`. Collaborator cursor styling and identity are separate settings under `presence`.

See [[Plugins]] for the supported extension model.
