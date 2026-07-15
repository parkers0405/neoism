# Configuration

Neoism reads a single JSON file — **terminal and agent settings co-live in it**:

```text
Linux    ~/.config/neoism/config.json   (or $NEOISM_CONFIG_HOME)
macOS    ~/.config/neoism/config.json
Windows  %LOCALAPPDATA%\neoism\config.json
```

Comments (`//`, `/* */`) and trailing commas are legal — the file is JSONC. Saves hot-reload instantly: the app re-applies on every write, and the agent server reads the file fresh per request. A legacy `config.toml` is still honored if no `config.json` exists.

On first launch Neoism writes a default `config.json` (just a comment pointing at the docs) — **every key is optional** and falls back to a sensible default. Open it from the command palette (search "config") or edit it directly.

## The essentials

```json
{
  "neoism": {
    "theme": "pastel_dark",      // unified theme for chrome + terminal + nvim
    "minimap": false,
    "display-name": "your-name", // what collaborators see in multiplayer presence
    "status-fps": true           // frame-rate pill on the status bar
  },
  "fonts": { "family": "cascadiacode", "size": 14.0 },
  "cursor": { "shape": "block", "blinking": false },
  "scroll": { "multiplier": 3.0 }
}
```

Also useful at the top level (all kebab-case): `"line-height"`, `"copy-on-select"`, `"confirm-before-quit"`, `"hide-mouse-cursor-when-typing"`, `"enable-scroll-bar"`, `"scrollback-history-limit"`, `"working-dir"`, `"env-vars": ["KEY=VALUE"]`, and `"force-theme": "dark"`.

## Agent settings — same file

The Neoism Agent reads its keys from this same `config.json`. Its keys sit at the top level alongside the terminal sections:

```json
{
  "model": "anthropic/claude-fable-5",
  "variant": "high",
  "mcp": {
    "fff": { "type": "local", "command": ["fff-mcp"], "enabled": true }
  },
  "dangerouslySkipPermissions": false
}
```

`"dangerouslySkipPermissions": true` auto-allows every agent permission that would normally prompt (explicit `"permission"` deny rules still deny) — the config-level equivalent of `--dangerously-skip-permissions`. For a single session, type `/yolo` in the agent pane instead: it toggles auto-answering "Yes" to every prompt until you `/yolo` again.

Related agent conventions in the same directory: skills live in `~/.config/neoism/skills/`, markdown agent definitions can go in `~/.config/neoism/agent/<name>.md`, and MCP servers can live in a standalone `~/.config/neoism/mcp.json` (either `{ "mcp": { ... } }` or a bare server map) — it merges after `config.json`, so its entries win. The extensions page writes MCP installs there.

## The rest of the tree

- [[Themes, Cursor and Fonts|Themes, Cursor & Fonts]] — pick a theme, color your cursor, choose a font.
- [[Shaders]] — optional CRT and post-process filters.
- [[../Keybindings|Keybindings]] — remap keys via the `"bindings"` table.

Other sections you can set: `"window"`, `"navigation"`, `"keyboard"`, `"bell"`, `"hints"`, `"renderer"` (see [[Shaders]]), and `"developer"` (`log-level`, `enable-fps-counter`).

> Changes made from the UI (theme picker, preferences) are written back into the `"neoism"` object of this same file.
