# Notes & Drawings

Neoism is also the workspace for project memory — the notes you're reading now live in a **vault**: a plain folder of Markdown files.

Press **Alt + N** any time to open the notes sidebar.

## The vault

- A vault is just a directory (this one is under `~/Neoism/Vaults/`). Files stay readable in any editor and easy to sync with Git, iCloud, Dropbox, or Syncthing.
- The sidebar shows folders and notes; click a folder to expand it, click a note to open it.
- Markdown renders with headings, lists, tables, task lists, syntax-highlighted code fences, and **Mermaid diagrams** (click to flip between diagram and source).

## Wiki links & backlinks

Neoism speaks **wiki links**. Type `[[` and a completion menu suggests notes; the finished link renders as clickable blue text.

```text
[[Getting Started]]           link to a note by name
[[Getting Started#First steps]]   jump to a heading
[[Editor/The Editor|The Editor]]  link a note in a subfolder, with a display name
```

Clicking a wiki link opens the target note — and **creates it if it doesn't exist yet**, so you can link forward as you think. Every note tracks its **backlinks** (what links here), and the whole vault is queryable by the [[The Neoism Agent|agent]] too, so your notes can become persistent project context.

> Tip: wiki links resolve relative to the current note's folder. Use a path like `[[Editor/The Editor|The Editor]]` to point across folders.

## Drawings

`.neodraw` files are hand-drawn sketch surfaces — Excalidraw-style — for diagrams and visual thinking, right next to your notes and code. You can even embed a drawing in Markdown with a ` ```draw ` fenced block that references a `.neodraw` file.

Code work isn't only code — it's design notes, TODOs, diagrams, bug trails, and agent context. Neoism keeps those next to the terminal and editor instead of scattered across apps.
