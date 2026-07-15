---
sidebar_position: 1
title: Configuration
---

# Configuration

Neoism has **one configuration file**: `config.json` in the Neoism config directory. Terminal, editor, window, and agent settings all co-live in it — there is no separate agent config folder.

The file is **JSONC**: `//` and `/* */` comments plus trailing commas are legal. Saves hot-reload — the app re-applies settings on every write, and the agent server reads the file fresh per request, so there is no restart step for either side.

On Linux the default path is:

```text
$XDG_CONFIG_HOME/neoism/config.json
```

If `XDG_CONFIG_HOME` is unset, Neoism falls back to:

```text
$HOME/.config/neoism/config.json
```

Set `NEOISM_CONFIG_HOME` to override the config directory. A legacy `config.toml` is still honored when no `config.json` exists.

## App settings

Common sections (each is a top-level JSON object) and fields include:

- `"neoism"` - theme, minimap, display name, cursor color/style, blinking-cursor alias, `status-fps` (frame-rate pill on the status bar), `mashup-pack`.
- `"cursor"` - terminal cursor shape, blinking, and blink interval.
- `"window"` - window mode, dimensions, opacity, blur, decorations, colorspace, IME behavior, background image.
- `"renderer"` - backend, unfocused/occluded rendering behavior, filters, and render strategy.
- `"navigation"` - tab/plain navigation mode, split behavior, clickable navigation, and pane opacity.
- `"keyboard"` - alt control-sequence behavior and IME cursor positioning.
- `"bell"` and `"effects"` - audio bell, custom mouse cursor, and cursor trail behavior.
- `"panel"` - split/chrome margin, padding, gaps, border width, and border radius.
- `"look"` - per-slot overrides (scrollbar / markdown / wordmark / icons) that win over the active Mash Up Pack.
- Top-level fields such as `"shell"`, `"editor"`, `"working-dir"`, `"line-height"`, `"env-vars"`, `"scrollback-history-limit"`, `"theme"`, `"adaptive-theme"`, `"force-theme"`, `"use-fork"`, `"copy-on-select"`, and `"confirm-before-quit"`.

Example:

```json
// Unified Neoism config — comments are legal (JSONC)
{
  "working-dir": "/home/me/projects",
  "line-height": 1.0,
  "scrollback-history-limit": 10000,
  "copy-on-select": true,
  "confirm-before-quit": true,
  "neoism": { "theme": "pastel_dark", "status-fps": true },
  "cursor": { "shape": "block", "blinking": true, "blinking-interval": 530 },
  "window": { "mode": "windowed", "width": 1200, "height": 800, "opacity": 1.0 },
  "renderer": { "backend": "automatic", "disable-unfocused-render": false },
  "keyboard": { "ime-cursor-positioning": true }
}
```

## Agent settings — same file

The agent server reads its keys from the same `config.json`, at the top level next to the app sections. Each reader ignores the other's keys, so they never conflict (the one shared key, `"shell"`, is accepted in both shapes).

Agent config can describe:

- Default model as `"model": "provider/model"` and thinking variant as `"variant"`.
- Enabled and disabled providers, and the default agent.
- Permission defaults and per-tool rules (`"permission"`), plus `"dangerouslySkipPermissions": true` to auto-allow everything that would prompt (explicit deny rules still deny).
- MCP server definitions (`"mcp"`).
- LSP and formatter integration.

```json
{
  "model": "anthropic/claude-fable-5",
  "variant": "high",
  "permission": { "external_directory": "ask" },
  "dangerouslySkipPermissions": false
}
```

### Standalone MCP catalog

MCP servers can also live in their own file, `mcp.json`, next to `config.json` — either wrapped (`{ "mcp": { ... } }`) or as a bare server map. It merges **after** `config.json`, so its entries win. The extensions page writes MCP installs there.

```json
// ~/.config/neoism/mcp.json
{
  "mcp": {
    "fff": { "type": "local", "command": ["fff-mcp"], "enabled": true }
  }
}
```

### Project config and directory layout

Project agent config is discovered upward from the workspace via `neoism.json` / `neoism.jsonc` (and `.neoism/` config directories, which may also hold their own `mcp.json`). Set `NEOISM_AGENT_DISABLE_PROJECT_CONFIG=1` to ignore project config while debugging. `NEOISM_AGENT_CONFIG_DIR` overrides the agent's global config directory.

Sibling conventions in the config directory:

- `skills/` — agent skills.
- `agent/<name>.md`, `mode/<name>.md`, `command/<name>.md` — markdown agent/mode/command definitions.
- `packs/<id>/pack.json` and `ide-themes/<name>.json` — Mash Up Packs and standalone themes (same JSONC dialect).

## Documentation Rule

Do not document guessed settings. If a setting is not verified in code, mark it as planned or leave it out.

## Settings Page Notes

The docs should eventually drive the settings UI: every setting should have a label, description, type, default, validation rule, and restart requirement. Keep the configuration reference structured so it can become schema-driven later.

Keep the settings page backed by config validation so bad model refs, zero-step agent configs, and malformed provider entries are caught before save.
