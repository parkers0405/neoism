# Notes and Drawings

Neoism notes are ordinary Markdown files stored in vault folders. Neoism indexes them for fast navigation and graph operations without converting them into a proprietary database.

## Vaults and workspace links

The Default vault lives under `~/Neoism/Vaults/Default` unless `NEOISM_NOTES_HOME` relocates the vault root. A code workspace can link to one vault through `.neoism/workspace.json`; the vault's `project.json` records associated code directories.

Use `Alt+N` to open the Notes sidebar. The sidebar shows the active/viewed vault hierarchy and opens notes in workspace buffers.

## Links, headings, tags, and tasks

Neoism indexes:

```markdown
[[Another Page]]
[[Another Page#Heading]]
[[@project/Page]]
#architecture
- [ ] unfinished task
- [x] completed task
```

Backlinks show which notes reference a page. Search can find page text, tags, headings, and tasks. The note graph summarizes links among indexed Markdown files.

## Markdown editing

Notes use Neoism's Markdown editor and renderer. Source remains Markdown on disk. Wiki links and project links are navigation syntax layered on normal files; external Markdown tools can still read the content.

## Drawings

`.neodraw` is Neoism's hand-drawn diagram format. A drawing opens in a native sketch editor and can be referenced from Markdown with a `draw` code fence that points to the `.neodraw` file.

Keep drawing files next to related notes or in a clear project folder so links remain portable.

## Notes MCP

The built-in Notes MCP lets agents list, read, write, search, inspect tags/headings/tasks, follow backlinks, and summarize the graph. Tool actions operate on the same vault files visible in the sidebar.

Notes MCP and Memory MCP are different: Notes handles user documents; Memory maintains compact agent recall topics inside `Memory/`. See [[Neoism Agent/Memory]].

## Sync and privacy

Vault synchronization follows Neoism's notes/sync architecture. Linked-project scope prevents a connected guest from automatically browsing sibling vaults. Pairing and host authorization remain separate from note indexing.

See [[Neoism Daemon/Multiplayer and Sync]] for device behavior.

## Troubleshooting

- Missing page: confirm the viewed vault and note path.
- Missing backlinks: ensure the target/link spelling matches and let the index refresh.
- Missing project completion: check the workspace-vault link metadata.
- Broken drawing embed: verify the referenced `.neodraw` relative path.
