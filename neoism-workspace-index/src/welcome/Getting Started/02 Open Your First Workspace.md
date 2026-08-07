# Open Your First Workspace

A Neoism workspace is a directory chosen explicitly as the project root. Open the directory that contains the code and files you want the terminal, editor, search, notes links, and agents to share.

## Create or select a workspace

Open the command palette (`Alt + P`, `Ctrl + Shift + P`, or `Cmd + P` on macOS) and choose **New Workspace**. Its default shortcut is `Ctrl + Shift + W`.

Select the project directory when prompted. Neoism then uses that declared directory as the workspace root:

- the file tree starts there;
- new terminal tabs start there;
- project search stays inside it;
- editor and agent requests receive the same project context;
- `.neoism/workspace.json` records the workspace identity and notes settings.

Neoism remembers workspace entries, so an existing workspace can be selected again rather than recreated.

## Verify that you opened the right directory

Toggle the file tree with `Alt + E`. The root row should name the directory you selected, and its children should be the project's files. Open a file from the tree to place it in the native editor.

If you selected a parent directory or the wrong repository, create/select a workspace for the intended directory. Running `cd` in a terminal changes only that terminal's working directory; it intentionally does not change the workspace root.

## Tabs, workspaces, and splits are different

- A **terminal tab** is another shell in the active workspace: `Ctrl + Shift + T` (`Cmd + T` on macOS).
- A **top-level workspace** has its own declared directory and session tree: `Ctrl + Shift + W`.
- A **split** shows another pane beside or below the current pane. Use **Split Right** or **Split Down** from the command palette; defaults are `Ctrl + Shift + R` / `Ctrl + Shift + D` on Linux and Windows, and `Cmd + D` / `Cmd + Shift + D` on macOS.

Use `Alt + Arrow` to move focus among the terminal, editor, and side panels. Use `Ctrl + Tab` and `Ctrl + Shift + Tab` for the next and previous tab.

## Open files and search the project

- `Alt + E` toggles the file tree.
- `Alt + S` opens project search (`Ctrl + Shift + F` is also available).
- `Alt + G` toggles the Git diff panel.
- Editor actions such as **Go to Definition**, **Find References**, **Rename Symbol**, formatting, diagnostics, and inlay hints are available through the command palette when supported by the file's language server.

The workspace is now the common context for the next parts of the tour.

Next: [[03 Terminal, Editor, and Notes|Terminal, Editor, and Notes]].