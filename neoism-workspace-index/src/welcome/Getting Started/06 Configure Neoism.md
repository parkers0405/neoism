# Configure Neoism

Neoism keeps terminal, editor, UI, presence, keybinding, agent, renderer, and developer settings in one **JSONC** file:

```text
Linux    ~/.config/neoism/config.json
macOS    ~/.config/neoism/config.json
Windows  %LOCALAPPDATA%\neoism\config.json
```

On Linux, `NEOISM_CONFIG_HOME` or `$XDG_CONFIG_HOME/neoism` can relocate it. Open the file with **Settings** from the command palette or with `Ctrl + Shift + ,` (`Cmd + ,` on macOS).

JSONC permits `//` and `/* ... */` comments and trailing commas. Neoism hot-reloads the file after a save. Keys use **kebab-case**, and every setting belongs inside a domain block; there are no loose top-level settings.

## Start with a small configuration

Every key is optional. Add only settings you want to override:

```jsonc
{
  "appearance": {
    "theme": "tokyo_night",
    "line-height": 1.2,
    "fonts": {
      "family": "CascadiaCode",
      "size": 14.0,
      "weight": 400,
    },
  },
  "editor": {
    "vim-mode": true,
    "format-on-save": true,
    "minimap": false,
  },
  "terminal": {
    "shell": {
      "program": "/bin/fish",
      "args": ["--login"],
    },
    "cursor": {
      "shape": "block",
      "blinking": false,
    },
    "scroll": { "multiplier": 3.0 },
  },
  "presence": {
    "display-name": "your-name",
  },
}
```

Change the shell program to one installed on your machine, or omit the `terminal.shell` block to use Neoism's default selection.

## Know the domain blocks

| Block | Purpose |
|---|---|
| `appearance` | IDE theme, terminal palette, fonts, line height, packs, and effects |
| `editor` | Native code and Markdown editor behavior |
| `terminal` | Shell, cursor, scrollback, navigation, and terminal integration |
| `ui` | Window, tabs, title, panels, and status line |
| `presence` | Multiplayer display name and cursor presentation |
| `keybinds` | User shortcut overrides |
| `agent` | Models, providers, permissions, MCP, tools, and agent definitions |
| `renderer` | GPU backend and post-processing controls |
| `developer` | Logging and the FPS counter |
| `platform` | `windows`, `macos`, and `linux` overrides |

`appearance.theme` selects the IDE-wide theme. `appearance.palette` is a separate terminal color palette file; setting one does not implicitly set the other.

## Configure the agent separately, in the same file

Agent settings are nested under `agent`:

```jsonc
{
  "agent": {
    "reasoning-effort": "high",
    "text-verbosity": "low",
    "input-hints": true,
    "permission": {
      "edit": "ask",
      "bash": "ask",
    },
    "dangerously-skip-permissions": false,
  },
}
```

Choose a model through the agent model picker before hard-coding `agent.model`; available identifiers depend on configured providers. A per-project `neoism.json` is also an agent configuration at its own root, while the global file nests the same settings under `agent`.

## Related files and overrides

- `.neoism/workspace.json` stores per-workspace identity and notes settings and is written automatically.
- `~/.config/neoism/mcp.json` can hold MCP server entries and merges after `config.json`.
- `~/.config/neoism/skills/` contains agent skills.
- `~/.config/neoism/agent/`, `mode/`, and `command/` contain Markdown agent definitions.
- `NEOISM_DISPLAY_NAME`, `NEOISM_NOTES_HOME`, `NEOISM_LOG_LEVEL`, `NEOISM_LOG_FILE`, and `NEOISM_REQUIRE_AUTH` provide targeted environment overrides.

Changes made by Neoism's preferences, theme picker, or `/hints` command are written back into the matching domain block in this same file.

Next: [[07 Essential Keybindings|Essential Keybindings]].
