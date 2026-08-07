# Essential Keybindings

The command palette is the universal fallback: use `Alt + P`, `Ctrl + Shift + P`, or `Cmd + P` on macOS, then search for an action by name.

Linux and Windows generally use `Ctrl` or `Ctrl + Shift`; macOS uses `Cmd` for the corresponding application shortcuts.

## Workspace and panels

| Keys | Action |
|---|---|
| `Alt + E` | Toggle the file tree |
| `Alt + N` | Toggle the notes sidebar |
| `Alt + G` | Toggle the Git diff panel |
| `Alt + S` | Search the project (`Ctrl + Shift + F` also works) |
| `Alt + A` | Open a new agent pane |
| `Alt + P` | Open the command palette |
| `Alt + Arrow` | Move focus between panes and side panels |
| `Alt + Shift + Space` | Toggle Vi mode |

## Tabs, workspaces, and splits

| Linux / Windows | macOS | Action |
|---|---|---|
| `Ctrl + Shift + T` | `Cmd + T` | New terminal tab |
| `Ctrl + Shift + W` | `Ctrl + Shift + W` | New top-level workspace |
| `Ctrl + Tab` | `Ctrl + Tab` | Next tab |
| `Ctrl + Shift + Tab` | `Ctrl + Shift + Tab` | Previous tab |
| `Ctrl + Shift + R` | `Cmd + D` | Split right |
| `Ctrl + Shift + D` | `Cmd + Shift + D` | Split down |
| `Ctrl + Shift + ]` / `[` | `Cmd + ]` / `[` | Next / previous split |
| `Ctrl + Alt + Arrow` | `Ctrl + Alt + Arrow` | Resize the focused split |
| `Ctrl + Shift + N` | `Cmd + N` | New window |

Use `Alt + Shift + Left` / `Right` to move the active tab. macOS also supports `Cmd + 1` through `Cmd + 8` to select a tab and `Cmd + 9` for the last tab.

## Editor and display

| Linux / Windows | macOS | Action |
|---|---|---|
| `Ctrl + Shift + Left` / `Right` | same | Previous / next editor buffer tab |
| `Ctrl + =` or `Ctrl + +` | `Cmd + =` | Increase font size |
| `Ctrl + -` | `Cmd + -` | Decrease font size |
| `Ctrl + 0` | `Cmd + 0` | Reset font size |
| `Ctrl + Shift + ,` | `Cmd + ,` | Open `config.json` |

On macOS, `Cmd + K` clears the terminal screen and `Ctrl + Cmd + F` toggles fullscreen. On Windows, `Alt + Enter` toggles fullscreen.

## Terminal scrollback

| Keys | Action |
|---|---|
| `Shift + PageUp` / `PageDown` | Scroll one page |
| `Shift + Home` / `End` | Scroll to top / bottom |

## Customize a binding

Bindings live in the `keybinds` domain of `config.json`; there is no separate keymap file. A user binding wins over the default with the same trigger.

```jsonc
{
  "keybinds": {
    "keys": [
      {
        "key": "n",
        "with": "control | shift",
        "action": "CreateWindow",
      },
    ],
  },
}
```

A binding can use `action` for a Neoism action or `esc` for a raw escape sequence. `mode` can restrict a binding to states such as `vi`, `~vi`, or `appcursor`.

When learning Neoism, remember just five entries: `Alt + P` for everything, `Alt + E` for files, `Alt + N` for notes, `Alt + A` for an agent, and `Alt + Arrow` to move focus.