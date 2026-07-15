# The Terminal

The terminal is the center of the Neoism workspace, not a panel bolted onto an IDE.

- **GPU-rendered** text through `sugarloaf`, with pixel-smooth scrollback.
- **Workspace-aware tabs** for shells, project commands, and agents.
- **Clean copy** — terminal and hint selections copy real text instead of leaking escape sequences into chat TUIs.
- The **command palette** works from terminal, editor, or tree focus.

## Open surfaces from a terminal

Type these in any shell:

```text
:tree      / :filetree     open or focus the file tree
:buffers   / :ls / :files  open the buffer picker
:opencode                  open an OpenCode agent tab
:claude                    open a Claude agent tab
:codex                     open a Codex agent tab
```

## Shortcuts

```text
Ctrl+P  /  Cmd+;  /  Cmd+:   command palette
Alt + E                      toggle the file tree
Alt + N                      toggle the notes sidebar
Alt + S                      search the project
```

## Tabs, workspaces & navigation

A **tab** lives inside a workspace (a shell, an editor buffer, an agent). A **workspace** is a top-level Island tab across the top — its own set of tabs and splits.

```text
Ctrl+Shift+T   new tab                Ctrl+Shift+W   new top-level workspace
Ctrl+Tab / Ctrl+Shift+Tab            next / previous tab
Alt + arrows   move focus between the editor, terminal, and side panels
Alt+Shift+Left / Right               move the active tab
```

Both **New Tab** and **New Workspace** are also in the command palette (`Ctrl + P`).

## Splits

```text
Ctrl+Shift+R   split right            Ctrl+Shift+D   split down
Ctrl+Shift+] / [   next / previous split
Ctrl+Alt+Arrows    resize the focused split
```

(On macOS, `Cmd` replaces `Ctrl+Shift` for most of these — full list in [[Keybindings]].)

## Fonts, scrollback & more

Font, size, scrollback limit, copy-on-select, and the cursor are all set in [[Configuration/Configuration|config.json]]. Want a retro look? See [[Configuration/Shaders|Shaders]].

New tabs open in your active workspace, so shells, project commands, and agents share the same context.
