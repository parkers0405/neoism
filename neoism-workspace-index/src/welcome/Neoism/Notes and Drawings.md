# Notes and Drawings

Neoism notes are ordinary Markdown files stored in vault folders. Neoism indexes them for fast navigation and graph operations without converting them into a proprietary database.

## Vaults and workspace links

The initial vault is named `Default` and lives under `~/Neoism/Vaults/Default` unless `NEOISM_NOTES_HOME` relocates the vault root. You can rename it like any other vault; Neoism tracks which vault is the default by stable identity rather than requiring that folder name.

A code workspace can link either to a whole vault or to any folder nested inside one. The workspace's `.neoism/workspace.json` caches the selection, while the owning vault's `project.json` records associated code directories and their relative notes folders. Existing links use the whole vault.

Use `Alt+N` to open the Notes sidebar. For a linked project it opens the linked folder automatically; an unlinked directory opens the current default vault. Choosing another vault remains a viewing override and does not silently relink the code project.

Right-click any vault folder to link the current or another code project there. From **Open Vaults Root**, dragging a standalone linked vault into a different vault offers to convert it into a nested project folder while preserving its code links.

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

Neoism Desktop automatically provides its Agent sessions with a vault-backed Notes MCP. Its seven tools list, search, read, create, and write Markdown notes, list tasks, and toggle tasks. Tool actions use the workspace's linked project folder, falling back to the default vault for an unlinked workspace. Standalone and hosted Neoism Agent instances only receive Notes when their host supplies a Notes MCP.

Notes and Agent memory are different: Notes handles user documents in vaults; the native `memory` tool maintains compact project recall under the declared workspace's `.neoism/memory` directory. See [[Neoism Agent/Memory]].

## Sync and privacy

Vault synchronization follows Neoism's notes/sync architecture. Linked-project scope prevents a connected guest from automatically browsing sibling vaults. Pairing and host authorization remain separate from note indexing.

See [[Neoism Daemon/Multiplayer and Sync]] for device behavior.

## Troubleshooting

- Missing page: confirm the viewed vault and note path.
- Missing backlinks: ensure the target/link spelling matches and let the index refresh.
- Missing project completion: check the workspace-vault link metadata.
- Broken drawing embed: verify the referenced `.neodraw` relative path.
