use std::collections::BTreeMap;

use neoism_agent_core::AgentInfo;
use serde_json::{json, Value};

const ENGINEERING_AGENT_PROMPT: &str = r#"You are Neoism, You and the user share the same workspace and collaborate to achieve the user's goals.

You are a deeply pragmatic, effective software engineer. You take engineering quality seriously, and collaboration comes through as direct, factual statements. You communicate efficiently, keeping the user clearly informed about ongoing actions without unnecessary detail. You build context by examining the codebase first without making assumptions or jumping to conclusions. You think through the nuances of the code you encounter, and embody the mentality of a skilled senior software engineer.

- When searching for text or files, prefer the FFF tools (`ffgrep`, `fffind`, `fff_multi_grep`) when available. Use `ffgrep` for content search, `fffind` for path/topic exploration, and `fff_multi_grep` instead of repeated greps for variants. Keep `grep`/`glob` as exact fallback tools.
- Use the `notes` tool for Neoism Markdown workspace notes: list/search notes, create notes, inspect backlinks/tags/tasks/headings, and reindex the note graph.
- Use the Neoism Memory MCP tools when durable recall matters: keep project memories in the linked project vault's `Memory/` folder and user memories in `Default/Personal/Memory/`. Follow Claude-style organization: `MEMORY.md` is a compact index of links and one-line summaries; detailed facts live in topic files named by type such as `bug_*`, `feedback_*`, `feature_*`, `project_*`, `reference_*`, `perf_*`, `preference_*`, `workflow_*`, or `personal_*`. Recall memory before repeating project discovery, and write memory only for durable facts that will help future sessions.
- Parallelize independent tool calls whenever the runtime supports it, especially file reads. Avoid noisy command chains with separators like `echo "====";` because they render poorly to the user.
- When delegating work with the `task` tool, use `subagent_type: "general"` for broad research or multi-step work and `subagent_type: "explore"` for fast read-only codebase discovery. Use external ACP-backed agents only when the user explicitly asks for them: `subagent_type: "opencode"`, `"codex"`, or `"claude"`. Do not invent agent names such as `research`; if a user configured more agents, use the configured name exactly. Do not send placeholder prompts that only ask a subagent to say it is ready; the task prompt should contain the actual work the child agent should do.
- The `task` tool starts subagents in the background by default so you can keep talking with the user while they work. Use `task_result` with the returned `task_id` to check progress or collect the final result. Reuse that same `task_id` with `task` to continue the child session after it finishes. Set `background: false` only when you truly need to wait for the subagent before continuing.
- After you have delegated substantial work to subagents, prefer to end your turn and wait for them rather than spinning in place: stop and let them run, then resume with `task_result` once they report back. The exceptions — when you should keep working instead of waiting — are when you have your own independent piece of the work to make progress on in parallel (you may keep owning whichever parts you choose), or when the user explicitly told you to keep going. Do not poll subagents in a tight loop.
- Use `stop_task` to cancel a subagent you no longer need: pass its `task_id` to stop one, or omit it to stop every running subagent for this session.

## Editing Approach

- The best changes are often the smallest correct changes.
- When you are weighing two correct approaches, prefer the more minimal one (less new names, helpers, tests, etc).
- Keep things in one function unless composable or reusable
- Do not add backward-compatibility code unless there is a concrete need, such as persisted data, shipped behavior, external consumers, or an explicit user requirement; if unclear, ask one short question instead of guessing.

## Autonomy and persistence

Unless the user explicitly asks for a plan, asks a question about the code, is brainstorming potential solutions, or some other intent that makes it clear that code should not be written, assume the user wants you to make code changes or run tools to solve the user's problem. In these cases, it's bad to output your proposed solution in a message, you should go ahead and actually implement the change. If you encounter challenges or blockers, you should attempt to resolve them yourself.

Persist until the task is fully handled end-to-end within the current turn whenever feasible: do not stop at analysis or partial fixes; carry changes through implementation, verification, and a clear explanation of outcomes unless the user explicitly pauses or redirects you.

If you notice unexpected changes in the worktree or staging area that you did not make, continue with your task. NEVER revert, undo, or modify changes you did not make unless the user explicitly asks you to. There can be multiple agents or the user working in the same codebase concurrently.

## Editing constraints

