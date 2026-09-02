# Configure Neoism

Neoism keeps terminal, editor, UI, presence, keybinding, agent, renderer, and developer settings in one **JSONC** file:

```text
Linux    ~/.config/neoism/config.json
macOS    ~/.config/neoism/config.json
Windows  %LOCALAPPDATA%\neoism\config.json
```

On Linux, `NEOISM_CONFIG_HOME` or `$XDG_CONFIG_HOME/neoism` can relocate it. Press `Alt + ,` or choose **Open Neoism Config** from the command palette. Neoism creates the file on first open and opens it in the native editor without changing your workspace root or terminal directory.

JSONC permits `//` and `/* ... */` comments and trailing commas. Neoism hot-reloads the file after a save. Keys use **kebab-case**, and every setting belongs inside a domain block; there are no loose top-level settings.

## Use config completion

`config.json` has built-in Neoism completion. Completion describes every supported object and key, shows its documentation and default, and suggests valid values while you type. Value suggestions include both fixed choices and capabilities reported by the host that owns the config, such as installed fonts, themes, agents, models, shells, extensions, and language servers.

Dynamic values are suggestions rather than restrictions when a setting accepts custom input. For example, you can type a font family or executable path that is not currently installed, which keeps configurations portable between hosts.

The graphical **Settings** page and the raw JSONC editor are two views of this same file. They use the same setting descriptions and available-value catalog, and a change made in either place updates `config.json`. Programmatic Settings changes preserve existing JSONC comments and formatting.

For a local desktop, `Alt + ,` opens that machine's config. In a web or joined remote workspace it opens the connected daemon host's config, because fonts, tools, agents, and models are resolved on that host.

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

`appearance.theme` selects the IDE-wide theme. `appearance.palette` is a separate terminal color palette file; setting one does not implicitly set the other. Omarchy users can select the live `omarchy` theme described in [[Neoism/Appearance#Follow Omarchy|Appearance]].

## Configure the agent separately, in the same file

Agent settings are nested under `agent`:

```jsonc
{
  "agent": {
    "variant": "high",
    "textVerbosity": "low",
    "input-hints": true,
    "permission": {
      "edit": "ask",
      "bash": "ask",
    },
    "dangerouslySkipPermissions": false,
  },
}
```

Choose a model through the agent model picker before hard-coding `agent.model`; available identifiers depend on configured providers. A workspace can override the same domain-based schema in `.neoism/config.json`.

## Related files and overrides

- `.neoism/workspace.json` stores per-workspace identity and notes settings and is written automatically.
- `~/.config/neoism/mcp.json` and workspace `.neoism/mcp.json` can hold a bare MCP server map or `{ "mcp": {...} }`; each merges after `config.json` in the same scope. See [[Neoism Agent/MCP Servers]].
- `~/.config/neoism/skills/` contains agent skills.
- `~/.config/neoism/agent/`, `mode/`, and `command/` contain Markdown agent definitions.
- `NEOISM_DISPLAY_NAME`, `NEOISM_NOTES_HOME`, `NEOISM_LOG_LEVEL`, `NEOISM_LOG_FILE`, and `NEOISM_REQUIRE_AUTH` provide targeted environment overrides.

Changes made by Neoism's Settings page, preferences, theme picker, or `/hints` command are written back into the matching domain block in this same file.

Next: [[07 Essential Keybindings|Essential Keybindings]].
