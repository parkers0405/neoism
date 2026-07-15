# Keybindings

Anything without a shortcut is still reachable from the **command palette** (`Alt + P`, or `Ctrl + Shift + P` — `Cmd + P` on macOS).

> **Platform note:** Linux & Windows use `Ctrl` / `Ctrl + Shift`. macOS uses `Cmd` in most of the same places. Both are listed where they differ.

## Workspace & panels

```text
Alt + E              toggle the file tree
Alt + N              toggle the notes sidebar
Alt + G              toggle the git diff panel
Alt + S              search the project           (Ctrl+Shift+F)
Alt + P              command palette              (Ctrl+Shift+P / Cmd+P)
Alt + Shift + Space  toggle Vi mode
```

## Tabs, workspaces & splits

```text
Ctrl+Shift+T         new tab                      (Cmd+T)
Ctrl+Shift+W         new top-level workspace
Ctrl+Tab / Ctrl+Shift+Tab      next / previous tab
Alt + arrows         move focus between editor, terminal & side panels
Alt+Shift+Left / Right         move the active tab
Ctrl+Shift+Left / Right        previous / next buffer tab   (Ctrl+Shift+[ / ])
Ctrl+Shift+N         new window                   (Cmd+N)
Ctrl+Shift+R         split right                  (Cmd+D)
Ctrl+Shift+D         split down                   (Cmd+Shift+D)
Ctrl+Shift+] / [     next / previous split        (Cmd+] / [)
Ctrl+Alt+Arrows      resize the focused split
```

Both **New Tab** and **New Workspace** are in the command palette (`Ctrl + P`) too.

macOS also has `Cmd+1..8` to select a tab and `Cmd+9` for the last tab.

## Font & display

```text
Ctrl+= / Ctrl++      increase font size           (Cmd+=)
Ctrl+-               decrease font size           (Cmd+-)
Ctrl+0               reset font size              (Cmd+0)
Ctrl+Shift+,         open the config file         (Cmd+,)
```

macOS: `Cmd+K` clears the screen, `Ctrl+Cmd+F` toggles fullscreen. Windows: `Alt+Enter` toggles fullscreen.

## Terminal scrollback

```text
Shift + PageUp / PageDown     scroll a page
Shift + Home / End            scroll to top / bottom
```

## Vi mode

Toggle with **Alt + Shift + Space** (or `i` / `Ctrl+C` while in it; `Esc` clears a selection). Once in Vi mode you get the familiar motions and more:

```text
h j k l   move            b w e   word motions (Shift = WORD)
0 / $     line start/end  g / G   buffer top / bottom
v         select          V       line select     Ctrl+V   block select
y         yank (copy)     /       search           n / N    next / prev match
Ctrl+B / Ctrl+F   page up / down   Ctrl+U / Ctrl+D   half page
z         center on cursor
```

## Customize your own

There's no separate keymap file — remap in [[Configuration|config.json]] under `[bindings]`:

```toml
[[bindings.keys]]
key = "t"
with = "super | shift"
action = "workspaceterminaltabcreatenew"
```

- `key`: a character or a named key (`home`, `space`, `tab`, `f1`, …).
- `with`: modifiers joined by `|` — `super`/`command`, `control`/`ctrl`, `alt`/`option`, `shift`.
- `action`: an action name (lowercased) — discover them in the command palette.
- `esc`: send a raw escape string instead of an action.
- `mode`: restrict to a mode, e.g. `vi`, `~vi`, `appcursor`.

A user binding always wins over the default with the same trigger.
