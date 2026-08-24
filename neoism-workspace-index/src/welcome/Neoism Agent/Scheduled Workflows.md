# Scheduled Workflows

Scheduled workflows are agent prompts stored as Markdown files. YAML frontmatter controls where, when, and how the agent runs. The Markdown body is the prompt submitted to a normal durable Neoism Agent conversation.

## Locations

| Scope | Canonical directory |
|---|---|
| Project | `<workspace>/.neoism/workflows/` |
| Global | `~/.config/neoism/workflows/` |
| Config override | `$NEOISM_AGENT_CONFIG_DIR/workflows/` |

Neoism also recognizes singular `workflow/` directories and the inherited `$HOME/.neoism` config root. Global workflows run separately in each tracked workspace, because every workflow conversation needs a workspace and project context.

Files can be nested under these directories. Every workflow file must use the `.md` extension.

## Complete example

```markdown
---
id: daily-review
name: Daily workspace review
active: true
directory: packages/web
schedule:
  frequency: daily
  interval: 1
  time: 9:30 AM
  timezone: America/Los_Angeles
agent: build
model:
  providerId: anthropic
  modelId: claude-sonnet-4
permissions:
  read: allow
  edit: deny
  bash:
    default: deny
    allow:
      - git status*
      - git diff*
---

Review recent changes and outstanding tasks for this package.
Return a concise report. Do not modify files.
```

## Frontmatter reference

| Field | Required | Meaning |
|---|---:|---|
| `id` | Yes | Stable lowercase slug containing letters, numbers, `.`, `_`, or `-`. |
| `name` | Yes | Human-readable workflow and conversation title. |
| `active` | No | `true` enables automatic execution. Defaults to `false`. |
| `schedule` | Yes | One-time or recurring schedule described below. |
| `directory` | No | Existing relative, home-relative, or absolute execution directory. Project workflows default to the workspace root; global workflows default to the OS home directory. |
| `agent` | No | Configured agent name. The normal default agent is used when omitted. |
| `model` | No | Provider, model, and optional variant selection. |
| `skill` | No | Configured skill whose content is prepended to the prompt. |
| `permissions` | No | Workflow permission overrides. Agent permissions are inherited when omitted. |

Unknown fields, invalid combinations, empty prompts, and malformed values produce a workflow diagnostic instead of silently changing behavior.

## Conversation directory and project

When `directory` is omitted, a project workflow under `<workspace>/.neoism/workflows/` creates its conversation in the tracked workspace root:

```yaml
id: root-review
name: Root review
active: true
schedule:
  frequency: daily
  time: 09:00
```

The resulting session remains attached to the root project and is available in normal agent session history. A global workflow under `~/.config/neoism/workflows/` instead defaults to the operating system home directory (`$HOME` on Linux and macOS, and the user profile directory on Windows).

Set `directory` to run in a specific project subdirectory:

```yaml
directory: packages/web
```

Relative paths resolve from that workflow's default directory: the workspace root for project workflows and the OS home directory for global workflows. Home-relative and absolute paths are also supported:

```yaml
directory: ~/Documents/x/y
```

```yaml
directory: /srv/projects/example
```

The directory must already exist. Neoism canonicalizes it, records the exact directory on the conversation, and performs normal project discovery there. A directory inside a Git worktree is categorized under that Git project. A directory outside a Git worktree is categorized as `global`.

## Activation and hot reload

```yaml
active: true
```

Neoism uses native filesystem events. Creating, saving, atomically replacing, renaming, or deleting workflow files immediately triggers a debounced reconciliation. There is no short filesystem polling loop.

Saving `active: false` pauses future automatic runs without deleting history. Saving `active: true` activates or reactivates the workflow. Prompt, schedule, directory, agent, model, skill, and permission edits are loaded before the next run.

The activate and pause API routes remain available as temporary runtime overrides. A later source-file change makes frontmatter authoritative again.

## One-time schedules

Use an ISO calendar date, an optional time, and a timezone:

```yaml
schedule:
  date: 2026-09-15
  time: 10:40 PM
  timezone: America/Chicago
```

`time` defaults to `00:00`. `timezone` defaults to `UTC`.

A completed one-time workflow does not run again. Activating it after its timestamp has passed does not run it retroactively.

### Complete timestamp

Use `at` for an RFC 3339 timestamp containing its UTC offset:

```yaml
schedule:
  at: 2026-09-15T22:40:00-05:00
```

`at` cannot be combined with `date`, `time`, `frequency`, `interval`, `minute`, `weekdays`, or `monthDay`.

## Recurring schedules

### Hourly

```yaml
schedule:
  frequency: hourly
  interval: 2
  minute: 15
  timezone: UTC
```

