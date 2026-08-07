# Skills

A skill is a reusable instruction package stored in a `SKILL.md` file. Skills are discovered when a session builds its tool inventory and loaded on demand through the `skill` tool.

## Skill format

```markdown
---
name: database-migrations
description: Safely design and apply this project's database migrations
---

# Database migrations

Use the repository migration framework. Never edit an applied migration.
...
```

`name` and `description` make a skill discoverable. The Markdown body contains the actual instructions returned to the agent when it loads the skill.

## Discovery locations

Neoism discovers compatible skill directories in user and project locations, including Neoism, Claude, Codex, and Agents-compatible skill roots. A normal project skill can live at:

```text
.neoism/skills/database-migrations/SKILL.md
```

Configure additional paths:

```jsonc
{
  "agent": {
    "skills": {
      "paths": ["tools/release/SKILL.md", "shared-skills"],
      "urls": []
    }
  }
}
```

Relative paths resolve from the project directory. A directory is searched recursively for `SKILL.md`. A path may also identify one file directly.

## Remote skills

`skills.urls` can point to supported remote skill indexes/content. Remote loading depends on network access and the configured source. Treat remote skill instructions as code: review who controls the URL before allowing agents to use it.

## Load a skill

The agent receives the available skill names and descriptions in the `skill` tool description, then calls the tool by name. Skill contents are not automatically injected into every turn, avoiding unnecessary context use.

## Permission

The `skill` permission is checked against the selected skill name:

```jsonc
{
  "agent": {
    "permission": {
      "skill": {
        "*": "allow",
        "production-release": "ask"
      }
    }
  }
}
```

## Precedence and duplicates

Skills are indexed by name. Project/local definitions can replace a same-named lower-priority definition. Use unique names for unrelated packages.

## Skills versus instructions

Instructions are always added to relevant session context. Skills are optional and loaded only when needed. Use instructions for repository-wide rules and skills for specialized workflows.

See [[Instructions]] and [[MCP Servers]].