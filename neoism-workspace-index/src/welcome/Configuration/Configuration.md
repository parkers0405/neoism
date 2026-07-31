# Configuration

Neoism reads a single JSON file — **terminal, editor, and agent settings all co-live in it**:

```text
Linux    ~/.config/neoism/config.json   (or $NEOISM_CONFIG_HOME, or $XDG_CONFIG_HOME/neoism)
macOS    ~/.config/neoism/config.json
Windows  %LOCALAPPDATA%\neoism\config.json
```

The file is **JSONC**: `//` and `/* */` comments and trailing commas are legal. Saves hot-reload instantly — the app re-applies on every write, and the agent server reads the file fresh per request. Neoism is **JSON only**; there is no `config.toml`.

On first launch Neoism writes an annotated `config.json` with each domain shown as a commented example. **Every key is optional** and falls back to a sensible default. Open it from the command palette (search "config") or edit it directly.

Keys are **kebab-case** throughout — terminal and agent alike (`line-height`, `format-on-save`, `small-model`, `reasoning-effort`).

## Settings are grouped by domain

Every setting lives inside a **domain block** — there are no loose top-level keys. Pick the block, then the key:

| Block | What it configures |
|---|---|
| `appearance` | theming + typography (IDE theme, terminal palette, fonts, packs, effects) |
| `editor` | the built-in code + markdown editor |
| `terminal` | the terminal emulator |
| `ui` | window chrome — OS window, tabs, title, panels, status line |
| `presence` | how collaborators see you in multiplayer |
| `keybinds` | keyboard-shortcut overrides |
| `agent` | the Neoism Agent (model, providers, permissions, MCP) |
| `renderer` | GPU backend + post-process effects |
| `developer` | logging + the FPS counter |
| `platform` | per-OS overrides (`windows` / `macos` / `linux`) |

```jsonc
{
  "appearance": {
    "theme": "tokyo_night",        // IDE theme: pastel_dark | nvchad_one | tokyo_night | catppuccin_mocha
    "palette": "lucario",          // terminal color file in themes/<name>.json (optional, separate from theme)
    "line-height": 1.2,
    "fonts": { "family": "CascadiaCode", "size": 14.0, "weight": 400 }
  },
  "editor": {
    "vim-mode": true,
    "format-on-save": true,
    "minimap": false
  },
  "terminal": {
    "shell": { "program": "/bin/fish", "args": ["--login"] },
    "cursor": { "shape": "block", "blinking": false },
    "scroll": { "multiplier": 3.0 }
  },
  "ui": {
    "window": { "opacity": 0.95, "blur": true },
    "status-fps": true             // frame-rate pill on the status bar
  },
  "presence": {
    "display-name": "your-name"    // what collaborators see in multiplayer
  }
}
```

> **`theme` vs `palette`.** `appearance.theme` is the IDE theme that skins the chrome, editor, and terminal together (and is what the theme picker sets). `appearance.palette` is an optional terminal-only color file in `themes/<name>.json`. Most people only ever set `theme`.

## Where it loads from

Neoism reads the **global** `config.json` above. The agent additionally layers a **per-project** file when it finds one — `neoism.json` (or a `.neoism/` folder) at your workspace root — so a repo can pin its own model, MCP servers, or permissions. Later sources win; most people only ever touch the global file.

## `appearance`

| Key | Type | Notes |
|---|---|---|
| `theme` | string | IDE theme (chrome + editor + terminal) |
| `palette` | string | terminal color file `themes/<name>.json` |
| `line-height` | number | line-height multiplier |
| `fonts` | object | `family`, `size`, `weight`, `symbol-map`, … — see [[Themes, Cursor and Fonts\|Themes, Cursor & Fonts]] |
| `mashup-pack` | string | active [[Mash Up Packs\|Mash Up Pack]] id under `packs/<id>` |
| `look` | object | per-slot overrides on the active pack (`scrollbar`, `markdown`, `icons`) |
| `effects` | object | `custom-mouse-cursor`, `trail-cursor` |
| `force-theme` | string | force `"dark"` / `"light"` window appearance |
| `colors` | object | full ANSI palette override (`background`, `foreground`, `cursor`, 16 base + `dim-*`/`light-*`) |

## `editor`

| Key | Type | Notes |
|---|---|---|
| `vim-mode` | bool | vim keybindings in the code + markdown editors (default on) |
| `format-on-save` | bool | run the LSP formatter before every save (default on) |
| `minimap` | bool | code-editor minimap |
| `external` | `{ program, args[] }` | external editor for "open in editor" |

Language servers and the `lsp` toggle are covered in [[../Editor/Languages and LSP|Languages & LSP]].

## `terminal`

