# Instructions

Instructions establish persistent rules and project context for agents. Neoism follows OpenCode v2's instruction-file semantics while using Neoism's own configuration locations and `agent.instructions` setting.

## Automatic instruction files

Neoism automatically recognizes one filename:

```text
AGENTS.md
```

Neoism loads the global file first, then every `AGENTS.md` from the session's workspace directory upward toward the home directory. For workspaces outside home, discovery stops at the project root. The project boundary is the Git worktree root, or the workspace itself for a non-Git directory. A workspace outside its project root receives only global automatic instructions.

Files are combined nearest-first, not broadest-first. For a workspace at `project/packages/web`, the project order is `packages/web/AGENTS.md`, `packages/AGENTS.md`, then `AGENTS.md`, followed by any eligible ancestors toward home. Commit project instruction files so collaborators receive the same guidance.

`AGENT.md`, `CLAUDE.md`, and `CONTEXT.md` are not automatic fallbacks. You can still name any file explicitly in `agent.instructions`.

## Global instructions

The global file is:

```text
<Neoism config directory>/AGENTS.md
```

This is normally `~/.config/neoism/AGENTS.md` on Linux. `.neoism/AGENTS.md` inside a project is not a fallback global file.

## Configured files

Add specific files in the Agent domain:

```jsonc
{
  "agent": {
    "instructions": [
      "CONTRIBUTING.md",
      "docs/guidelines/*.md",
      "~/company/security.md",
      "https://example.com/instructions.md"
    ]
  }
}
```

Configured sources load after automatic `AGENTS.md` files, in listed order. Instruction arrays from configuration layers are combined and duplicate files are deduplicated.

| Entry | Resolution |
| --- | --- |
| Relative file or glob | Search each directory from workspace through project root, nearest-first |
| `~/` file or glob | Resolve from home |
| Absolute file or glob | Use directly |
| HTTP(S) URL | Fetch on instruction configuration refresh, with a five-second timeout |

Local file contents are checked before model requests. New files matching a glob are discovered when the configuration changes. Missing files are skipped. A failed URL refresh preserves the last successfully fetched contents, if any. Neoism does not automatically follow links or file references inside instruction text.

## Disabling project configuration

Set `NEOISM_DISABLE_PROJECT_CONFIG=1` to disable project configuration and automatic project/nested instruction discovery while keeping global instructions. Relative configured entries then resolve against the global config directory. Relative configured entries also use that directory when the workspace is outside its project root.

## Nearby instructions

Reading a file or listing a directory below the workspace discovers `AGENTS.md` files between that target and the workspace. They enter model-visible history at discovery time, nearest-first, rather than being moved into the initial system prompt.

Nested instructions are deduplicated while their original entry remains in model-visible history. Repeated reads do not replace already-loaded text, even if the nested file changed. If compaction or a revert removes the entry, a later read can load the file again. Start a new session when a nested instruction edit must take effect immediately.

## Live updates

Global, upward-discovered, and configured local instruction files are checked before model requests, including follow-up steps within an active run. Changes become instruction updates in conversation order. Removing all ambient files explicitly invalidates the previous ambient instructions; a temporary read failure preserves the last known contents instead of treating the file as deleted.

Moving a session preserves its instruction state and introduces destination guidance as an update. Committing a revert starts a fresh ambient instruction baseline before the next prompt. Automatically loaded instruction contents and caches are private server state: client metadata exposes source information, not the instruction text. Explicit file reads still return ordinary file contents.

## Per-agent prompt

An agent definition can add a prompt:

```jsonc
{
  "agent": {
    "agent": {
      "review": {
        "description": "Review changes",
        "mode": "subagent",
        "prompt": "Report findings first. Do not edit files."
      }
    }
  }
}
```

## Precedence is additive

Instructions are normally concatenated, not treated as one winning scalar value. More specific instructions can refine broader ones, but contradictory instructions create ambiguity. Keep global files general and project files concrete.

System/runtime safety rules remain higher priority than repository instructions. A project file cannot bypass tool permissions or platform policy.

## What to include

Good instructions describe:

- Repository layout and source-of-truth directories.
- Verification commands and commands that must not be run.
- Editing conventions that are not obvious from code.
- Architecture boundaries.
- Release and migration rules.
- Security constraints.

Avoid copying large API references or transient task state into `AGENTS.md`. Put reusable procedures in [[Skills]] and durable project facts in [[Memory]].