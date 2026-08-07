# Workspaces

A Neoism workspace is an explicitly selected directory plus the UI layout attached to it. The workspace root belongs to Neoism and the workspace daemon; it is not inferred from whichever directory one shell happens to use.

## Open a workspace

Create or open a workspace from the workspace picker or command palette. The selected directory becomes the root for Explorer, project search, editor paths, agent context, and new terminal tabs.

Running `cd` changes only that terminal's current directory. It does not move Explorer or silently redefine the workspace.

## Workspace tabs and buffer tabs

The top-level strip represents workspaces. Each workspace contains its own pane layout and open content. The buffer-tab strip represents files/content inside the focused workspace or pane.

Closing them has different scope:

- `:q` closes the focused buffer and refuses unsaved changes.
- `:q!` discards and closes the focused buffer.
- `:qall` closes the active workspace, not every workspace in the window.
- `:qall!` discards dirty buffers in the active workspace and closes it.

The workspace tab context menu exposes **Close Workspace**, **Close Other Workspaces**, **Close Workspaces to the Right**, **Copy Path**, **Open in New Window**, and related actions where applicable.

## Panes and splits

A workspace can split right or down. Panes can host terminals, files, Markdown, drawings, notebooks, and agents. Use `Alt+Arrow` to move focus across panes and side panels.

Tabs can be dragged between split targets. A workspace tab can also be detached into another OS window without moving its PTY/session to a different process.

## Explorer and project search

Toggle Explorer with `Alt+E`. It is rooted at the declared workspace directory. Open files, create entries, rename, and navigate folders without changing terminal working directories.

Use `Alt+S` for project search. Finder modes provide file and word search; results open in the active workspace.

## Command palette and side panels

`Alt+P` opens the command palette. It is the universal route to actions that do not have a memorable shortcut.

Workspace side panels include:

- Explorer: `Alt+E`
- Notes: `Alt+N`
- Git changes: `Alt+G`

Side panels participate in the same focus chain as panes.

## Unsaved content

Modified buffer tabs display dirty state. Normal close commands refuse to discard it. Workspace close checks only buffers belonging to that workspace, so an unrelated dirty workspace does not block the active one.

## Persistence

The daemon stores workspace records and shared layout state. Terminal PTYs are live host processes; see [[Neoism Daemon/Sessions and Persistence]] for exactly what survives disconnects and restarts.

## Relevant configuration

Workspace chrome and tab behavior live under the `ui` domain:

```jsonc
{
  "ui": {
    "navigation": {
      "hide-if-single": true
    }
  }
}
```

Use Neoism's settings UI for discoverable options and preserve every setting inside its domain block.
