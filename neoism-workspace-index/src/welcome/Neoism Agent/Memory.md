# Memory

Neoism Memory stores durable facts in Markdown files inside a vault. It is designed for information that should survive session compaction, a new conversation, or a different connected device.

Memory is not hidden model state. You can open, edit, link, search, and delete the files yourself.

## Project and user memory

| Scope | Use it for | Location |
|---|---|---|
| `project` | Architecture, bugs, feature status, project workflows, technical decisions | The linked project's vault `Memory/`, or the Default vault when no vault is linked |
| `user` | Facts about the person: durable preferences, personal environment, recurring personal workflow | Default vault `Memory/Personal/` |
| `auto` / `all` | Recall from both scopes | Both roots |

Never store project facts in user memory merely because one user mentioned them.

## Structure

Each root contains a compact `MEMORY.md` index and detailed topic files:

```text
Memory/
├── MEMORY.md
├── bug_session_replay.md
├── feature_background_tasks.md
├── project_workspace_model.md
└── feedback_no_release_builds.md
```

`MEMORY.md` should contain one link and one-line summary per topic. Details belong in topic files. Neoism injects a bounded version of populated memory indexes into the agent's system context, then the agent reads a linked topic when needed.

## Operations

The built-in Memory MCP exposes:

| Operation | Purpose |
|---|---|
| `memory_init` | Create memory folders and indexes. |
| `memory_list` | List topic files. |
| `memory_read` | Read one topic by relative path. |
| `memory_recall` | Semantic/keyword search across indexes and topics. |
| `memory_write` | Write/update a topic and keep the index compact. |

Recall uses semantic ranking when embeddings are available and falls back to keyword matching.

## Write memory

A memory write includes a stable filename, name, description, type, origin, timestamps, and Markdown body. Supported organizational types include:

```text
project  feedback  bug  feature  reference  perf
preference  workflow  personal
```

Use descriptive filenames such as `bug_stale_session_attach.md`, not dates or conversation IDs.

## What belongs in memory

Good candidates:

- A confirmed architecture decision.
- A subtle bug root cause and its invariant.
- A durable user preference.
- A feature's real implementation status.
- A workflow future agents must follow.

Poor candidates:

- Temporary task progress already visible in the current session.
- Secrets or credentials.
- Guesses that have not been verified.
- Whole transcripts.
- Information already easy to discover from code and unlikely to matter again.

## Memory versus notes

Notes are user knowledge and documents. Memory is a compact agent recall layer implemented using notes-compatible Markdown. A project plan can be a normal note; the invariant learned while implementing it may become a memory topic.

## Cross-device behavior

Because project memory is stored in a linked vault, it follows that vault's normal sync behavior. User memory lives in the Default vault. The Agent server reads the same files regardless of which attached Neoism client displays the session.

See [[Instructions]], [[Skills]], and [[Compaction]].