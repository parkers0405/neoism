# Instructions

Instructions establish persistent rules and project context for agents. Neoism combines built-in runtime instructions with global, project, nearby-file, configured, and per-agent prompt text.

## Automatic instruction files

Neoism recognizes these names:

```text
AGENTS.md
CLAUDE.md
CONTEXT.md
```

For project instructions, Neoism walks the directory ancestry. It chooses the first filename family that exists and loads matching files from broad parent scope toward the active directory.

`AGENTS.md` is the recommended cross-agent project format.

## Global instructions

Neoism checks:

```text
<Neoism config directory>/AGENTS.md
~/.claude/CLAUDE.md
```

The first existing global candidate is loaded.

## Configured files

Add specific files in the Agent domain:

```jsonc
{
  "agent": {
    "instructions": [
      "AGENTS.md",
      "docs/architecture.md",
      "~/company/security.md"
    ]
  }
}
```

Relative paths are searched upward from the workspace and then resolved against it. Absolute and `~/` paths are supported. HTTP URLs are not loaded by this instruction-file path.

## Nearby instructions

When a tool works with a file in a nested directory, Neoism can load instruction files between the workspace root and that file. This lets a subsystem carry narrower guidance without polluting every unrelated operation.

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