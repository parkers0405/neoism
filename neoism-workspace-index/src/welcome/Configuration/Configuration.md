# Configuration

Neoism reads a single JSON file — **terminal, editor, and agent settings all co-live in it**:

```text
Linux    ~/.config/neoism/config.json   (or $NEOISM_CONFIG_HOME, or $XDG_CONFIG_HOME/neoism)
macOS    ~/.config/neoism/config.json
Windows  %LOCALAPPDATA%\neoism\config.json
```

The file is **JSONC**: `//` and `/* */` comments and trailing commas are legal. Saves hot-reload instantly — the app re-applies on every write, and the agent server reads the file fresh per request. Neoism is **JSON only**; there is no `config.toml`.

On first launch Neoism writes an annotated `config.json` with every common key shown as a commented example. **Every key is optional** and falls back to a sensible default. Open it from the command palette (search "config") or edit it directly.

Keys are **kebab-case** throughout — terminal and agent alike (`line-height`, `format-on-save`, `small-model`, `reasoning-effort`).

## Where it loads from

Neoism reads the **global** `config.json` above. The agent additionally layers a **per-project** file when it finds one — `neoism.json` (or a `.neoism/` folder) at your workspace root — so a repo can pin its own model, MCP servers, or permissions. Later sources win; most people only ever touch the global file.

## The essentials

Everything lives at the **top level** — there is no `[neoism]` wrapper.

```json
{
  "theme": "tokyo_night",        // IDE theme: pastel_dark | nvchad_one | tokyo_night | catppuccin_mocha
  "palette": "lucario",          // terminal color file in themes/<name>.json (optional, separate from theme)
  "line-height": 1.2,
  "minimap": false,
  "format-on-save": true,
  "display-name": "your-name",   // what collaborators see in multiplayer presence
  "status-fps": true,            // frame-rate pill on the status bar
  "fonts": { "family": "CascadiaCode", "size": 14.0 },
  "cursor": { "shape": "block", "blinking": false },
  "scroll": { "multiplier": 3.0 }
}
```

> **`theme` vs `palette`.** `theme` is the IDE theme that skins the chrome, editor, and terminal together (and is what the theme picker sets). `palette` is an optional terminal-only color file in `themes/<name>.json`. Most people only ever set `theme`.

## Appearance

| Key | Type | Notes |
|---|---|---|
| `theme` | string | IDE theme (chrome + editor + terminal) |
| `palette` | string | terminal color file `themes/<name>.json` |
| `line-height` | number | terminal line-height multiplier |
| `minimap` | bool | code-editor minimap |
| `mashup-pack` | string | active [[Mash Up Packs\|Mash Up Pack]] id under `packs/<id>` |
| `cursor-color` | `#RRGGBB` | your caret colour (collaborators see it too) |
| `cursor-style` | string | `"solid"` (default) or `"rainbow"` |
| `status-fps` | bool | FPS pill on the status bar (default on) |
| `force-theme` | string | force `"dark"` / `"light"` window appearance |
| `fonts` | object | see [[Themes, Cursor and Fonts\|Themes, Cursor & Fonts]] |
| `cursor` | object | `shape`, `blinking`, `blinking-interval` |
| `window` | object | `opacity`, `blur`, `decorations`, `width`/`height`, `background-image`, macOS/Windows knobs |
| `colors` | object | full ANSI palette override (`background`, `foreground`, `cursor`, 16 base + `dim-*`/`light-*`) |
| `look` | object | per-slot overrides on the active pack (`[look.scrollbar]`, `[look.markdown]`, `[look.icons]`) |

## Terminal

| Key | Type | Notes |
|---|---|---|
| `shell` | `{ program, args[] }` | login shell for the run tool |
| `working-dir` | string | startup working directory |
| `scrollback-history-limit` | int | scrollback lines (default 10000) |
| `enable-scroll-bar` | bool | show the scrollbar (default on) |
| `copy-on-select` | bool | auto-copy on selection |
| `confirm-before-quit` | bool | quit confirmation |
| `hide-mouse-cursor-when-typing` | bool | hide the pointer while typing |
| `option-as-alt` | string | macOS: `None` / `Left` / `Right` / `Both` |
| `env-vars` | string[] | `["KEY=VALUE"]` |
| `scroll` | object | `multiplier`, `divider` |
| `navigation` | object | `hide-if-single`, `use-split`, `open-config-with-split`, `current-working-directory`, `unfocused-split-opacity`, … |
| `bell` | object | `audio` |
| `hints` | object | URL/path hint `alphabet` + `rules[]` |
| `keyboard` | object | `disable-ctlseqs-alt`, `ime-cursor-positioning` |
| `effects` | object | `custom-mouse-cursor`, `trail-cursor` |
| `panel` | object | split layout: `margin`, `padding`, `row-gap`, `column-gap`, `border-width`, `border-radius` |
| `margin` | array | outer terminal margin (CSS 1/2/4 values) |

