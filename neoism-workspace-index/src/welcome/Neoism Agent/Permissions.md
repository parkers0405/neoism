# Permissions

Permissions decide whether an agent operation is allowed, denied, or must wait for you. They are evaluated before side-effecting tools run.

## Actions

| Action | Behavior |
|---|---|
| `allow` | Run without prompting. |
| `ask` | Pause and emit a permission request. |
| `deny` | Refuse the operation. |

An unmatched operation defaults to `ask`.

## Configure rules

A string applies to every target for one permission:

```jsonc
{
  "agent": {
    "permission": {
      "read": "allow",
      "edit": "ask",
      "external_directory": "deny"
    }
  }
}
```

An object applies actions to target patterns:

```jsonc
{
  "agent": {
    "permission": {
      "bash": {
        "git status": "allow",
        "git diff*": "allow",
        "git push*": "ask",
        "rm -rf*": "deny"
      },
      "read": {
        "*": "allow",
        "*.env": "deny",
        "*.env.example": "allow"
      }
    }
  }
}
```

Patterns support `*` and `?`. `~/` and `$HOME/` are expanded in configured patterns. Rules are evaluated from last to first, so the last matching rule wins. Put broad rules before narrow exceptions.

## Permission names

Tools can expose their own permission names. Important native groups include:

- `edit` for `edit`, `write`, and `apply_patch`.
- `bash` for shell commands.
- `read` for file reads.
- `external_directory` for paths outside the workspace boundary.
- `task` for subagent delegation.
- `webfetch` and `websearch` for network research.
- `question` and `plan_enter` for interactive workflow controls.
- `doom_loop` when the runtime detects repeated tool behavior.

Use `"*"` as a fallback permission rule.

## Agent-specific rules

Agent rules are applied after global rules:

```jsonc
{
  "agent": {
    "permission": { "*": "ask", "read": "allow" },
    "agent": {
      "explore-local": {
        "description": "Read-only explorer",
        "mode": "subagent",
        "permission": { "*": "allow", "edit": "deny", "bash": "deny" }
      }
    }
  }
}
```

## Respond to a request

A permission card identifies the tool, permission, and target. You can deny it, allow the current request, or allow the supplied target pattern for subsequent calls. Remembered grants are attached to the active runtime/session; permanent project policy belongs in configuration.

Permission requests from child agents include their source session, title, parent session, and agent name so the UI can route your answer correctly.

## YOLO mode

`/yolo` toggles the runtime's skip-permissions mode. The equivalent configuration is:

```jsonc
{
  "agent": {
    "dangerously-skip-permissions": true
  }
}
```

This converts undecided and `ask` operations to allow. An explicit matching `deny` still wins. This option is intentionally dangerous: shell commands, edits, external paths, and network tools can create irreversible side effects.

## Tools versus permissions

The `tools` map controls whether a tool is available at all. Permissions control whether an available invocation may run:

```jsonc
{
  "agent": {
    "tools": { "websearch": false },
    "permission": { "bash": "ask" }
  }
}
```

A disabled tool cannot be recovered by an `allow` permission.

See [[Tools and Background Tasks]] and [[Agents and Subagents]].