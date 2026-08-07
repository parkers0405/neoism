# Agents and Subagents

An agent is a named execution profile: instructions, model settings, tool switches, and permissions. A session chooses one primary agent, while the task tool creates child sessions using subagents.

## Built-in agents

| Name | Mode | Intended use |
|---|---|---|
| `build` | Primary | Normal implementation work. |
| `plan` | Primary | Read-oriented investigation and planning. |
| `general` | Subagent | Broad delegated research or multi-step work. |
| `explore` | Subagent | Fast read-only codebase discovery. |

Use `/agent` to switch the current session.

## Define an agent

```jsonc
{
  "agent": {
    "agent": {
      "review": {
        "description": "Review code without modifying it",
        "mode": "subagent",
        "model": "anthropic/claude-sonnet-4-5",
        "variant": "high",
        "prompt": "Find correctness bugs, regressions, and missing tests.",
        "temperature": 0.2,
        "max-steps": 20,
        "tools": { "write": false, "edit": false, "apply_patch": false },
        "permission": { "*": "allow", "edit": "deny", "bash": "ask" }
      }
    }
  }
}
```

`mode` may be `primary`, `subagent`, or `all`. Other fields include `top-p`, `hidden`, `disable`, and optional UI `color` metadata.

## Delegate work

The native `task` tool accepts `subagent_type`, a real `prompt`, a short `description`, `background`, and optionally a previous `task_id`. Use `general` for broad work and `explore` for read-only discovery.

External ACP-backed agents only appear when explicitly configured and available. They are separate runtimes, not aliases for native agents.

## Background subagents

With `background: true`, the parent continues and Neoism reports completion later. Use `task_result` with the returned task ID, reuse that ID to continue the child, and use `stop_task` to cancel child work.

Child sessions have independent histories. They receive the delegated prompt, not an automatic copy of every parent message. Parallel editing agents can collide, so delegate disjoint files or use read-only discovery agents.

## Permissions

Effective subagent permissions combine global rules and selected-agent rules. A restrictive agent can deny editing even when global configuration allows it. See [[Permissions]].

`/subagents` lists associated child work. See [[Sessions and Sharing]] and [[Tools and Background Tasks]].