## Editor

| Key | Type | Notes |
|---|---|---|
| `format-on-save` | bool | run the LSP formatter before every save (default on) |
| `minimap` | bool | code-editor minimap |

Language servers and the `lsp` toggle are covered in [[../Editor/Languages and LSP|Languages & LSP]].

## Renderer & developer

`renderer` selects the GPU backend and post-process effects (`backend`, `strategy`, `filters`, `shader-overlays`, `disable-unfocused-render`, `disable-occluded-render`, `use-cpu`) — see [[Shaders]]. `developer` holds `log-level`, `enable-log-file`, and `enable-fps-counter`.

## Multiplayer

`display-name` is the name collaborators see in presence (the `NEOISM_DISPLAY_NAME` env var overrides it). Pair `cursor-style: "rainbow"` with `cursor-color` and collaborators see your caret in sync. See [[../Multiplayer|Multiplayer]].

## Agent settings — same file

The Neoism Agent reads its keys from this same `config.json`, at the top level alongside the terminal keys (kebab-case):

```json
{
  "model": "anthropic/claude-opus-5",
  "small-model": "anthropic/claude-haiku-4-5",
  "reasoning-effort": "high",     // low | medium | high | xhigh | max
  "agent-input-hints": true,       // helper row below the input; /hints toggles and saves this
  "permission": { "edit": "ask", "bash": "ask" },
  "mcp": {
    "fff": { "type": "local", "command": ["fff-mcp"], "enabled": true }
  },
  "dangerously-skip-permissions": false
}
```

`"dangerously-skip-permissions": true` auto-allows every agent permission that would normally prompt (explicit `"permission"` deny rules still deny) — the config-level equivalent of `--dangerously-skip-permissions`. For a single session, type `/yolo` in the agent pane instead: it auto-answers "Yes" to every prompt until you `/yolo` again.

`"agent-input-hints": false` hides the complete helper row below the agent input and gives that space back to the conversation. `/hints` toggles the same global preference and writes it here, so the choice survives new chats and restarts.

Other agent keys: `default-agent`, `enabled-providers` / `disabled-providers`, `tools`, `instructions`, `skills`, `formatter`, `lsp`, `share`, `experimental`, and the `agent` / `mode` / `command` maps of named definitions.

Related conventions in the same directory: skills live in `~/.config/neoism/skills/`; markdown agent, mode, and command definitions go in `~/.config/neoism/agent/<name>.md` (and `mode/`, `command/`); MCP servers can live in a standalone `~/.config/neoism/mcp.json` (either `{ "mcp": { ... } }` or a bare server map) — it merges after `config.json`, so its entries win. The extensions page writes MCP installs there.

## Related files

- **`.neoism/workspace.json`** (per code directory) — workspace id, name, and notes/vault settings. Written automatically.
- **`project.json`** (inside a vault) — the code dirs a vault is linked to, for `[[@` page-link completion.
- **`packs/<id>/pack.json`**, **`themes/<name>.json`**, **`ide-themes/*.json`** — [[Mash Up Packs]], terminal palettes, and IDE themes.

## Environment variables

A few settings have `NEOISM_*` escape hatches, handy for scripting or one-off overrides: `NEOISM_CONFIG_HOME` (config dir), `NEOISM_NOTES_HOME` (vaults root), `NEOISM_DISPLAY_NAME` (presence name), `NEOISM_LOG_LEVEL` / `NEOISM_LOG_FILE` (logging), and `NEOISM_REQUIRE_AUTH` (multiplayer pairing). These override the matching config keys when set.

> Changes made from the UI (theme picker, preferences) are written back into this same file, at the top level.
