# Documentation UI test

This sample document introduces Neoism and provides a small reading and editing fixture for the documentation UI. It includes headings, inline formatting, lists, links, a table, and fenced code blocks so you can inspect how different kinds of content appear together.

## Project overview

Neoism is a terminal-first IDE built around a persistent workspace. It brings terminal sessions, native code and Markdown editing, Git tooling, notes, and AI agents into one application.

The workspace is more than a single window. A background daemon owns workspace services, while desktop and browser clients provide ways to interact with them. This separation is the foundation for reconnecting to sessions and working across devices.

## Explore the workspace

- Open a terminal to run project commands.
- Browse source files and documentation in the file explorer.
- Edit Markdown alongside code without leaving the workspace.
- Review changes in the Git panel.
- Use the agent pane for tool-assisted development and research.

For the repository overview, see the [project README](../README.md). For the collection format, see [Documentation notebooks](documentation-notebooks.md).

## Reading and editing

A useful documentation page should be comfortable to scan and straightforward to edit. **Bold text** highlights important details, *italic text* adds emphasis, and inline code such as `notebook.json` distinguishes filenames and identifiers from prose.

This deliberately longer paragraph provides a wrapping sample. Resize the editor pane to see how its lines adapt, then move the caret through the text and scroll past the next heading. The paragraph stays on one source line so the editor, rather than manual line breaks, controls its visual layout. You can also select a phrase, undo an edit, and return to the same location to inspect caret and selection rendering.

> UI test note: this is sample content, not a promise that every feature described in linked documentation is available on every platform.

### Navigation checklist

1. Open this file from the `docs` folder.
2. Move between headings using the document outline, if available.
3. Follow the link to the notebook documentation.
4. Return to this page and inspect the restored position.
5. Resize the pane and check the table and code blocks below.

### Editing checklist

- [ ] Insert text into an existing paragraph.
- [ ] Add an item to this checklist.
- [ ] Toggle a checkbox.
- [ ] Undo and redo an edit.
- [ ] Save the document and check the modified indicator.

## Workspace components

| Component | Purpose | Example interaction |
| --- | --- | --- |
| Terminal | Run shell commands in a real PTY | Inspect repository status |
| Code editor | Read and modify source files | Navigate to a symbol |
| Markdown editor | Write notes and documentation | Edit this page |
| Git panel | Inspect and manage changes | Review a file diff |
| Agent runtime | Coordinate models, tools, and permissions | Ask about the project |
| Workspace daemon | Host persistent workspace services | Connect a client |

## Notebook manifest example

A documentation notebook groups ordinary Markdown files using a `notebook.json` manifest. The following is an illustrative manifest, not an active notebook created by this test page.

```json
{
  "title": "Neoism documentation preview",
  "pages": [
    {
      "path": "documentation-ui-test.md",
      "title": "Project overview",
      "section": "Getting started"
    },
    {
      "path": "documentation-notebooks.md",
      "title": "Documentation notebooks",
      "section": "Reference"
    }
  ]
}
```

Notebook creation commands are currently hidden, as described in the [notebook documentation](documentation-notebooks.md#creation-temporarily-disabled). This sample does not change that behavior.

## Code rendering

This Rust snippet is only a syntax-highlighting fixture. It does not call a Neoism API or modify the workspace.

```rust
struct WorkspaceSummary<'a> {
    name: &'a str,
    pages: usize,
}

fn main() {
    let workspace = WorkspaceSummary {
        name: "Documentation preview",
        pages: 2,
    };

    println!("{}: {} pages", workspace.name, workspace.pages);
}
```

A shorter shell block provides a second language sample:

```sh
git status --short
git diff -- docs/documentation-ui-test.md
```

## Link rendering

- [Return to the project overview](#project-overview)
- [Jump to the navigation checklist](#navigation-checklist)
- [Read the notebook manifest specification](documentation-notebooks.md#manifest)
- [Open the Rust workspace manifest](../Cargo.toml)

---

## End of sample

This final section provides a clear scroll destination. Return to [Reading and editing](#reading-and-editing) to test a same-page heading link, or make a small edit here to inspect the document's saved and modified states.
