# Documentation notebooks

A documentation notebook is a folder with a `notebook.json` manifest. Its pages
remain ordinary Markdown files. On desktop, the collection opens in one buffer
tab; the active page is rendered and edited by the existing Markdown editor.
This is separate from executable `.ipynb` notebooks.

## Creation temporarily disabled

Notebook creation and folder-conversion entries are currently hidden from the
Notes sidebar, file explorer, and command palette. The Notes MCP server no longer
advertises or accepts `notebookCreate`. Existing notebook files, opening/navigation,
and existing-page management remain available. The creation implementation is
retained for a later design pass.

The sections below describe the retained implementation; creation instructions
are not currently exposed in the UI.

## Create or open

- Use the command palette's **Create Documentation Notebook from Folder** action.
  Supply an existing folder to collect its Markdown files, or a new folder to
  start with an `overview.md` page.
- In the file explorer, right-click a folder and choose **Turn Folder into
  Notebook**. The action never moves or replaces existing Markdown files.
- In the Notes sidebar, right-click the Add control, a note, or a folder and choose
  **New Notebook**. This immediately creates and opens `Untitled Notebook` inside
  the clicked folder (or the displayed vault when clicking empty panel space),
  with no path modal. Existing names get a numbered suffix.
- Right-click an existing Notes folder and choose **Turn Folder into Notebook**
  or **Open Notebook**. Clicking a notebook folder (or pressing Enter on it) opens
  the collection; horizontal tree navigation still expands its files.
- Use **Open Documentation Notebook**, open `notebook.json`, or activate the
  notebook folder in the explorer to open the collection.
- Nested folders become named sections. Hidden files, symlinks, and nested
  notebooks are not imported. Large directory imports are bounded; choose a
  documentation folder rather than an entire dependency/build tree.

## Reading and editing

The left rail lists ordered pages and collapsible sections. The center uses the
normal Markdown renderer, including editing and vim modes. The existing heading
outline occupies the right gutter when there is sufficient space.

Previous/Next follows page order. Back/Forward follows navigation history.
Notebook-member links navigate inside the same tab. Following a link outside the
collection uses the normal file-opening behavior.

Visited pages retain their live document buffers, undo history, caret, and scroll
position. An asterisk marks edited pages. The notebook tab remains modified while
any of its visited pages is dirty, and closing it is blocked until those pages
are saved. Save works on the current page, not implicitly on every page.
The last visited page is remembered locally under Neoism's configuration directory
and restored when the notebook is reopened. Per-page caret and scroll positions
are retained during the open session, not serialized by the notebook feature.

The rail includes **Add page**, **Link existing file**, and **Move up/down**.
Existing files outside the folder or vault are referenced, not copied. Missing
pages produce a notification instead of silently creating a replacement.
File/folder moves through Neoism update references in open notebooks. External
filesystem changes are not automatically repaired.

## Manifest

```json
{
  "title": "Agent Architecture",
  "pages": [
    "overview.md",
    {
      "path": "internals/prompt-admission.md",
      "title": "Prompt admission and API",
      "section": "Internals"
    },
    "../reference/security.md"
  ]
}
```

Relative references start at the notebook folder. Absolute paths are accepted but
are machine-specific. The manifest must have a nonempty title and at least one
Markdown page. Duplicate normalized references are rejected. The name
`notebook.json` is reserved for this collection manifest when opening files via
the normal desktop file-opening path.

Page-list changes are staged beside the manifest and renamed into place. If the
manifest has been edited by another process since it was loaded, Neoism refuses
to overwrite it; close and reopen the notebook before changing its page list.

## Links to files outside vaults

Use **Link to Markdown File** from the command palette while editing an ordinary
Markdown page or a notebook page. Enter an existing absolute path, a `~/` path,
a file URL, or a path relative to the source document. Neoism inserts a standard
Markdown link at the cursor. It uses relative links within the source folder and
file URLs for other locations; file URLs are machine-specific.

You can also write links directly:

```md
[Prompt admission and API](#prompt-admission-and-api)
[Project notes](../../projects/my-app/docs/overview.md)
[Architecture](file:///home/user/projects/my-app/docs/architecture.md)
[[../../projects/my-app/docs/architecture.md|Architecture]]
```

Same-page heading links resolve against the open document, including unsaved
headings. File paths and heading fragments support percent-encoded characters.
Cross-file heading lookup still reads the destination from disk.

## Agent Notes MCP

The desktop-provided `notes` MCP server exposes the same notebook format and
creation implementation as the UI:

- `notebookList`: discover notebooks in the linked vault.
- `notebookCreate`: create a new folder notebook or convert an existing folder.
- `notebookRead`: read its title and ordered page references.
- `notebookAddPage`: create a Markdown page or reference an existing vault file.
- `notebookMovePage`: move a one-based page position up/down without moving files.

Example tool arguments:

```json
{"path":"Architecture","title":"Agent Architecture"}
```

Pass that to `notebookCreate`, then call `notebookAddPage` with:

```json
{"notebook":"Architecture","title":"API","content":"# API\n\nAPI notes here.\n"}
```

MCP paths are relative to the workspace's linked Notes vault. An `existing_path`
for `notebookAddPage` is vault-relative, not notebook-relative; this can reference
a file outside the notebook folder, but not outside the vault. Symlink escapes
are rejected. The desktop's explicit file-linking UI still supports outside-vault
files. Notebook discovery/read are allowed in agent plan mode; notebook mutations
are denied there, just like other Notes writes.

Agent edits to the manifest of an already-open notebook are protected from being
overwritten by the UI; reopen the notebook to reload that page list.

## Initial scope

This first integration is desktop/local-workspace only. The manifest and
navigation renderer live in the shared crate, but joined-workspace transport and
web hosting are not wired yet. Their palette actions are hidden on web.

Page ordering currently uses Move up/down, not drag-to-reorder. Moving the whole
notebook tab between splits/windows is disabled until all of its live page
contexts can travel as one group. A page already open in another notebook is
rejected rather than creating a second independent editable buffer.

Notebook-wide search, a graphical file chooser, explicit standalone-page views,
and special notebook folder icons in the notes sidebar remain follow-up work.
