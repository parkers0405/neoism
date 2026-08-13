# Commands

Commands are named prompt templates invoked from an agent pane with `/name arguments`. Neoism combines built-in commands with commands from effective agent configuration.

## Built-in commands

The Agent server defines:

| Command | Purpose |
|---|---|
| `/init` | Analyze the project and create or refresh `AGENTS.md` guidance. |
| `/summarize` | Summarize the current session. |

The Neoism client also supplies runtime commands such as `/model`, `/agent`, `/mcp`, `/variant`, `/sessions`, `/new`, `/undo`, `/redo`, `/compact`, `/status`, `/subagents`, `/yolo`, `/hints`, `/answer`, and `/reject`. These control the UI/session rather than acting as configured prompt templates. `/mcp` opens the inline MCP management picker described in [[MCP Servers]].

## Define a command

```jsonc
{
  "agent": {
    "command": {
      "review": {
        "description": "Review the current changes",
        "template": "Review the current diff. Focus on $ARGUMENTS",
        "agent": "review",
        "model": "anthropic/claude-sonnet-4-5"
      }
    }
  }
}
```

Project `neoism.json` uses the direct form:

```jsonc
{
  "command": {
    "explain": {
      "description": "Explain one subsystem",
      "template": "Explain $1 for a new contributor. Cover $2"
    }
  }
}
```

## Fields

| Field | Purpose |
|---|---|
| `description` | Command palette/help text. |
| `template` | Prompt text. |
| `agent` | Agent profile used for execution. |
| `model` | Optional model override. |
| `subtask` | Marks command execution as subtask-oriented metadata. |

## Arguments

`$ARGUMENTS` expands to the complete argument string. `$1`, `$2`, and later numeric placeholders expand parsed arguments. The final numeric placeholder receives the remaining arguments.

Quoted arguments remain one value:

```text
/explain "session compaction" failure-modes and tests
```

If the template contains no placeholder, Neoism appends the supplied arguments after the template.

## Prompt command files

Neoism also discovers Markdown command definitions from supported command directories. Frontmatter supplies command metadata while the Markdown body is the prompt template. Project configuration remains useful when you want all settings in one JSONC file.

## Security

A command is prompt text, not a permission bypass. Its resulting tools still use the selected agent's tools and permission rules.

See [[Agents and Subagents]] and [[Instructions]].