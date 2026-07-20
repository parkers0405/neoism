use serde_json::{json, Value};

use super::{BuiltinTool, ToolExecutor};

pub(super) fn definitions() -> Vec<BuiltinTool> {
    vec![
        tool(
            "bash",
            "Run shell commands",
            object(&[
                ("command", "string"),
                ("timeout", "integer"),
                ("workdir", "string"),
                ("description", "string"),
            ]),
            ToolExecutor::Bash,
        ),
        tool(
            "background_task",
            "Start a long-running shell command in the background and return a job_id immediately",
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
                    "workdir": {
                        "type": "string",
                        "description": "Alias for cwd."
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
            ToolExecutor::Unsupported,
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
                    "jobId": {
                        "type": "string",
                        "description": "Alias for job_id."
                    }
                }
            }),
            ToolExecutor::Unsupported,
        ),
        tool(
            "read",
            "Read files or directories from the current project. Use `paths`/`filePaths` to batch multiple known reads in one call; default limit is 2000 lines, output is capped, and offset is 1-indexed. Use ffgrep/grep before reading large files when looking for specific content.",
            json!({
                "type": "object",
                "properties": {
                    "path": { "type": "string" },
                    "filePath": { "type": "string" },
                    "paths": { "type": "array", "items": { "type": "string" } },
                    "filePaths": { "type": "array", "items": { "type": "string" } },
                    "offset": { "type": "integer" },
                    "limit": { "type": "integer" }
                }
            }),
            ToolExecutor::Read,
        ),
        tool(
            "read_many",
            "Read multiple files or per-file line ranges in one call. Use this when different files need different offsets/limits.",
            json!({
                "type": "object",
                "properties": {
                    "files": {
                        "type": "array",
                        "items": {
                            "oneOf": [
                                { "type": "string" },
                                {
                                    "type": "object",
                                    "properties": {
                                        "path": { "type": "string" },
                                        "filePath": { "type": "string" },
                                        "offset": { "type": "integer" },
                                        "limit": { "type": "integer" },
                                        "ranges": {
                                            "type": "array",
                                            "items": {
                                                "type": "object",
                                                "properties": {
                                                    "offset": { "type": "integer" },
                                                    "line": { "type": "integer" },
                                                    "limit": { "type": "integer" }
                                                }
                                            }
                                        }
                                    }
                                }
                            ]
                        }
                    }
                },
                "required": ["files"]
            }),
            ToolExecutor::ReadMany,
        ),
        tool(
            "read_around",
            "Read a window around a line, pattern, or symbol in a file. Best for large files after search finds a relevant line or identifier.",
            json!({
                "type": "object",
                "properties": {
                    "path": { "type": "string" },
                    "filePath": { "type": "string" },
                    "line": { "type": "integer" },
                    "pattern": { "type": "string" },
                    "symbol": { "type": "string" },
                    "before": { "type": "integer" },
                    "after": { "type": "integer" }
                }
            }),
            ToolExecutor::ReadAround,
        ),
        tool(
            "write",
            "Create or overwrite files",
            object_required(
                &[
                ("path", "string"),
                ("filePath", "string"),
                ("content", "string"),
                ],
                &["filePath", "content"],
            ),
            ToolExecutor::Write,
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
            ToolExecutor::Edit,
        ),
        tool(
            "ffgrep",
            "Fast FFF content search. Prefer this over grep for codebase search; use bare identifiers, path/glob constraints, or fff_multi_grep for variants. For broad research, search first and batch-read only the best files.",
            object(&[
                ("pattern", "string"),
                ("query", "string"),
                ("path", "string"),
                ("include", "string"),
                ("exclude", "string"),
                ("limit", "integer"),
                ("context", "integer"),
                ("caseSensitive", "boolean"),
                ("mode", "string"),
                ("timeout", "integer"),
            ]),
            ToolExecutor::FfGrep,
        ),
        tool(
            "fffind",
            "Fast FFF fuzzy path and filename search. Prefer this over glob when exploring files or modules by topic/name. Query may be empty to list a directory page; keep fuzzy queries short and use path to scope searches.",
            object(&[
                ("query", "string"),
                ("pattern", "string"),
                ("path", "string"),
                ("limit", "integer"),
                ("offset", "integer"),
                ("timeout", "integer"),
            ]),
            ToolExecutor::FfFind,
        ),
        tool(
            "fff_multi_grep",
            "Fast FFF multi-pattern content search. Use this instead of repeated grep/ffgrep calls for case variants or related identifiers; provide constraints to avoid repo-wide noise.",
            json!({
                "type": "object",
                "properties": {
                    "patterns": {
                        "type": "array",
                        "items": { "type": "string" }
                    },
                    "constraints": { "type": "string" },
                    "path": { "type": "string" },
                    "exclude": { "type": "string" },
                    "limit": { "type": "integer" },
                    "context": { "type": "integer" },
                    "timeout": { "type": "integer" }
                },
                "required": ["patterns"]
            }),
            ToolExecutor::FffMultiGrep,
        ),
        tool(
            "grep",
            "Search file contents. Prefer ffgrep for most code search; keep grep for exact fallback compatibility.",
            object(&[
                ("pattern", "string"),
                ("path", "string"),
                ("include", "string"),
                ("exclude", "string"),
                ("limit", "integer"),
            ]),
            ToolExecutor::Grep,
        ),
        tool(
            "glob",
            "Find files by glob pattern",
            object(&[
                ("pattern", "string"),
                ("path", "string"),
                ("exclude", "string"),
                ("limit", "integer"),
            ]),
            ToolExecutor::Glob,
        ),
        tool(
            "list",
            "List directory entries",
            object(&[("path", "string")]),
            ToolExecutor::List,
        ),
        tool(
            "apply_patch",
            "Use the apply_patch tool to edit files. patchText must be a V4A envelope patch with *** Begin Patch, one or more *** Add File / *** Delete File / *** Update File headers, and *** End Patch.",
            object_required(
                &[("patchText", "string")],
                &["patchText"],
            ),
            ToolExecutor::ApplyPatch,
        ),
        tool(
            "webfetch",
            "Fetch and read a web page",
            object(&[("url", "string")]),
            ToolExecutor::WebFetch,
        ),
        tool(
            "webfetch_batch",
            "Fetch multiple web pages with bounded concurrency, retries, per-item truncation, and partial failure reporting",
            json!({
                "type": "object",
                "properties": {
                    "urls": {
                        "type": "array",
                        "items": { "type": "string" },
                        "description": "HTTP/HTTPS URLs to fetch. Limited to 20 entries."
                    },
                    "url": {
                        "type": "string",
                        "description": "Single URL fallback. Prefer urls for batches."
                    },
                    "concurrency": {
                        "type": "integer",
                        "description": "Maximum parallel requests. Defaults to 4, capped at 8."
                    },
                    "retries": {
                        "type": "integer",
                        "description": "Retry count per URL. Defaults to 1, capped at 3."
                    },
                    "perItemLimit": {
                        "type": "integer",
                        "description": "Maximum output characters per item. Defaults to 12000, capped at 50000."
                    }
                }
            }),
            ToolExecutor::WebFetchBatch,
        ),
        tool(
            "websearch",
            "Search the web",
            object(&[("query", "string")]),
            ToolExecutor::WebSearch,
        ),
        tool(
            "websearch_batch",
            "Run multiple web searches with bounded concurrency, retries, per-query truncation, and partial failure reporting",
            json!({
                "type": "object",
                "properties": {
                    "queries": {
                        "type": "array",
                        "items": { "type": "string" },
                        "description": "Queries to search. Limited to 20 entries."
                    },
                    "query": {
                        "type": "string",
                        "description": "Single query fallback. Prefer queries for batches."
                    },
                    "concurrency": {
                        "type": "integer",
                        "description": "Maximum parallel searches. Defaults to 4, capped at 8."
                    },
                    "retries": {
                        "type": "integer",
                        "description": "Retry count per query. Defaults to 1, capped at 3."
                    },
                    "perItemLimit": {
                        "type": "integer",
                        "description": "Maximum output characters per query. Defaults to 12000, capped at 50000."
                    }
                }
            }),
            ToolExecutor::WebSearchBatch,
        ),
        tool(
            "notes",
            "Neoism Markdown notes operations: init, list, read, write, search, tags, backlinks, headings, tasks, graph, or reindex.",
            object(&[
                ("operation", "string"),
                ("path", "string"),
                ("content", "string"),
                ("query", "string"),
                ("tag", "string"),
                ("limit", "integer"),
            ]),
            ToolExecutor::Notes,
        ),
        tool(
            "skill",
            "Load a configured SKILL.md instruction by name",
            object_required(
                &[("name", "string"), ("skill", "string"), ("id", "string")],
                &["name"],
            ),
            ToolExecutor::Skill,
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
                    "file": { "type": "string" },
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
                    },
                    "column": {
                        "type": "integer",
                        "minimum": 0,
                        "description": "Alias for character; zero-based UTF-8 byte column"
                    }
                },
                "required": ["operation"]
            }),
            ToolExecutor::Lsp,
        ),
        tool(
            "artifact_read",
            "Read lines from a saved large tool-output artifact by artifact:// URI or artifact id.",
            json!({
                "type": "object",
                "properties": {
                    "artifact": { "type": "string" },
                    "artifactId": { "type": "string" },
                    "offset": { "type": "integer" },
                    "limit": { "type": "integer" }
                }
            }),
            ToolExecutor::ArtifactRead,
        ),
        tool(
            "artifact_search",
            "Search within a saved large tool-output artifact by artifact:// URI or artifact id.",
            json!({
                "type": "object",
                "properties": {
                    "artifact": { "type": "string" },
                    "artifactId": { "type": "string" },
                    "query": { "type": "string" },
                    "pattern": { "type": "string" },
                    "limit": { "type": "integer" }
                }
            }),
            ToolExecutor::ArtifactSearch,
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
            ToolExecutor::Unsupported,
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
            ToolExecutor::Unsupported,
        ),
        tool(
            "task",
            "Delegate work to a subagent",
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
                    "agent": {
                        "type": "string",
                        "description": "Alias for subagent_type."
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
            ToolExecutor::Unsupported,
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
            ToolExecutor::Unsupported,
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
            ToolExecutor::Unsupported,
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
            ToolExecutor::Unsupported,
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
            ToolExecutor::Unsupported,
        ),
        tool(
            "plan_enter",
            "Enter planning mode",
            json!({ "type": "object", "properties": {} }),
            ToolExecutor::Unsupported,
        ),
        tool(
            "plan_exit",
            "Exit planning mode",
            json!({ "type": "object", "properties": {} }),
            ToolExecutor::Unsupported,
        ),
    ]
}

fn tool(
    id: &'static str,
    description: &'static str,
    parameters: Value,
    executor: ToolExecutor,
) -> BuiltinTool {
    BuiltinTool {
        id,
        description,
        parameters,
        executor,
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