This runs every second matching hour at 15 minutes past the hour.

### Daily

```yaml
schedule:
  frequency: daily
  interval: 1
  time: 09:30
  timezone: America/Los_Angeles
```

### Weekly

```yaml
schedule:
  frequency: weekly
  interval: 1
  time: 8:00 AM
  timezone: Europe/London
  weekdays: [monday, thursday]
```

Weekdays accept full names or `mon`, `tue`, `wed`, `thu`, `fri`, `sat`, and `sun`.

### Monthly

```yaml
schedule:
  frequency: monthly
  interval: 1
  time: 10:00
  timezone: UTC
  monthDay: 31
```

`monthDay` accepts `1` through `31`. If the selected day is beyond the end of a month, Neoism uses that month's final day.

## Schedule field compatibility

| Schedule kind | Fields |
|---|---|
| One-time date | `date`, optional `time`, optional `timezone` |
| One-time timestamp | `at` |
| Hourly | `frequency`, optional `interval`, optional `minute`, optional `timezone` |
| Daily | `frequency`, optional `interval`, optional `time`, optional `timezone` |
| Weekly | `frequency`, optional `interval`, required `weekdays`, optional `time`, optional `timezone` |
| Monthly | `frequency`, optional `interval`, optional `monthDay`, optional `time`, optional `timezone` |

`interval` defaults to `1` and must be at least `1`. Fields from different schedule kinds cannot be mixed.

## Date, time, and timezone formats

Dates use one unambiguous format:

```text
YYYY-MM-DD
```

Accepted local time formats are:

```text
22:40
22:40:30
10:40 PM
10:40:30 PM
```

AM and PM are case-insensitive. Ambiguous locale dates such as `8/9/26` and free-form phrases such as `tomorrow morning` are intentionally rejected.

Use an IANA timezone such as:

```yaml
timezone: America/Chicago
timezone: Europe/London
timezone: UTC
```

`timezone: local` resolves the operating system's current IANA timezone. Named timezones are safer when a workflow may move between computers.

During daylight-saving transitions, nonexistent local times move forward to the first valid minute. Ambiguous local times run once at the earlier occurrence.

## Permissions

Omit `permissions` to inherit the selected agent and global configuration:

```yaml
agent: build
```

Use a direct action for all targets in one permission group:

```yaml
permissions:
  read: allow
  edit: deny
  bash: deny
```

Actions are `allow`, `deny`, and `ask`. Scheduled workflows cannot explicitly use `ask`, because unattended runs cannot wait indefinitely for approval.

Use `default` and pattern lists for narrow overrides:

```yaml
permissions:
  bash:
    default: deny
    allow:
      - notify-send*
      - git status*
    deny:
      - rm *
```

Pattern precedence is deterministic: explicit `deny` patterns override `allow`, explicit `allow` overrides `ask`, and all explicit patterns override `default`. Workflow rules are applied after configured agent rules.

## Notification example

```markdown
---
id: workflow-notification
name: Workflow notification test
active: true
schedule:
  date: 2026-08-23
  time: 10:40 PM
  timezone: America/Chicago
permissions:
  bash:
    default: deny
    allow:
      - notify-send*
---

Run exactly this command with the bash tool:

`notify-send "Neoism Workflow" "Workflow worked"`

Do not perform any other action.
```

## Agent, model, and skill

Select a configured agent:

```yaml
agent: build
```

Select a model:

```yaml
model:
  providerId: anthropic
  modelId: claude-sonnet-4
  variant: high
```

Load a configured skill before the workflow prompt:

```yaml
skill: release-review
```

When omitted, Neoism uses the normal agent and model selection for the execution directory.

## Execution and recovery

At the scheduled timestamp, Neoism durably claims the run before creating a session. One activation cannot overlap itself. The session and run remain visible in normal history.

If the daemon was offline for several recurring occurrences, Neoism coalesces the backlog to the latest missed occurrence instead of replaying every interval. A one-time run is eligible only when it was activated before its timestamp.

If Neoism restarts during an unfinished run, that run is marked `interrupted` instead of automatically repeating possible side effects. Invalid or deleted active source files are paused rather than executing stale prompts.

## Preview, manual runs, and history

The agent server exposes:

```text
GET  /workflow
GET  /workflow/{id}
GET  /workflow/{id}/preview
POST /workflow/{id}/activate
POST /workflow/{id}/pause
POST /workflow/{id}/run
GET  /workflow/{id}/runs?limit=50
```

Pass the workspace with `?directory=/path/to/workspace` or the `x-neoism-directory` header. Preview returns upcoming UTC timestamps and local representations. Manual runs do not change source activation state.