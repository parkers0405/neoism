use serde_json::{json, Value};

use super::{
    apply_patch_handler, artifact_read_handler, artifact_search_handler, bash_handler,
    edit_handler, glob_handler, grep_handler, lsp_handler, notes_handler, read_handler,
    skill_handler, stateful_handler, webfetch_handler, write_handler,
    BuiltinTool, ToolHandler,
};

pub(super) fn definitions() -> &'static [BuiltinTool] {
    static DEFINITIONS: std::sync::OnceLock<Vec<BuiltinTool>> =
        std::sync::OnceLock::new();
    DEFINITIONS.get_or_init(|| vec![
        tool(
            "bash",
            crate::platform_shell::tool_description(),
            object_required(
                &[
                    ("command", "string"),
                    ("timeout", "integer"),
                    ("workdir", "string"),
                    ("description", "string"),
                ],
                &["command"],
            ),
            bash_handler,
        ),
        tool(
            "background_task",
            if cfg!(windows) {
                match crate::platform_shell::runtime().kind() {
                    crate::platform_shell::ShellKind::PowerShell => "Start a long-running PowerShell command in the background and return a job_id immediately",
                    crate::platform_shell::ShellKind::Cmd => "Start a long-running Command Prompt command in the background and return a job_id immediately",
                    crate::platform_shell::ShellKind::Posix => "Start a long-running shell command in the background and return a job_id immediately",
                }
            } else {
                "Start a long-running shell command in the background and return a job_id immediately"
            },
            json!({
                "type": "object",
                "properties": {
                    "description": {
                        "type": "string",
                        "description": "A short description of the background task."
                    },
                    "command": {
                        "type": "string",
                        "description": "The shell command to run."
                    },
                    "cwd": {
                        "type": "string",
                        "description": "Optional working directory. Relative paths resolve from the session project directory."
                    },
                    "timeout": {
                        "type": "integer",
                        "description": "Optional timeout in milliseconds before the process is terminated. Defaults to 1800000."
                    },
                    "outputLimit": {
                        "type": "integer",
                        "description": "Optional maximum stdout/stderr bytes retained in memory. Defaults to 262144."
                    }
                },
                "required": ["description", "command"]
            }),
            stateful_handler,
        ),
        tool(
            "background_task_result",
            "Check background shell task status or collect a completed result",
            json!({
                "type": "object",
                "properties": {
                    "job_id": {
                        "type": "string",
                        "description": "The job_id returned by background_task. Omit to list background tasks for the current session."
                    },
                }
            }),
            stateful_handler,
        ),
        tool(
            "read",
            "Read one file or directory. filePath is required; offset is 1-indexed, the default limit is 2000 lines, and output stops at 50 KB. Call multiple read tools in parallel for independent files. Use grep before reading a large file when you need specific content.",
            json!({
                "type": "object",
                "properties": {
                    "filePath": {
                        "type": "string",
                        "description": "Absolute path, or a path relative to the workspace."
                    },
                    "offset": { "type": "integer", "minimum": 1 },
                    "limit": { "type": "integer", "minimum": 1 }
                },
                "required": ["filePath"]
            }),
            read_handler,
        ),
        tool(
            "write",
            "Create or overwrite files",
            object_required(
                &[("filePath", "string"), ("content", "string")],
                &["filePath", "content"],
            ),
            write_handler,
        ),
        tool(
            "edit",
            "Replaces text in a file. Requires filePath, oldString, and newString. For V4A envelope patches, use apply_patch.",
            object_required(
                &[
                    ("filePath", "string"),
                    ("oldString", "string"),
                    ("newString", "string"),
                    ("replaceAll", "boolean"),
                ],
                &["filePath", "oldString", "newString"],
            ),
            edit_handler,
        ),
        tool(
            "grep",
            "Search file contents with FFF. pattern accepts one string or an array of literal alternatives. mode may be auto, plain, regex, or fuzzy. Results are bounded and include file paths and line numbers. Use several independent grep calls in parallel when their results do not depend on each other.",
            json!({
                "type": "object",
                "properties": {
                    "pattern": {
                        "oneOf": [
                            { "type": "string" },
                            {
                                "type": "array",
                                "items": { "type": "string" },
                                "minItems": 1
                            }
                        ]
                    },
                    "path": { "type": "string" },
                    "include": { "type": "string" },
                    "exclude": { "type": "string" },
                    "limit": { "type": "integer", "minimum": 1 },
                    "context": { "type": "integer", "minimum": 0 },
                    "caseSensitive": { "type": "boolean" },
                    "mode": { "type": "string", "enum": ["auto", "plain", "regex", "fuzzy"] },
                    "timeout": { "type": "integer", "minimum": 1000 }
                },
                "required": ["pattern"]
            }),
            grep_handler,
        ),
        tool(
            "glob",
            "Find files with FFF fuzzy path search and query constraints. pattern is a filename, path fragment, or glob expression. Keep broad queries short and scope with path when possible.",
            json!({
                "type": "object",
                "properties": {
                    "pattern": { "type": "string" },
                    "path": { "type": "string" },
                    "limit": { "type": "integer", "minimum": 1 },
                    "offset": { "type": "integer", "minimum": 0 },
                    "timeout": { "type": "integer", "minimum": 1000 }
                },
                "required": ["pattern"]
            }),
            glob_handler,
        ),
        tool(
            "apply_patch",
            "Use the apply_patch tool to edit one or many files atomically. patchText must be a V4A envelope patch with *** Begin Patch, one or more *** Add File / *** Delete File / *** Update File headers, and *** End Patch. Put related multi-file changes in one patch; independent tool calls may run in parallel.",
            object_required(&[("patchText", "string")], &["patchText"]),
            apply_patch_handler,
        ),
        tool(
            "webfetch",
            "Fetch and read a web page",
            json!({
                "type": "object",
                "properties": {
                    "url": { "type": "string" },
                    "format": { "type": "string", "enum": ["text", "markdown", "html"] },
                    "timeout": { "type": "integer", "minimum": 1, "maximum": 120 }
                },
                "required": ["url"]
            }),
            webfetch_handler,
        ),
        tool(
            "notes",
            "Neoism Markdown note-file operations: init, create, list, read, write, search, tasks, or taskToggle. Project-linked vaults are resolved automatically; graph indexing is disabled.",
            object(&[
                ("operation", "string"),
                ("path", "string"),
                ("content", "string"),
                ("query", "string"),
                ("tag", "string"),
                ("limit", "integer"),
                ("line", "integer"),
                ("checked", "boolean"),
                ("title", "string"),
            ]),
            notes_handler,
        ),
        tool(
            "skill",
            "Load a configured SKILL.md instruction by name",
            object_required(&[("name", "string")], &["name"]),
            skill_handler,
        ),
        tool(
            "lsp",
            "Query language-server information for the workspace. Supports status, workspaceSymbol, hover, goToDefinition, findReferences, goToImplementation, prepareCallHierarchy, incomingCalls, outgoingCalls, diagnostics, and documentSymbol.",
            json!({
                "type": "object",
                "properties": {
                    "operation": {
                        "type": "string",
                        "enum": [
                            "status",
                            "workspaceSymbol",
                            "hover",
                            "goToDefinition",
                            "findReferences",
                            "goToImplementation",
                            "prepareCallHierarchy",
                            "incomingCalls",
                            "outgoingCalls",
                            "diagnostics",
                            "documentSymbol"
                        ]
                    },
                    "query": { "type": "string" },
                    "filePath": { "type": "string" },
                    "line": {
                        "type": "integer",
                        "minimum": 0,
                        "description": "Zero-based document line"
                    },
                    "character": {
                        "type": "integer",
                        "minimum": 0,
                        "description": "Zero-based UTF-8 byte column (not a Unicode scalar or UTF-16 offset)"
                    }
                },
                "required": ["operation"]
            }),
            lsp_handler,
        ),
        tool(
            "artifact_read",
            "Read lines from saved large tool output by artifact:// URI, artifact id, or Neoism-managed output path.",
            json!({
                "type": "object",
                "properties": {
                    "artifact": { "type": "string" },
                    "offset": { "type": "integer" },
                    "limit": { "type": "integer" }
                },
                "required": ["artifact"]
            }),
            artifact_read_handler,
        ),
        tool(
            "artifact_search",
            "Search saved large tool output by artifact:// URI, artifact id, or Neoism-managed output path.",
            json!({
                "type": "object",
                "properties": {
                    "artifact": { "type": "string" },
                    "query": { "type": "string" },
                    "limit": { "type": "integer" }
                },
                "required": ["artifact", "query"]
            }),
            artifact_search_handler,
        ),
        tool(
            "session_search",
            "Full-text search across past session transcripts (FTS5). Use for episodic recall like \"didn't we fix this before?\". Returns bm25-ranked snippets with role and date.",
            json!({
                "type": "object",
                "properties": {
                    "query": {
                        "type": "string",
                        "description": "Full-text query. Words are stemmed; use plain keywords."
                    },
                    "session_id": {
                        "type": "string",
                        "description": "Optional: restrict the search to one session."
                    },
                    "limit": {
                        "type": "integer",
                        "description": "Maximum hits to return. Defaults to 10."
                    }
                },
                "required": ["query"]
            }),
            stateful_handler,
        ),
        tool(
            "todowrite",
            "Update an agent-visible task list",
            json!({
                "type": "object",
                "properties": {
                    "todos": {
                        "type": "array",
                        "items": {
                            "type": "object",
                            "properties": {
                                "content": { "type": "string" },
                                "status": { "type": "string" },
                                "priority": { "type": "string" }
                            }
                        }
                    }
                }
            }),
            stateful_handler,
        ),
        tool(
            "task",
            "Delegate work to a subagent. For broad read-only codebase research, launch the explore subagent as the only tool call in the step. Strongly prefer stopping the parent turn and waiting for Neoism to deliver its concise completion result when further work depends on it. The parent may continue user conversation or genuinely independent light work, but must not duplicate the exploration with parent-session tools or poll while it runs.",
            json!({
                "type": "object",
                "properties": {
                    "description": {
                        "type": "string",
                        "description": "A short 3-5 word description of the task."
                    },
                    "prompt": {
                        "type": "string",
                        "description": "The full task for the subagent to perform. Do not use a placeholder prompt that only asks the subagent to announce it is ready."
                    },
                    "subagent_type": {
                        "type": "string",
                        "description": "Configured subagent name. Use \"general\" for broad research or multi-step work, \"explore\" for fast read-only codebase discovery, and \"opencode\", \"codex\", or \"claude\" only when the user explicitly asks to delegate to that external ACP-backed agent. Do not invent names like \"research\" unless the user configured that agent."
                    },
                    "task_id": {
                        "type": "string",
                        "description": "Only set this to resume a previous task_id in the same child session."
                    },
                    "background": {
                        "type": "boolean",
                        "description": "Defaults to true so the UI stays usable while the subagent works. When true, start the subagent and then stop your turn unless the user explicitly asked you to continue with independent work; you will be notified when it finishes. Set false only when the next step truly must synchronously wait inside this same model turn."
                    },
                    "command": {
                        "type": "string",
                        "description": "The command or user-facing label that triggered this task."
                    }
                },
                "required": ["description", "prompt", "subagent_type"]
            }),
            stateful_handler,
        ),
        tool(
            "task_result",
            "Check background subagent task status or collect a completed result",
            json!({
                "type": "object",
                "properties": {
                    "task_id": {
                        "type": "string",
                        "description": "The task_id returned by task. Omit to list subagent tasks for the current session."
                    }
                }
            }),
            stateful_handler,
        ),
        tool(
            "stop_task",
            "Stop a running subagent task. Cancels the subagent's run and clears its queued follow-ups. Pass a task_id to stop one subagent, or omit it to stop every running subagent for this session.",
            json!({
                "type": "object",
                "properties": {
                    "task_id": {
                        "type": "string",
                        "description": "The task_id (child session id) returned by task. Omit to stop all running subagents for this session."
                    }
                }
            }),
            stateful_handler,
        ),
        tool(
            "question",
            "Ask the user a structured question",
            json!({
                "type": "object",
                "properties": {
                    "questions": {
                        "type": "array",
                        "items": { "type": "object" }
                    }
                }
            }),
            stateful_handler,
        ),
        tool(
            "complete_goal",
            "Mark the active persistent goal complete (or blocked) so the agent stops continuing automatically. Call this when the goal is fully accomplished, or when you cannot make further progress without the user.",
            json!({
                "type": "object",
                "properties": {
                    "status": {
                        "type": "string",
                        "enum": ["complete", "blocked"],
                        "description": "Use \"complete\" when the goal is fully done, or \"blocked\" when you cannot proceed without help. Defaults to \"complete\"."
                    },
                    "summary": {
                        "type": "string",
                        "description": "A thorough summary of what was accomplished (for complete) or exactly what is blocking you and what you need (for blocked)."
                    }
                },
                "required": ["summary"]
            }),
            stateful_handler,
        ),
        tool(
            "plan_enter",
            "Enter planning mode",
            json!({ "type": "object", "properties": {} }),
            stateful_handler,
        ),
        tool(
            "plan_exit",
            "Exit planning mode",
            json!({ "type": "object", "properties": {} }),
            stateful_handler,
        ),
    ]).as_slice()
}

fn tool(
    id: &'static str,
    description: &'static str,
    mut parameters: Value,
    handler: ToolHandler,
) -> BuiltinTool {
    if let Some(schema) = parameters.as_object_mut() {
        schema
            .entry("additionalProperties")
            .or_insert_with(|| Value::Bool(false));
    }
    BuiltinTool {
        id,
        description,
        parameters,
        output_schema: super::standard_output_schema(),
        handler,
    }
}

fn object(properties: &[(&str, &str)]) -> Value {
    object_with_required(properties, &[])
}

fn object_required(properties: &[(&str, &str)], required: &[&str]) -> Value {
    object_with_required(properties, required)
}

fn object_with_required(properties: &[(&str, &str)], required: &[&str]) -> Value {
    let properties = properties
        .iter()
        .map(|(name, kind)| ((*name).to_string(), json!({ "type": kind })))
        .collect::<serde_json::Map<_, _>>();
    json!({
        "type": "object",
        "properties": properties,
        "required": required,
    })
}
