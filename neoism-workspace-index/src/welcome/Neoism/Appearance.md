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

## Backgrounds

On desktop, choose **Backgrounds** from the hamburger menu or search for **Backgrounds** in the command palette. The picker uses the same searchable list and split layout as Themes, with the selected image displayed on the right. Arrow keys browse, Enter or a result click applies, and Escape closes without changing your background. Narrow windows show the list without the preview pane.

Images placed in `~/.config/neoism/bg/` appear alongside wallpaper images declared by installed Mash Up Packs and your currently configured background. Folder images retain the current standalone wallpaper opacity when switching, while pack images retain their declared opacity. The right-hand preview shows the artwork at full opacity so it remains easy to compare. On Windows this folder is under `%LOCALAPPDATA%\neoism\bg`; relocated configuration homes use their own `bg` folder. Selecting a pack's image does not activate that pack or change its theme, font, or panel colors.

You can also configure an image directly, with no active pack:

```jsonc
{
  "ui": {
    "window": {
      "background-image": {
        "path": "/absolute/path/to/painting.png",
        "opacity": 0.35
      },
      "background-image-disabled": false
    }
  }
}
```

Background selection has three states: **Use pack/default** clears the standalone override and lets the active pack supply its wallpaper; **None** suppresses all wallpaper without deactivating the pack; an **image** overrides the pack wallpaper. None is stored as `ui.window.background-image-disabled: true`. Image opacity controls only the wallpaper, independently of whole-window opacity and panel background alpha. Config edits are hot-reloaded and picker changes preserve JSONC comments.

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