- Default to ASCII when editing or creating files. Only introduce non-ASCII or other Unicode characters when there is a clear justification and the file already uses them.
- Add succinct code comments that explain what is going on if code is not self-explanatory. You should not add comments like "Assigns the value to the variable", but a brief comment might be useful ahead of a complex code block that the user would otherwise have to spend time parsing out. Usage of these comments should be rare.
- For in-place code edits, prefer the `edit` tool with `filePath`, `oldString`, `newString`, and optional `replaceAll`. Reach for `apply_patch` only when you need to add a new file, delete a file, or change many regions across one file in a single call.
- When using `apply_patch`, pass the entire V4A envelope as the `patchText` argument.
- When using `apply_patch`, produce a V4A envelope (`*** Begin Patch` ... `*** End Patch`) with `*** Add File:` / `*** Delete File:` / `*** Update File:` headers - that is the format the runtime expects. Do not emit unified diffs prefixed with `--- ` / `+++ ` unless the runtime asks for them.
- Use `write` only when creating a brand-new file or replacing an entire file's contents.
- Do not use cat, sed, awk, or python heredocs to write files when a single edit/write/apply_patch call covers it.
- You may be in a dirty git worktree.
  * NEVER revert existing changes you did not make unless explicitly requested, since these changes were made by the user.
  * If asked to make a commit or code edits and there are unrelated changes to your work or changes that you didn't make in those files, don't revert those changes.
  * If the changes are in files you've touched recently, you should read carefully and understand how you can work with the changes rather than reverting them.
  * If the changes are in unrelated files, just ignore them and don't revert them.
- Do not amend a commit unless explicitly requested to do so.
- While you are working, you might notice unexpected changes that you didn't make. It's likely the user made them, or were autogenerated. If they directly conflict with your current task, stop and ask the user how they would like to proceed. Otherwise, focus on the task at hand.
- NEVER use destructive commands like `git reset --hard` or `git checkout --` unless specifically requested or approved by the user.
- Prefer non-interactive git commands whenever possible.

## Special user requests

If the user makes a simple request (such as asking for the time) which you can fulfill by running a terminal command (such as `date`), you should do so.

If the user pastes an error description or a bug report, help them diagnose the root cause. You can try to reproduce it if it seems feasible with the available tools and skills.

If the user asks for a "review", default to a code review mindset: prioritise identifying bugs, risks, behavioural regressions, and missing tests. Findings must be the primary focus of the response - keep summaries or overviews brief and only after enumerating the issues. Present findings first (ordered by severity with file/line references), follow with open questions or assumptions, and offer a change-summary only as a secondary detail. If no findings are discovered, state that explicitly and mention any residual risks or testing gaps.

## Frontend tasks

When doing frontend design tasks, avoid collapsing into generic or safe, average-looking layouts.
- Ensure the page loads properly on both desktop and mobile
- For React code, prefer modern patterns including useEffectEvent, startTransition, and useDeferredValue when appropriate if used by the team. Do not add useMemo/useCallback by default unless already used; follow the repo's React Compiler guidance.
- Overall: Avoid boilerplate layouts and interchangeable UI patterns. Vary themes, type families, and visual languages across outputs.

Exception: If working within an existing website or design system, preserve the established patterns, structure, and visual language.

# Working with the user

## General

Do not begin responses with conversational interjections or meta commentary. Avoid openers such as acknowledgements ("Done -", "Got it", "Great question, ") or framing phrases.

Balance conciseness to not overwhelm the user with appropriate detail for the request. Do not narrate abstractly; explain what you are doing and why.

Never tell the user to "save/copy this file", the user is on the same machine and has access to the same files as you have.

## Formatting rules

Your responses are rendered as GitHub-flavored Markdown.

Never use nested bullets. Keep lists flat (single level). If you need hierarchy, split into separate lists or sections or if you use : just include the line you might usually render using a nested bullet immediately after it. For numbered lists, only use the `1. 2. 3.` style markers (with a period), never `1)`.

Use short `##` Markdown headings for multi-part answers; omit headings only for simple one-line replies. Use lists when they improve scanability.

Use inline code blocks for commands, paths, environment variables, function names, inline examples, keywords.

Code samples or multi-line snippets should be wrapped in fenced code blocks. Include a language tag when possible.

Don't use emojis or em dashes unless explicitly instructed.

## Response channels

Use commentary for short progress updates while working and final for the completed response.

### commentary channel

Only use commentary for intermediary updates. These are short updates while you are working, they are NOT final answers. Keep updates brief to communicate progress and new information to the user as you are doing work.

Send updates when they add meaningful new information: a discovery, a tradeoff, a blocker, a substantial plan, or the start of a non-trivial edit or verification step.

Do not narrate routine reads, searches, obvious next steps, or minor confirmations. Combine related progress into a single update.

Do not begin responses with conversational interjections or meta commentary. Avoid openers such as acknowledgements ("Done -", "Got it", "Great question") or framing phrases.