| Key | Type | Notes |
|---|---|---|
| `shell` | `{ program, args[] }` | login shell for the terminal + the agent's run tool |
| `cursor` | object | `shape`, `blinking`, `blinking-interval` |
| `scroll` | object | `multiplier`, `divider` |
| `keyboard` | object | `disable-ctlseqs-alt`, `ime-cursor-positioning` |
| `working-dir` | string | startup working directory |
| `env-vars` | string[] | `["KEY=VALUE"]` |
| `option-as-alt` | string | macOS: `None` / `Left` / `Right` / `Both` |
| `copy-on-select` | bool | auto-copy on selection |
| `hide-mouse-cursor-when-typing` | bool | hide the pointer while typing |
| `draw-bold-text-with-light-colors` | bool | bold text draws in the bright ANSI palette |
| `bell` | object | `audio` |
| `hints` | object | URL/path hint `alphabet` + `rules[]` |
| `enable-scroll-bar` | bool | show the scrollbar (default on) |
| `scrollback-history-limit` | int | scrollback lines (default 10000) |

## `ui`

| Key | Type | Notes |
|---|---|---|
| `window` | object | `opacity`, `blur`, `decorations`, `width`/`height`, `background-image`, macOS/Windows knobs |
| `navigation` | object | `hide-if-single`, `use-split`, `open-config-with-split`, `current-working-directory`, `unfocused-split-opacity`, … |
| `title` | object | window/tab title template |
| `panel` | object | split layout: `margin`, `padding`, `row-gap`, `column-gap`, `border-width`, `border-radius` |
| `margin` | array | outer terminal margin (CSS 1/2/4 values) |
| `status-fps` | bool | FPS pill on the status bar (default on) |
| `confirm-before-quit` | bool | quit confirmation |

## `presence`

| Key | Type | Notes |
|---|---|---|
| `display-name` | string | the name collaborators see in presence (`NEOISM_DISPLAY_NAME` overrides) |
| `cursor-color` | `#RRGGBB` | your caret colour (collaborators see it too) |
| `cursor-style` | string | `"solid"` (default) or `"rainbow"` |

Pair `cursor-style: "rainbow"` with `cursor-color` and collaborators see your caret in sync. See [[../Multiplayer|Multiplayer]].

## `keybinds`

Override any built-in shortcut. See [[../Keybindings|Keybindings]] for the full list and the action names.

```jsonc
{
  "keybinds": {
    "keys": [
      { "key": "t", "with": "super", "action": "CreateTab" }
    ]
  }
}
```

## `renderer` & `developer`

`renderer` selects the GPU backend and post-process effects (`backend`, `strategy`, `filters`, `shader-overlays`, `disable-unfocused-render`, `disable-occluded-render`, `use-cpu`) — see [[Shaders]]. `developer` holds `log-level`, `enable-log-file`, and `enable-fps-counter`.

## `agent` — same file, its own block

The Neoism Agent reads its keys from the `agent` block of this same `config.json` (kebab-case). A dedicated per-project `neoism.json` is an agent config at its own root — the global `config.json` nests it under `agent`.

```jsonc
{
  "agent": {
    "model": "anthropic/claude-opus-5",
    "small-model": "anthropic/claude-haiku-4-5",
    "reasoning-effort": "high",    // low | medium | high | xhigh | max
    "input-hints": true,           // helper row below the input; /hints toggles and saves this
    "permission": { "edit": "ask", "bash": "ask" },
    "mcp": {
      "fff": { "type": "local", "command": ["fff-mcp"], "enabled": true }
    },
    "dangerously-skip-permissions": false
  }
}
```

`"dangerously-skip-permissions": true` auto-allows every agent permission that would normally prompt (explicit `"permission"` deny rules still deny) — the config-level equivalent of `--dangerously-skip-permissions`. For a single session, type `/yolo` in the agent pane instead: it auto-answers "Yes" to every prompt until you `/yolo` again.

`"input-hints": false` hides the complete helper row below the agent input and gives that space back to the conversation. `/hints` toggles the same preference and writes it here, so the choice survives new chats and restarts.

Other `agent` keys: `default-agent`, `enabled-providers` / `disabled-providers`, `tools`, `instructions`, `skills`, `formatter`, `lsp`, `share`, `experimental`, and the `agent` / `mode` / `command` maps of named definitions.

Related conventions in the same directory: skills live in `~/.config/neoism/skills/`; markdown agent, mode, and command definitions go in `~/.config/neoism/agent/<name>.md` (and `mode/`, `command/`); MCP servers can live in a standalone `~/.config/neoism/mcp.json` (either `{ "mcp": { ... } }` or a bare server map) — it merges after `config.json`, so its entries win. The extensions page writes MCP installs there.

## Related files

- **`.neoism/workspace.json`** (per code directory) — workspace id, name, and notes/vault settings. Written automatically.
- **`project.json`** (inside a vault) — the code dirs a vault is linked to, for `[[@` page-link completion.
- **`packs/<id>/pack.json`**, **`themes/<name>.json`**, **`ide-themes/*.json`** — [[Mash Up Packs]], terminal palettes, and IDE themes.

## Environment variables

A few settings have `NEOISM_*` escape hatches, handy for scripting or one-off overrides: `NEOISM_CONFIG_HOME` (config dir), `NEOISM_NOTES_HOME` (vaults root), `NEOISM_DISPLAY_NAME` (presence name), `NEOISM_LOG_LEVEL` / `NEOISM_LOG_FILE` (logging), and `NEOISM_REQUIRE_AUTH` (multiplayer pairing). These override the matching config keys when set.

> Changes made from the UI (theme picker, preferences) are written back into this same file, inside the matching domain block.
