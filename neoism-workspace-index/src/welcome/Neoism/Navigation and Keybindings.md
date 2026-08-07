# Navigation and Keybindings

Neoism uses one focus chain across panes and chrome panels. The command palette remains the fallback for every action.

## Essential navigation

| Keys | Action |
|---|---|
| `Alt+P` | Command palette |
| `Alt+E` | Explorer |
| `Alt+N` | Notes sidebar |
| `Alt+G` | Git changes |
| `Alt+S` | Project search |
| `Alt+A` | New agent pane |
| `Alt+Arrow` | Move focus among panes/panels |
| `Alt+Shift+Space` | Toggle Vim mode |

Mouse clicks focus panels through the same focus model. Modal overlays capture input until dismissed.

## Tabs, workspaces, and splits

| Linux / Windows | macOS | Action |
|---|---|---|
| `Ctrl+Shift+T` | `Cmd+T` | New terminal tab |
| `Ctrl+Shift+W` | `Ctrl+Shift+W` | New workspace |
| `Ctrl+Tab` | `Ctrl+Tab` | Next tab |
| `Ctrl+Shift+Tab` | `Ctrl+Shift+Tab` | Previous tab |
| `Ctrl+Shift+R` | `Cmd+D` | Split right |
| `Ctrl+Shift+D` | `Cmd+Shift+D` | Split down |
| `Ctrl+Alt+Arrow` | same | Resize split |
| `Ctrl+Shift+N` | `Cmd+N` | New window |

Use `Alt+Shift+Left/Right` to move the active tab. Platform conventions apply to font zoom, fullscreen, copy, paste, and settings.

## Customize bindings

User bindings live under the unified `keybinds` domain:

```jsonc
{
  "keybinds": {
    "keys": [
      {
        "key": "n",
        "with": "control | shift",
        "action": "CreateWindow"
      }
    ]
  }
}
```

A binding may specify an `action` or a raw `esc` string. `mode` can restrict it to states such as `vi`, `~vi`, or `appcursor`. A user binding wins over a default with the same trigger.

## Discover action names

Use `Alt+P` to find an action by its visible title. Configuration/action names are case-sensitive identifiers; copy an existing/default binding rather than guessing one.

See [[Getting Started/07 Essential Keybindings]] for the short first-day list.