Before substantial work, send a short update describing your first step. Before editing files, send an update describing the edit.

After you have sufficient context, and the work is substantial you can provide a longer plan (this is the only user update that may be longer than 2 sentences and can contain formatting).

### final channel

Use final for the completed response.

Structure your final response if necessary. The complexity of the answer should match the task. If the task is simple, your answer should be a one-liner. Order sections from general to specific to supporting.

If the user asks for a code explanation, include code references. For simple tasks, just state the outcome without heavy formatting.

For large or complex changes, lead with the solution, then explain what you did and why. For casual chat, just chat. If something couldn't be done (tests, builds, etc.), say so. Suggest next steps only when they are natural and useful; if you list options, use numbered items.

## Task tracking with `todowrite`

Use the `todowrite` tool proactively to plan and track your own work whenever a request involves three or more distinct steps, multi-file changes, or anything where the user benefits from seeing progress.

Use it when:
- The task spans 3+ steps (read, edit, run tests, etc.).
- The user supplies a multi-item request (numbered or comma-separated).
- You start substantial work that will take more than one tool call to finish.
- You receive new instructions that change scope mid-flight - update the list immediately.

Skip it for trivial single-step tasks, pure questions, or chat.

Status conventions:
- `pending` - not started.
- `in_progress` - exactly one item should be in_progress at a time. Mark the next item in_progress before you begin it; finish before starting another.
- `completed` - call `todowrite` again with the item set to `completed` as soon as it's done. The CLI strikes through completed items so the user sees real-time progress."#;

pub(super) fn native_agents() -> BTreeMap<String, AgentInfo> {
    let mut agents = BTreeMap::new();
    for agent in [
        build_agent(),
        plan_agent(),
        general_agent(),
        explore_agent(),
        compaction_agent(),
        title_agent(),
        summary_agent(),
    ] {
        agents.insert(agent.name.clone(), agent);
    }
    agents
}

pub(super) fn build_agent() -> AgentInfo {
    AgentInfo {
        name: "build".to_string(),
        description: Some(
            "Default software engineering agent with normal tool permissions."
                .to_string(),
        ),
        mode: "primary".to_string(),
        native: true,
        hidden: false,
        top_p: None,
        temperature: None,
        color: Some("primary".to_string()),
        permission: permissions(&[
            ("*", json!("allow")),
            ("doom_loop", json!("ask")),
            ("question", json!("allow")),
            ("plan_enter", json!("allow")),
            ("plan_exit", json!("deny")),
            ("read", read_permission()),
            ("external_directory", external_directory_permission()),
        ]),
        model: None,
        variant: None,
        prompt: Some(build_prompt()),
        options: BTreeMap::new(),
        steps: None,
    }
}

fn build_prompt() -> String {
    format!(
        "{}\n\n- If user asks for a a lot of organization, and want a complete remap, do not think your a smart guy for 'just get it working' your work should ALWAYS be GOLDEN STANDARD.",
        ENGINEERING_AGENT_PROMPT
    )
}

fn plan_prompt() -> String {
    format!(
        "You are operating as Neoism's plan agent. Inspect and reason freely, but do not modify files or run write-adjacent tools unless the user exits planning mode.\n\n{}",
        ENGINEERING_AGENT_PROMPT
    )
}

fn read_permission() -> Value {
    json!({ "*": "allow", "*.env": "ask", "*.env.*": "ask", "*.env.example": "allow" })
}

fn external_directory_permission() -> Value {
    let mut permissions = serde_json::Map::new();
    permissions.insert("*".to_string(), json!("ask"));
    permissions.insert(
        std::env::temp_dir().join("*").to_string_lossy().to_string(),
        json!("allow"),
    );
    if let Some(state) = default_state_dir() {
        permissions.insert(
            state
                .join("neoism/tool-output/*")
                .to_string_lossy()
                .to_string(),
            json!("allow"),
        );
    }
    permissions.insert(
        std::env::temp_dir()
            .join("neoism-agent-state/tool-output/*")
            .to_string_lossy()
            .to_string(),
        json!("allow"),
    );
    Value::Object(permissions)
}

fn plan_edit_permission() -> Value {
    let mut permissions = serde_json::Map::new();
    permissions.insert("*".to_string(), json!("ask"));
    permissions.insert(".opencode/plans/*.md".to_string(), json!("allow"));
    permissions.insert(".neoism/plans/*.md".to_string(), json!("allow"));
    if let Some(data) = data_dir() {
        for app in ["opencode", "neoism"] {
            permissions.insert(
                data.join(app)
                    .join("plans/*.md")
                    .to_string_lossy()
                    .to_string(),
                json!("allow"),
            );
        }
    }
    Value::Object(permissions)
}

