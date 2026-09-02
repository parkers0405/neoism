# Configure Neoism Agent

Neoism Agent reads the Agent domain from Neoism's unified JSONC configuration and then applies project-level agent configuration for the active workspace.

## Global configuration

Neoism's application configuration uses domain blocks. Agent settings belong under `"agent"`:

```jsonc
{
  "agent": {
    "model": "anthropic/claude-sonnet-4-5",
    "smallModel": "anthropic/claude-haiku-4-5",
    "defaultAgent": "build",
    "variant": "high",
    "textVerbosity": "low",
    "instructions": ["AGENTS.md"],
    "permission": {
      "edit": "ask",
      "bash": "ask"
    }
  }
}
```

Do not place `model`, `permission`, or `mcp` beside `appearance` and `terminal`. The desktop passes the contents of the top-level `agent` block to Neoism Agent.

## Workspace configuration

A workspace uses `.neoism/config.json`, with the same domain-based JSONC schema as the global file:

```jsonc
{
  "agent": {
    "model": "anthropic/claude-sonnet-4-5",
    "defaultAgent": "build",
    "instructions": ["AGENTS.md", "docs/engineering.md"],
    "permission": {
      "bash": {
        "git status": "allow",
        "git push*": "ask"
      }
    }
  }
}
```

Neoism reads configuration from the declared workspace root. Workspace values are partial overrides: every missing key continues to use the global `~/.config/neoism/config.json` value. If `.neoism/config.json` is absent, the global configuration is used unchanged. An existing root `neoism.json` is moved into the canonical workspace file automatically.

## Precedence

Configuration is merged from lower to higher precedence:

1. Global `~/.config/neoism/config.json`.
2. Global `~/.config/neoism/mcp.json`.
3. Workspace `.neoism/config.json`.
4. Workspace `.neoism/mcp.json`.
5. Session choices such as a model or agent selected in the UI.

Objects are deep-merged. Arrays and scalar values are replaced by the higher-precedence value. This lets a workspace override one permission without repeating every global setting.

MCP definitions may also live in a dedicated `mcp.json`. A dedicated MCP catalog is merged after ordinary configuration in the same scope. See [[MCP Servers]] for global/workspace shapes, OAuth fields, and management commands.

## Main fields

| Field | Purpose |
|---|---|
| `model` | Default `provider/model` selection. |
| `smallModel` | Less expensive model used for lightweight internal work when available. |
| `variant` | Default reasoning/model variant, such as `high`. |
| `textVerbosity` | Response length for supported models: `low`, `medium`, or `high`. GPT-5.x defaults to `low`. |
| `defaultAgent` | Initial primary agent. |
| `agent` | Named custom agent definitions. |
| `command` | Named prompt commands. |
| `skills` | Skill discovery configuration. |
| `instructions` | Instruction file paths or glob patterns. |
| `permission` | Global permission rules. |
| `tools` | Global tool enable/disable map. |
| `mcp` | Named MCP server definitions. |
| `plugins` | Named plugin entries — serve plugins (`serve`/`entry`/`npm`) and declarative hook plugins. See [[Plugins]]. |
| `formatter` | Formatter definitions used by formatting tools. |
| `lsp` | Language-server definitions. |
| `share` / `autoshare` | Session-sharing policy. |
| `enabledProviders` | Provider allowlist. |
| `disabledProviders` | Provider denylist. |
| `dangerouslySkipPermissions` | Converts otherwise undecided/ask operations to allow; explicit deny rules still win. |

## Complete practical example

```jsonc
{
  "agent": {
    "model": "openai/gpt-5.2-codex",
    "smallModel": "anthropic/claude-haiku-4-5",
    "defaultAgent": "build",
    "variant": "high",
    "textVerbosity": "low",
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
