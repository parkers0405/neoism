# Appearance

Neoism renders the terminal, editor, workspace chrome, panels, and overlays through its own GPU stack. Appearance is configured in domain blocks and through the Themes/Extensions UI, not by browser CSS.

## Themes and terminal palettes

`appearance.theme` controls the IDE-wide theme. `appearance.palette` selects a terminal palette; they are related but independent.

```jsonc
{
  "appearance": {
    "theme": "tokyo_night",
    "line-height": 1.2,
    "fonts": {
      "family": "CascadiaCode",
      "size": 14.0,
      "weight": 400
    }
  }
}
```

Open the hamburger menu and choose **Themes** to browse installed themes. UI changes are written back to the matching configuration domain.

### Follow Omarchy

On Omarchy, Neoism exposes an `omarchy` theme automatically from the active `colors.toml`. Select **Omarchy** in the theme picker once, or set:

```jsonc
{
  "appearance": {
    "theme": "omarchy"
  }
}
```

Neoism maps Omarchy's semantic background, foreground, accent, ANSI, and syntax colors across the terminal, editor, and workspace chrome. On Linux desktop it reads `$XDG_STATE_HOME/omarchy/current/theme/colors.toml`, falling back to `~/.local/state/omarchy/current/theme/colors.toml`.

Future `omarchy-theme-set` changes are watched and applied live. Omarchy replaces its active theme atomically; Neoism follows that replacement without a generated Neoism theme, symlink, restart, or shell hook. The integration is currently native-desktop only because web/Wasm clients cannot read host theme files.

## Splash animations

Startup logo animations are off by default. Set `appearance.splash-auto-animate` to `true` to play one random NEOISM animation when each new terminal or agent tab first shows its splash. Existing tabs do not replay when you change this setting, switch tabs, resize, or clear the terminal. Clicking the logo always plays an animation, regardless of this preference.

```jsonc
{
  "appearance": {
    "splash-auto-animate": true
  }
}
```

You can also toggle **Splash startup animation** under Appearance in Settings. Changes apply without restarting Neoism.

## Backgrounds

On desktop, choose **Backgrounds** from the hamburger menu or search for **Backgrounds** in the command palette. The picker uses the same searchable list and split layout as Themes, with the selected image displayed on the right. Arrow keys browse, Enter or a result click applies, and Escape closes without changing your background. Narrow windows show the list without the preview pane.

Images placed in `~/.config/neoism/bg/` appear alongside wallpaper images declared by installed Mash Up Packs and your currently configured background. Folder images retain the current standalone wallpaper opacity, blur, fit, and position when switching, while pack images retain their declared properties. The right-hand preview shows the artwork at full opacity so it remains easy to compare. On Windows this folder is under `%LOCALAPPDATA%\neoism\bg`; relocated configuration homes use their own `bg` folder. Selecting a pack's image does not activate that pack or change its theme, font, or panel colors.

You can also configure an image directly, with no active pack:

```jsonc
{
  "ui": {
    "window": {
      "background-image": {
        "path": "/absolute/path/to/painting.png",
        "opacity": 0.35,
        "blur": 3,
        "fit": "cover",
        "position-x": 0.5,
        "position-y": 0.5
      },
      "background-image-disabled": false
    }
  }
}
```

Images default to centered **cover**: the image keeps its aspect ratio and fills the window, cropping overflow rather than stretching. Set `fit` to `"contain"` to show the entire image without cropping, leaving the window background visible around it. `position-x` and `position-y` control alignment from `0` (left/top) to `1` (right/bottom); both default to `0.5` (center). Position selects which overflow is cropped in cover mode and where spare space falls in contain mode. These fields live inside `ui.window.background-image`, or the equivalent `platform.linux.window.background-image`, `platform.macos.window.background-image`, or `platform.windows.window.background-image` override. Settings exposes fit as a Cover/Contain choice and position as numbers from 0 to 1. Fit and position can also be set without a path to adjust an inherited pack wallpaper without changing its opacity. Older path/opacity-only configs use centered cover automatically.

Background selection has three states: **Use pack/default** clears the standalone override and lets the active pack supply its wallpaper; **None** suppresses all wallpaper without deactivating the pack; an **image** overrides the pack wallpaper. None is stored as `ui.window.background-image-disabled: true`. Image opacity controls only the wallpaper, independently of whole-window opacity and panel background alpha. In the example, `opacity: 0.35` blends the image at 35% opacity; it does not make the whole window 35% opaque. Image `blur` is approximate Gaussian sigma measured in source-image pixels, defaults to `0` (disabled), accepts finite values from `0` to `100`, and `0` disables image blur. The example's `blur: 3` explicitly opts into image blur. For artwork already blurred on disk, use `0` to avoid blurring it twice. Settings labels this **Image blur**. Blur can also be set without a path alongside fit and position to adjust an inherited pack wallpaper. The separate `ui.window.blur` boolean (**Background blur**) requests OS compositor blur behind translucent windows and does not control image blur. Config edits are hot-reloaded and picker changes preserve JSONC comments.

Wallpaper decoding and optional blur run on a background worker at startup and during config reload. A window with a configured wallpaper stays hidden until the image is ready for its first frame, avoiding a plain-background flash. The first uncached load can take longer; later launches reuse cached pixels. An already visible wallpaper stays in place while its replacement loads. Failed or cleared image requests release the startup wait so a bad path cannot leave the window hidden. Prepared pixels are cached in the OS cache directory; changing the source image, opacity, or blur invalidates the entry. Fit and position reuse the same pixels. The cache is bounded to 64 entries and 256 MiB; a missing, damaged, or unwritable cache falls back to preparing the source image.

## Fonts and symbols

Neoism supports a primary font plus fallback/symbol handling used across terminal and chrome. Choose a monospace font with the glyph coverage your shell, status line, and code require. Font size can be changed with platform zoom shortcuts.

## Cursor

Terminal cursor shape/blinking belongs under `terminal.cursor`:

```jsonc
{
  "terminal": {
    "cursor": {
      "shape": "block",
      "blinking": false
    }
  }
}
```

Collaborator/presence cursor presentation belongs under `presence`, including display name and supported cursor styling. It is separate from the terminal caret.

## Window and chrome

Window opacity, blur, navigation visibility, tabs, panels, and status UI live in `ui`. Platform-specific differences belong under `platform` overrides. Keep settings in their documented domain rather than adding loose top-level keys.

## Shaders and Mash Up Packs

Renderer shader overlays and Mash Up Packs can create more opinionated looks. Packs group coordinated appearance settings; shaders run as GPU effects over supported content. Disable expensive effects when diagnosing animation or rendering performance.

See [[Mash Up Packs]] for pack manifests, editor Lua plugin selection, user overrides, and transaction behavior.

## Extensions

Use **hamburger menu → Extensions** for installable runtime components such as language servers, formatters/linters where available, MCP servers, and kernels. Themes use the adjacent Themes flow; built-in syntax parsers require no download.

## Troubleshooting

- Missing glyphs: choose a font/fallback with the required symbols.
- Terminal colors differ from chrome: check both theme and palette.
- Effects stutter: disable shader overlays and inspect the FPS/developer tools.
- A setting appears ignored: confirm its domain and kebab-case spelling.