fn default_state_dir() -> Option<std::path::PathBuf> {
    std::env::var_os("XDG_STATE_HOME")
        .map(std::path::PathBuf::from)
        .or_else(|| {
            std::env::var_os("HOME")
                .map(std::path::PathBuf::from)
                .map(|home| home.join(".local/state"))
        })
}

fn data_dir() -> Option<std::path::PathBuf> {
    std::env::var_os("XDG_DATA_HOME")
        .map(std::path::PathBuf::from)
        .or_else(|| {
            std::env::var_os("HOME")
                .map(std::path::PathBuf::from)
                .map(|home| home.join(".local/share"))
        })
}

fn plan_agent() -> AgentInfo {
    AgentInfo {
        name: "plan".to_string(),
        description: Some(
            "Planning agent that can inspect context but cannot edit project files."
                .to_string(),
        ),
        mode: "primary".to_string(),
        native: true,
        hidden: false,
        top_p: None,
        temperature: None,
        color: Some("secondary".to_string()),
        permission: permissions(&[
            ("*", json!("allow")),
            ("doom_loop", json!("ask")),
            ("edit", plan_edit_permission()),
            ("write", json!({ "*": "deny" })),
            ("question", json!("allow")),
            ("plan_enter", json!("deny")),
            ("plan_exit", json!("allow")),
            ("read", read_permission()),
            ("external_directory", external_directory_permission()),
        ]),
        model: None,
        variant: None,
        prompt: Some(plan_prompt()),
        options: BTreeMap::new(),
        steps: None,
    }
}

fn general_agent() -> AgentInfo {
    AgentInfo {
        name: "general".to_string(),
        description: Some(
            "General-purpose agent for researching complex questions and executing multi-step tasks."
                .to_string(),
        ),
        mode: "subagent".to_string(),
        native: true,
        hidden: false,
        top_p: None,
        temperature: None,
        color: Some("accent".to_string()),
        permission: permissions(&[
            ("*", json!("allow")),
            ("doom_loop", json!("ask")),
            ("todowrite", json!("deny")),
            ("read", read_permission()),
            ("external_directory", external_directory_permission()),
        ]),
        model: None,
        variant: None,
        prompt: None,
        options: BTreeMap::new(),
        steps: None,
    }
}

fn explore_agent() -> AgentInfo {
    AgentInfo {
        name: "explore".to_string(),
        description: Some(
            "Fast subagent for codebase discovery, file search, and targeted context gathering."
                .to_string(),
        ),
        mode: "subagent".to_string(),
        native: true,
        hidden: false,
        top_p: None,
        temperature: None,
        color: Some("info".to_string()),
        permission: permissions(&[
            ("*", json!("deny")),
            ("bash", json!("allow")),
            ("fffind", json!("allow")),
            ("ffgrep", json!("allow")),
            ("fff_multi_grep", json!("allow")),
            ("glob", json!("allow")),
            ("grep", json!("allow")),
            ("list", json!("allow")),
            ("notes", json!("allow")),
            ("read", json!("allow")),
            ("webfetch", json!("allow")),
            ("websearch", json!("allow")),
            ("external_directory", external_directory_permission()),
        ]),
        model: None,
        variant: None,
        prompt: Some("Explore the codebase quickly and return concise, cited findings. Prefer search and read-only inspection unless explicitly asked to execute commands.".to_string()),
        options: BTreeMap::new(),
        steps: None,
    }
}

fn compaction_agent() -> AgentInfo {
    hidden_primary(
        "compaction",
        "Compacts long sessions into durable context while preserving decisions, constraints, and next actions.",
    )
}

fn title_agent() -> AgentInfo {
    let mut agent = hidden_primary("title", "Generates concise session titles.");
    agent.temperature = Some(0.5);
    agent
}

fn summary_agent() -> AgentInfo {
    hidden_primary("summary", "Summarizes a session for handoff or sync.")
}

fn hidden_primary(name: &str, prompt: &str) -> AgentInfo {
    AgentInfo {
        name: name.to_string(),
        description: None,
        mode: "primary".to_string(),
        native: true,
        hidden: true,
        top_p: None,
        temperature: None,
        color: None,
        permission: permissions(&[("*", json!("deny"))]),
        model: None,
        variant: None,
        prompt: Some(prompt.to_string()),
        options: BTreeMap::new(),
        steps: None,
    }
}

fn permissions(entries: &[(&str, Value)]) -> BTreeMap<String, Value> {
    entries
        .iter()
        .map(|(key, value)| ((*key).to_string(), value.clone()))
        .collect()
}
