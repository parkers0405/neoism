# Meet Neoism

Neoism is a terminal-first workspace for code, Markdown notes, agents, and work shared across devices. It keeps those activities in one place without reducing the terminal to a small panel inside a conventional IDE.

## The workspace at a glance

A Neoism window can contain:

- **Terminal tabs** backed by real PTYs, with shell programs, scrollback, links, search, and terminal applications.
- **Native editor buffers** for code and Markdown, including diagnostics, LSP actions, search, formatting, and an optional Vim-style editing layer.
- **Project navigation** through the file tree (`Alt + E`), project search (`Alt + S`), and the Git diff panel (`Alt + G`).
- **Notes vaults** made from ordinary folders and Markdown files, available from the notes sidebar (`Alt + N`).
- **Agent panes** whose sessions belong to the current workspace and can use tools, inspect the project, and request permission before consequential actions.
- **Splits and top-level workspaces** so several terminals, files, notes, and agent sessions can remain visible without becoming unrelated windows.

Neoism's desktop and web experiences share workspace state through `neoism-workspace-daemon`. The daemon owns durable workspace concerns such as PTYs, layouts, pairing, and remote sessions; the desktop app remains the native client.

## One directory is one workspace

The declared workspace directory is the project boundary. The file tree is rooted there, editor and agent actions use it as project context, and new terminal tabs begin there. Changing directory inside one shell does **not** silently repoint the rest of Neoism.

This distinction keeps navigation predictable: a shell can temporarily visit another directory while the workspace, file tree, notes links, and agents continue to refer to the project you opened.

## A useful first tour

1. Open a project directory as a workspace.
2. Toggle the file tree with `Alt + E` and open a file.
3. Create a terminal tab with `Ctrl + Shift + T` (`Cmd + T` on macOS).
4. Open notes with `Alt + N`.
5. Open an agent pane with `Alt + A` and give it a concrete task in the current project.

If you do not know where an action lives, open the **command palette** with `Alt + P`, `Ctrl + Shift + P`, or `Cmd + P` on macOS, then search by name. The palette also exposes actions that have no default shortcut.

## Files stay yours

Code remains in the directory you selected. Notes remain plain Markdown in a vault folder. Workspace metadata is stored separately in `.neoism/workspace.json`, which Neoism writes automatically. You can continue to use normal shell tools, Git, and other editors alongside Neoism.

Next: [[02 Open Your First Workspace|Open Your First Workspace]].