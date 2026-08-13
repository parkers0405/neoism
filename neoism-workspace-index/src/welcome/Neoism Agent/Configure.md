# Configure Neoism Agent

Neoism Agent reads the Agent domain from Neoism's unified JSONC configuration and then applies project-level agent configuration for the active workspace.

## Global configuration

Neoism's application configuration uses domain blocks. Agent settings belong under `"agent"`:

```jsonc
{
  "agent": {
    "model": "anthropic/claude-sonnet-4-5",
    "small-model": "anthropic/claude-haiku-4-5",
    "default-agent": "build",
    "variant": "high",
    "text-verbosity": "low",
    "instructions": ["AGENTS.md"],
    "permission": {
      "edit": "ask",
      "bash": "ask"
    }
  }
}
```

Do not place `model`, `permission`, or `mcp` beside `appearance` and `terminal`. The desktop passes the contents of the top-level `agent` block to Neoism Agent.

## Project configuration

A workspace may contain `neoism.json` or `neoism.jsonc`. These files are agent-specific, so their keys are direct:

```jsonc
{
  "model": "anthropic/claude-sonnet-4-5",
  "default-agent": "build",
  "instructions": ["AGENTS.md", "docs/engineering.md"],
  "permission": {
    "bash": {
      "git status": "allow",
      "git push*": "ask"
    }
  }
}
```

Neoism walks from the filesystem root toward the active workspace directory and merges project configuration in that order. A closer project file overrides a more distant one.

## Precedence

Configuration is merged from lower to higher precedence:

1. Global configuration directories.
2. The unified Neoism `agent` block supplied by the desktop.
3. Project configuration discovered from parent directories toward the workspace.
4. Explicit runtime overrides, including `NEOISM_AGENT_CONFIG` and `NEOISM_AGENT_CONFIG_CONTENT`.
5. Session choices such as a model or agent selected in the UI.

Objects are deep-merged. Arrays and scalar values are replaced by the higher-precedence value. This lets a project override one permission without repeating every global setting.

MCP definitions may also live in a dedicated `mcp.json` or `mcp.jsonc`. A dedicated MCP catalog is merged after ordinary configuration files in the same directory. See [[MCP Servers]] for global/project shapes, OAuth fields, and management commands.

## Main fields

| Field | Purpose |
|---|---|
| `model` | Default `provider/model` selection. |
| `small-model` | Less expensive model used for lightweight internal work when available. |
| `variant` | Default reasoning/model variant, such as `high`. |
| `text-verbosity` | Response length for supported models: `low`, `medium`, or `high`. GPT-5.x defaults to `low`. |
| `default-agent` | Initial primary agent. |
| `agent` | Named custom agent definitions. |
| `command` | Named prompt commands. |
| `skills` | Skill discovery configuration. |
| `instructions` | Instruction file paths or glob patterns. |
| `permission` | Global permission rules. |
| `tools` | Global tool enable/disable map. |
| `mcp` | Named MCP server definitions. |
| `formatter` | Formatter definitions used by formatting tools. |
| `lsp` | Language-server definitions. |
| `share` / `autoshare` | Session-sharing policy. |
| `enabled-providers` | Provider allowlist. |
| `disabled-providers` | Provider denylist. |
| `dangerously-skip-permissions` | Converts otherwise undecided/ask operations to allow; explicit deny rules still win. |

## Complete practical example

```jsonc
{
  "agent": {
    "model": "openai/gpt-5.2-codex",
    "small-model": "anthropic/claude-haiku-4-5",
    "default-agent": "build",
    "variant": "high",
    "text-verbosity": "low",
    "agent": {
      "review": {
        "description": "Reviews changes without editing files",
        "mode": "subagent",
        "model": "anthropic/claude-sonnet-4-5",
        "prompt": "Review for correctness, regressions, and missing tests.",
        "tools": { "write": false, "edit": false, "apply_patch": false },
        "permission": { "*": "allow", "edit": "deny" }
      }
    },
    "command": {
      "review": {
        "description": "Review the current workspace changes",
        "template": "Review the current changes. Focus on $ARGUMENTS",
        "agent": "review"
      }
    },
    "permission": {
      "read": "allow",
      "grep": "allow",
      "edit": "ask",
      "bash": {
        "git status": "allow",
        "git diff*": "allow",
        "git push*": "ask"
      }
    }
  }
}
```

## Environment substitution and secrets

Neoism configuration supports substitution in string values:

```jsonc
{
  "agent": {
    "username": "{env:USER}"
  }
}
```

Provider API keys normally come from the provider connection flow or the environment variables declared by the provider catalog. Do not commit API keys, bearer tokens, or authorization headers to `neoism.json`.

Related guides: [[Providers]], [[Agents and Subagents]], [[Permissions]], [[Commands]], [[MCP Servers]], [[Formatters, LSP, and References]].
