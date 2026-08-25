use axum::Json;
use serde_json::{json, Value};

pub(crate) async fn canonical_openapi_doc() -> Json<Value> {
    Json(canonical_openapi())
}

pub fn canonical_openapi() -> Value {
    let json_response = |description: &str, schema: Value| {
        json!({ "description": description, "content": { "application/json": { "schema": schema } } })
    };
    let errors = || {
        json!({
            "400": { "$ref": "#/components/responses/BadRequest" },
            "404": { "$ref": "#/components/responses/NotFound" },
            "500": { "$ref": "#/components/responses/InternalError" }
        })
    };
    let mut document = json!({
        "openapi": "3.1.1",
        "info": {
            "title": "Neoism Agent API",
            "version": env!("CARGO_PKG_VERSION"),
            "description": "Canonical, versioned Neoism Agent contract. Unknown event and part payloads are intentionally open."
        },
        "tags": [
            { "name": "system" }, { "name": "plugins" }, { "name": "events" },
            { "name": "sessions" }, { "name": "catalog" }, { "name": "artifacts" }, { "name": "interactions" }, { "name": "subagents" }
        ],
        "paths": {},
        "components": {
            "parameters": {
                "PluginId": { "name": "plugin_id", "in": "path", "required": true, "schema": { "type": "string" } },
                "ArtifactId": { "name": "artifact_id", "in": "path", "required": true, "schema": { "type": "string" } },
                "RequestId": { "name": "request_id", "in": "path", "required": true, "schema": { "type": "string" } },
                "SessionId": { "name": "session_id", "in": "path", "required": true, "schema": { "type": "string" } },
                "Directory": { "name": "directory", "in": "query", "required": false, "schema": { "type": "string" } }
            },
            "responses": {
                "BadRequest": { "description": "Invalid request", "content": { "application/json": { "schema": { "$ref": "#/components/schemas/ApiError" } } } },
                "NotFound": { "description": "Resource not found", "content": { "application/json": { "schema": { "$ref": "#/components/schemas/ApiError" } } } },
                "InternalError": { "description": "Internal server error", "content": { "application/json": { "schema": { "$ref": "#/components/schemas/ApiError" } } } }
            },
            "schemas": canonical_schemas()
        }
    });

    let paths = document["paths"].as_object_mut().expect("paths object");
    paths.insert("/v2/openapi.json".into(), json!({ "get": {
        "tags": ["system"], "operationId": "v2.openapi.get", "summary": "Get this OpenAPI document",
        "responses": { "200": json_response("OpenAPI 3.1 document", json!({ "type": "object" })) }
    }}));
    paths.insert("/v2/meta".into(), json!({ "get": {
        "tags": ["system"], "operationId": "v2.meta.get", "responses": {
            "200": json_response("Server protocol metadata", json!({ "$ref": "#/components/schemas/ApiMeta" }))
        }
    }}));
    paths.insert("/v2/audit".into(), json!({ "get": {
        "tags": ["system"], "operationId": "v2.audit.list",
        "parameters": [{ "name": "limit", "in": "query", "required": false, "schema": { "type": "integer", "minimum": 1, "maximum": 1000, "default": 100 } }],
        "responses": merge_responses(json!({ "200": json_response("Tenant audit entries", json!({ "type": "array", "items": { "$ref": "#/components/schemas/AuditEntry" } })) }), errors())
    }}));
    paths.insert("/v2/capabilities".into(), json!({ "get": {
        "tags": ["plugins"], "operationId": "v2.capabilities.list", "parameters": [{ "$ref": "#/components/parameters/Directory" }],
        "responses": { "200": json_response("Available capabilities", json!({ "type": "array", "items": { "$ref": "#/components/schemas/Capability" } })) }
    }}));
    paths.insert("/v2/plugins".into(), json!({ "get": {
        "tags": ["plugins"], "operationId": "v2.plugins.list", "parameters": [{ "$ref": "#/components/parameters/Directory" }],
        "responses": { "200": json_response("Plugin manifests", json!({ "type": "array", "items": { "$ref": "#/components/schemas/PluginManifest" } })) }
    }}));
    paths.insert("/v2/plugins/{plugin_id}".into(), json!({
        "parameters": [{ "$ref": "#/components/parameters/PluginId" }],
        "get": { "tags": ["plugins"], "operationId": "v2.plugins.get", "parameters": [{ "$ref": "#/components/parameters/Directory" }],
            "responses": merge_responses(json!({ "200": json_response("Plugin manifest", json!({ "$ref": "#/components/schemas/PluginManifest" })) }), errors()) }
    }));
    for (path, method, operation_id) in [
        ("/v2/plugins/dev.neoism.goals/{session_id}", "get", "v2.plugins.goals.get"),
        ("/v2/plugins/dev.neoism.goals/{session_id}", "post", "v2.plugins.goals.set"),
        ("/v2/plugins/dev.neoism.goals/{session_id}", "delete", "v2.plugins.goals.clear"),
        ("/v2/plugins/dev.neoism.goals/{session_id}/research", "post", "v2.plugins.goals.research"),
        ("/v2/plugins/dev.neoism.semantic/search", "get", "v2.plugins.semantic.search"),
        ("/v2/plugins/dev.neoism.workflows", "get", "v2.plugins.workflows.list"),
        ("/v2/plugins/dev.neoism.workflows/{workflow_id}", "get", "v2.plugins.workflows.get"),
        ("/v2/plugins/dev.neoism.workflows/{workflow_id}/activate", "post", "v2.plugins.workflows.activate"),
        ("/v2/plugins/dev.neoism.workflows/{workflow_id}/pause", "post", "v2.plugins.workflows.pause"),
        ("/v2/plugins/dev.neoism.workflows/{workflow_id}/run", "post", "v2.plugins.workflows.run"),
        ("/v2/plugins/dev.neoism.workflows/{workflow_id}/preview", "get", "v2.plugins.workflows.preview"),
        ("/v2/plugins/dev.neoism.workflows/{workflow_id}/runs", "get", "v2.plugins.workflows.history"),
        ("/v2/plugins/dev.neoism.lsp", "get", "v2.plugins.lsp.status"),
        ("/v2/plugins/dev.neoism.lsp/hover", "get", "v2.plugins.lsp.hover"),
        ("/v2/plugins/dev.neoism.lsp/signature-help", "get", "v2.plugins.lsp.signatureHelp"),
        ("/v2/plugins/dev.neoism.lsp/inlay-hints", "get", "v2.plugins.lsp.inlayHints"),
        ("/v2/plugins/dev.neoism.lsp/document-highlights", "get", "v2.plugins.lsp.documentHighlights"),
        ("/v2/plugins/dev.neoism.lsp/definition", "get", "v2.plugins.lsp.definition"),
        ("/v2/plugins/dev.neoism.lsp/references", "get", "v2.plugins.lsp.references"),
        ("/v2/plugins/dev.neoism.lsp/implementation", "get", "v2.plugins.lsp.implementation"),
        ("/v2/plugins/dev.neoism.lsp/prepare-call-hierarchy", "get", "v2.plugins.lsp.prepareCallHierarchy"),
        ("/v2/plugins/dev.neoism.lsp/incoming-calls", "get", "v2.plugins.lsp.incomingCalls"),
        ("/v2/plugins/dev.neoism.lsp/outgoing-calls", "get", "v2.plugins.lsp.outgoingCalls"),
        ("/v2/plugins/dev.neoism.lsp/diagnostics", "get", "v2.plugins.lsp.diagnostics"),
        ("/v2/plugins/dev.neoism.lsp/document-symbols", "get", "v2.plugins.lsp.documentSymbols"),
        ("/v2/plugins/dev.neoism.lsp/formatting", "get", "v2.plugins.lsp.formatting"),
        ("/v2/plugins/dev.neoism.lsp/code-actions", "get", "v2.plugins.lsp.codeActions"),
        ("/v2/plugins/dev.neoism.lsp/touch", "post", "v2.plugins.lsp.touch"),
        ("/v2/plugins/dev.neoism.lsp/shutdown", "post", "v2.plugins.lsp.shutdown"),
        ("/v2/plugins/dev.neoism.mcp", "get", "v2.plugins.mcp.status"),
        ("/v2/plugins/dev.neoism.mcp", "post", "v2.plugins.mcp.add"),
        ("/v2/plugins/dev.neoism.mcp/catalog", "get", "v2.plugins.mcp.catalog"),
        ("/v2/plugins/dev.neoism.mcp/{name}/auth", "post", "v2.plugins.mcp.auth.start"),
        ("/v2/plugins/dev.neoism.mcp/{name}/auth", "delete", "v2.plugins.mcp.auth.remove"),
        ("/v2/plugins/dev.neoism.mcp/{name}/auth/callback", "get", "v2.plugins.mcp.auth.callback.get"),
        ("/v2/plugins/dev.neoism.mcp/{name}/auth/callback", "post", "v2.plugins.mcp.auth.callback.post"),
        ("/v2/plugins/dev.neoism.mcp/{name}/auth/authenticate", "post", "v2.plugins.mcp.auth.authenticate"),
        ("/v2/plugins/dev.neoism.mcp/{name}/connect", "post", "v2.plugins.mcp.connect"),
        ("/v2/plugins/dev.neoism.mcp/{name}/disconnect", "post", "v2.plugins.mcp.disconnect"),
        ("/v2/plugins/dev.neoism.mcp/{name}/config", "patch", "v2.plugins.mcp.config"),
        ("/v2/plugins/dev.neoism.mcp/{name}/tools", "get", "v2.plugins.mcp.tools"),
        ("/v2/plugins/dev.neoism.mcp/{name}/tools/{tool_name}", "post", "v2.plugins.mcp.tools.call"),
        ("/v2/plugins/dev.neoism.mcp/{name}/resources", "get", "v2.plugins.mcp.resources"),
        ("/v2/plugins/dev.neoism.mcp/{name}/prompts", "get", "v2.plugins.mcp.prompts"),
        ("/v2/plugins/dev.neoism.vcs", "get", "v2.plugins.vcs.get"),
        ("/v2/plugins/dev.neoism.vcs/diff", "get", "v2.plugins.vcs.diff"),
        ("/v2/plugins/dev.neoism.vcs/status", "get", "v2.plugins.vcs.status"),
        ("/v2/plugins/dev.neoism.vcs/diff/raw", "get", "v2.plugins.vcs.diff.raw"),
        ("/v2/plugins/dev.neoism.vcs/apply", "post", "v2.plugins.vcs.apply"),
        ("/v2/plugins/dev.neoism.pty/shells", "get", "v2.plugins.pty.shells"),
        ("/v2/plugins/dev.neoism.pty", "get", "v2.plugins.pty.list"),
        ("/v2/plugins/dev.neoism.pty", "post", "v2.plugins.pty.create"),
        ("/v2/plugins/dev.neoism.pty/{pty_id}", "get", "v2.plugins.pty.get"),
        ("/v2/plugins/dev.neoism.pty/{pty_id}", "put", "v2.plugins.pty.update"),
        ("/v2/plugins/dev.neoism.pty/{pty_id}", "delete", "v2.plugins.pty.remove"),
        ("/v2/plugins/dev.neoism.pty/{pty_id}/connect-token", "post", "v2.plugins.pty.connectToken"),
        ("/v2/plugins/dev.neoism.pty/{pty_id}/connect", "get", "v2.plugins.pty.connect"),
    ] {
        paths.entry(path).or_insert_with(|| json!({}))[method] = json!({
            "tags": ["plugins"],
            "operationId": operation_id,
            "responses": merge_responses(json!({ "200": json_response("Plugin response", json!({})) }), errors())
        });
    }
    for (path, method, operation_id) in [
        ("/v2/health", "get", "v2.health"),
        ("/v2/config", "get", "v2.config.get"),
        ("/v2/config", "patch", "v2.config.update"),
        ("/v2/config/validate", "get", "v2.config.validate"),
        ("/v2/sessions/import", "post", "v2.sessions.import"),
        ("/v2/sessions/export", "post", "v2.sessions.export"),
        ("/v2/sessions/{session_id}/messages/{message_id}", "get", "v2.sessions.messages.get"),
        ("/v2/sessions/{session_id}/messages/{message_id}", "delete", "v2.sessions.messages.delete"),
        ("/v2/sessions/{session_id}/messages/{message_id}/parts/{part_id}", "patch", "v2.sessions.parts.update"),
        ("/v2/sessions/{session_id}/messages/{message_id}/parts/{part_id}", "delete", "v2.sessions.parts.delete"),
        ("/v2/sessions/{session_id}/directory-options", "get", "v2.sessions.directoryOptions"),
        ("/v2/sessions/{session_id}/todos", "get", "v2.sessions.todos"),
        ("/v2/sessions/{session_id}/fork", "post", "v2.sessions.fork"),
        ("/v2/sessions/{session_id}/diff", "get", "v2.sessions.diff"),
        ("/v2/sessions/{session_id}/undo-tree", "get", "v2.sessions.undoTree"),
        ("/v2/sessions/{session_id}/shell", "post", "v2.sessions.shell"),
        ("/v2/sessions/{session_id}/revert", "post", "v2.sessions.revert"),
        ("/v2/sessions/{session_id}/unrevert", "post", "v2.sessions.unrevert"),
    ] {
        paths.entry(path).or_insert_with(|| json!({}))[method] = json!({
            "tags": ["sessions"],
            "operationId": operation_id,
            "responses": merge_responses(json!({ "200": json_response("Response", json!({})) }), errors())
        });
    }
    paths.insert("/v2/events".into(), json!({ "get": {
        "tags": ["events"], "operationId": "v2.events.subscribe",
        "parameters": [
            { "name": "Last-Event-ID", "in": "header", "required": false, "schema": { "type": "integer", "minimum": 0 } },
            { "name": "since", "in": "query", "required": false, "schema": { "type": "integer", "minimum": 0 } },
            { "name": "tail", "in": "query", "required": false, "description": "Start after the latest durable event when no cursor is supplied", "schema": { "type": "boolean", "default": false } },
            { "name": "limit", "in": "query", "required": false, "schema": { "type": "integer", "minimum": 1, "maximum": 5000, "default": 1000 } },
            { "name": "sessionId", "in": "query", "required": false, "schema": { "type": "string" } }
        ],
        "responses": { "200": { "description": "Durable ordered SSE stream; each data field is an EventEnvelope", "content": { "text/event-stream": { "schema": { "type": "string" } } } } }
    }}));
    paths.insert("/v2/artifacts".into(), json!({
        "get": { "tags": ["artifacts"], "operationId": "v2.artifacts.list", "parameters": [
            { "name": "sessionId", "in": "query", "required": false, "schema": { "type": "string" } }
        ], "responses": merge_responses(json!({ "200": json_response("Artifacts", json!({ "type": "array", "items": { "$ref": "#/components/schemas/Artifact" } })) }), errors()) },
        "post": { "tags": ["artifacts"], "operationId": "v2.artifacts.create", "parameters": [
            { "name": "X-Neoism-Filename", "in": "header", "required": false, "schema": { "type": "string" } },
            { "name": "X-Neoism-Session-Id", "in": "header", "required": false, "schema": { "type": "string" } }
        ], "requestBody": { "required": true, "content": { "application/octet-stream": { "schema": { "type": "string", "format": "binary" } } } },
        "responses": merge_responses(json!({ "201": json_response("Created artifact", json!({ "$ref": "#/components/schemas/Artifact" })) }), errors()) }
    }));
    paths.insert("/v2/artifacts/{artifact_id}".into(), json!({
        "parameters": [{ "$ref": "#/components/parameters/ArtifactId" }],
        "get": { "tags": ["artifacts"], "operationId": "v2.artifacts.get", "responses": merge_responses(json!({ "200": json_response("Artifact", json!({ "$ref": "#/components/schemas/Artifact" })) }), errors()) },
        "delete": { "tags": ["artifacts"], "operationId": "v2.artifacts.delete", "responses": merge_responses(json!({ "204": { "description": "Artifact deleted" } }), errors()) }
    }));
    paths.insert("/v2/artifacts/{artifact_id}/content".into(), json!({
        "parameters": [{ "$ref": "#/components/parameters/ArtifactId" }],
        "get": { "tags": ["artifacts"], "operationId": "v2.artifacts.content", "responses": merge_responses(json!({ "200": { "description": "Artifact bytes", "content": { "application/octet-stream": { "schema": { "type": "string", "format": "binary" } } } } }), errors()) }
    }));
    paths.insert("/v2/interactions/permissions".into(), json!({ "get": {
        "tags": ["interactions"], "operationId": "v2.interactions.permissions.list",
        "parameters": [{ "name": "sessionId", "in": "query", "required": false, "schema": { "type": "string" } }],
        "responses": { "200": json_response("Pending permission requests", json!({ "type": "array", "items": { "$ref": "#/components/schemas/PermissionRequest" } })) }
    }}));
    paths.insert("/v2/interactions/permissions/{request_id}/reply".into(), json!({
        "parameters": [{ "$ref": "#/components/parameters/RequestId" }],
        "post": { "tags": ["interactions"], "operationId": "v2.interactions.permissions.reply", "requestBody": json_body(true, "#/components/schemas/PermissionReply"),
        "responses": merge_responses(json!({ "200": json_response("Reply accepted", json!({ "type": "boolean" })) }), errors()) }
    }));
    paths.insert("/v2/interactions/questions".into(), json!({ "get": {
        "tags": ["interactions"], "operationId": "v2.interactions.questions.list",
        "parameters": [{ "name": "sessionId", "in": "query", "required": false, "schema": { "type": "string" } }],
        "responses": { "200": json_response("Pending questions", json!({ "type": "array", "items": { "$ref": "#/components/schemas/QuestionRequest" } })) }
    }}));
    for (suffix, operation, with_body) in [
        ("reply", "v2.interactions.questions.reply", true),
        ("reject", "v2.interactions.questions.reject", false),
    ] {
        let mut operation = json!({ "tags": ["interactions"], "operationId": operation,
            "responses": { "200": json_response("Resolution accepted", json!({ "type": "boolean" })) } });
        if with_body {
            operation["requestBody"] = json_body(true, "#/components/schemas/QuestionReply");
        }
        paths.insert(format!("/v2/interactions/questions/{{request_id}}/{suffix}"), json!({
            "parameters": [{ "$ref": "#/components/parameters/RequestId" }], "post": operation
        }));
    }
    for (path, operation, schema) in [
        ("/v2/agents", "v2.agents.list", "AgentList"),
        ("/v2/commands", "v2.commands.list", "CommandList"),
        ("/v2/providers", "v2.providers.list", "UnknownObject"),
        ("/v2/providers/configured", "v2.providers.configured", "ProviderList"),
        ("/v2/providers/auth-methods", "v2.providers.authMethods", "UnknownObject"),
        ("/v2/skills", "v2.skills.list", "SkillList"),
        ("/v2/tools", "v2.tools.list", "ToolList"),
    ] {
        paths.insert(path.to_string(), json!({ "get": {
            "tags": ["catalog"], "operationId": operation, "parameters": [{ "$ref": "#/components/parameters/Directory" }],
            "responses": merge_responses(json!({ "200": json_response("Catalog response", json!({ "$ref": format!("#/components/schemas/{schema}") })) }), errors())
        }}));
    }
    paths.insert("/v2/agents/{name}".into(), json!({
        "parameters": [{ "name": "name", "in": "path", "required": true, "schema": { "type": "string" } }],
        "get": { "tags": ["catalog"], "operationId": "v2.agents.get", "parameters": [{ "$ref": "#/components/parameters/Directory" }],
        "responses": merge_responses(json!({ "200": json_response("Agent", json!({ "$ref": "#/components/schemas/Agent" })) }), errors()) }
    }));
    paths.insert("/v2/sessions/{session_id}/jobs/{job_id}".into(), json!({
        "parameters": [
            { "$ref": "#/components/parameters/SessionId" },
            { "name": "job_id", "in": "path", "required": true, "schema": { "type": "string" } }
        ],
        "delete": { "tags": ["sessions"], "operationId": "v2.sessions.jobs.cancel",
            "responses": merge_responses(json!({ "200": json_response("Job cancellation result", json!({})) }), errors()) }
    }));
    paths.insert("/v2/providers/{provider_id}/auth".into(), json!({
        "parameters": [{ "name": "provider_id", "in": "path", "required": true, "schema": { "type": "string" } }],
        "get": { "tags": ["catalog"], "operationId": "v2.providers.auth.get", "responses": merge_responses(json!({ "200": json_response("Authentication state", json!({})) }), errors()) },
        "put": { "tags": ["catalog"], "operationId": "v2.providers.auth.set", "requestBody": { "required": true, "content": { "application/json": { "schema": {} } } }, "responses": merge_responses(json!({ "200": json_response("Authentication updated", json!({})) }), errors()) },
        "delete": { "tags": ["catalog"], "operationId": "v2.providers.auth.delete", "responses": merge_responses(json!({ "204": { "description": "Authentication removed" } }), errors()) }
    }));
    for (suffix, operation) in [
        ("authorize", "v2.providers.oauth.authorize"),
        ("callback", "v2.providers.oauth.callback"),
    ] {
        paths.insert(format!("/v2/providers/{{provider_id}}/oauth/{suffix}"), json!({
            "parameters": [{ "name": "provider_id", "in": "path", "required": true, "schema": { "type": "string" } }],
            "post": { "tags": ["catalog"], "operationId": operation, "requestBody": { "required": true, "content": { "application/json": { "schema": {} } } },
                "responses": merge_responses(json!({ "200": json_response("OAuth response", json!({})) }), errors()) }
        }));
    }
    paths.insert("/v2/sessions".into(), json!({
        "get": { "tags": ["sessions"], "operationId": "v2.sessions.list", "parameters": session_list_parameters(),
            "responses": merge_responses(json!({ "200": json_response("Session page", json!({ "$ref": "#/components/schemas/SessionPage" })) }), errors()) },
        "post": { "tags": ["sessions"], "operationId": "v2.sessions.create", "parameters": [{ "$ref": "#/components/parameters/Directory" }],
            "requestBody": json_body(false, "#/components/schemas/CreateSessionRequest"),
            "responses": merge_responses(json!({ "200": json_response("Created session", json!({ "$ref": "#/components/schemas/Session" })) }), errors()) }
    }));
    paths.insert("/v2/sessions/status".into(), json!({ "get": {
        "tags": ["sessions"], "operationId": "v2.sessions.status",
        "responses": merge_responses(json!({ "200": json_response("Session status map", json!({ "type": "object", "additionalProperties": true })) }), errors())
    }}));
    paths.insert("/v2/sessions/{session_id}".into(), json!({
        "parameters": [{ "$ref": "#/components/parameters/SessionId" }],
        "get": { "tags": ["sessions"], "operationId": "v2.sessions.get", "responses": session_response("Session", errors()) },
        "patch": { "tags": ["sessions"], "operationId": "v2.sessions.update", "requestBody": json_body(true, "#/components/schemas/UpdateSessionRequest"), "responses": session_response("Updated session", errors()) },
        "delete": { "tags": ["sessions"], "operationId": "v2.sessions.delete", "responses": merge_responses(json!({ "200": json_response("Whether the session was deleted", json!({ "type": "boolean" })) }), errors()) }
    }));
    for (suffix, operation, schema) in [
        ("messages", "v2.sessions.messages", "MessagePage"),
        ("children", "v2.sessions.children", "SessionPage"),
        ("context", "v2.sessions.context", "MessageList"),
    ] {
        paths.insert(format!("/v2/sessions/{{session_id}}/{suffix}"), json!({
            "parameters": [{ "$ref": "#/components/parameters/SessionId" }],
            "get": { "tags": ["sessions"], "operationId": operation,
                "responses": merge_responses(json!({ "200": json_response("Successful response", json!({ "$ref": format!("#/components/schemas/{schema}") })) }), errors()) }
        }));
    }
    for (suffix, operation) in [
        ("prompt", "v2.sessions.prompt"), ("prompt-async", "v2.sessions.promptAsync")
    ] {
        paths.insert(format!("/v2/sessions/{{session_id}}/{suffix}"), json!({
            "parameters": [{ "$ref": "#/components/parameters/SessionId" }],
            "post": { "tags": ["sessions"], "operationId": operation, "requestBody": json_body(true, "#/components/schemas/PromptRequest"),
                "responses": merge_responses(json!({ "204": { "description": "Prompt accepted" } }), errors()) }
        }));
    }
    for (suffix, operation, description) in [
        ("abort", "v2.sessions.abort", "Run aborted"),
        ("compact", "v2.sessions.compact", "Context compacted"),
        ("wait", "v2.sessions.wait", "Session exists")
    ] {
        paths.insert(format!("/v2/sessions/{{session_id}}/{suffix}"), json!({
            "parameters": [{ "$ref": "#/components/parameters/SessionId" }],
            "post": { "tags": ["sessions"], "operationId": operation,
                "responses": merge_responses(json!({ "204": { "description": description } }), errors()) }
        }));
    }
    paths.insert("/v2/sessions/{session_id}/queue".into(), json!({
        "parameters": [{ "$ref": "#/components/parameters/SessionId" }],
        "get": { "tags": ["sessions"], "operationId": "v2.sessions.queue.list", "responses": merge_responses(json!({ "200": json_response("Queued prompts", json!({ "type": "array", "items": {} })) }), errors()) },
        "delete": { "tags": ["sessions"], "operationId": "v2.sessions.queue.clear", "responses": merge_responses(json!({ "204": { "description": "Queue cleared" } }), errors()) }
    }));
    for (suffix, operation) in [
        ("queue/pop", "v2.sessions.queue.pop"),
        ("commands", "v2.sessions.commands.execute"),
        ("undo", "v2.sessions.undo"),
        ("redo", "v2.sessions.redo"),
        ("summarize", "v2.sessions.summarize"),
        ("pin", "v2.sessions.pin"),
    ] {
        paths.insert(format!("/v2/sessions/{{session_id}}/{suffix}"), json!({
            "parameters": [{ "$ref": "#/components/parameters/SessionId" }],
            "post": { "tags": ["sessions"], "operationId": operation,
                "requestBody": { "required": false, "content": { "application/json": { "schema": {} } } },
                "responses": merge_responses(json!({ "200": json_response("Successful response", json!({})) }), errors()) }
        }));
    }
    paths.insert("/v2/plugins/dev.neoism.subagents/sessions/{session_id}/tasks".into(), json!({
        "parameters": [{ "$ref": "#/components/parameters/SessionId" }],
        "get": { "tags": ["subagents"], "operationId": "v2.subagents.tasks.list",
            "responses": merge_responses(json!({ "200": json_response("Descendant subagent tasks", json!({ "type": "array", "items": { "$ref": "#/components/schemas/SubagentTask" } })) }), errors()) }
    }));
    paths.insert("/v2/plugins/dev.neoism.subagents/sessions/{session_id}/stop".into(), json!({
        "parameters": [{ "$ref": "#/components/parameters/SessionId" }],
        "post": { "tags": ["subagents"], "operationId": "v2.subagents.tasks.stop", "requestBody": json_body(true, "#/components/schemas/StopSubagentsRequest"),
            "responses": merge_responses(json!({ "200": json_response("Stopped tasks", json!({ "$ref": "#/components/schemas/StopSubagentsResult" })) }), errors()) }
    }));
    document
}

fn merge_responses(mut success: Value, errors: Value) -> Value {
    success.as_object_mut().unwrap().extend(errors.as_object().unwrap().clone());
    success
}

fn json_body(required: bool, schema_ref: &str) -> Value {
    json!({ "required": required, "content": { "application/json": { "schema": { "$ref": schema_ref } } } })
}

fn session_response(description: &str, errors: Value) -> Value {
    merge_responses(json!({ "200": { "description": description, "content": { "application/json": { "schema": { "$ref": "#/components/schemas/Session" } } } } }), errors)
}

fn session_list_parameters() -> Value {
    json!([
        { "name": "directory", "in": "query", "schema": { "type": "string" } },
        { "name": "path", "in": "query", "schema": { "type": "string" } },
        { "name": "roots", "in": "query", "schema": { "type": "string" } },
        { "name": "start", "in": "query", "schema": { "type": "integer", "minimum": 0 } },
        { "name": "search", "in": "query", "schema": { "type": "string" } },
        { "name": "limit", "in": "query", "schema": { "type": "integer", "minimum": 1 } }
    ])
}

fn canonical_schemas() -> Value {
    json!({
        "ApiMeta": { "type": "object", "additionalProperties": false, "required": ["apiVersion", "serverVersion", "pluginApiVersion", "eventSchemaVersion", "partSchemaVersion", "generation"], "properties": {
            "apiVersion": { "type": "string" }, "serverVersion": { "type": "string" }, "pluginApiVersion": { "type": "string" },
            "eventSchemaVersion": { "type": "string" }, "partSchemaVersion": { "type": "string" }, "generation": { "type": "integer", "minimum": 0 }
        }},
        "AuditEntry": { "type": "object", "additionalProperties": false, "required": ["id", "tenantId", "method", "path", "status", "created"], "properties": {
            "id": { "type": "string" }, "tenantId": { "type": "string" }, "method": { "type": "string" }, "path": { "type": "string" }, "status": { "type": "integer" }, "created": { "type": "integer" }
        }},
        "Capability": { "type": "object", "additionalProperties": false, "required": ["id", "version", "enabled", "disableable", "source"], "properties": {
            "id": { "type": "string" }, "version": { "type": "string" }, "enabled": { "type": "boolean" }, "disableable": { "type": "boolean" }, "source": { "type": "string" },
            "pluginId": { "type": "string" }, "apiPrefix": { "type": "string" }, "reason": { "type": "string" }
        }},
        "PluginManifest": { "type": "object", "additionalProperties": false, "required": ["id", "name", "version", "pluginApi", "internal", "enabled", "active", "disableable", "capabilities", "requires", "eventNamespaces"], "properties": {
            "id": { "type": "string" }, "name": { "type": "string" }, "version": { "type": "string" }, "pluginApi": { "type": "string" }, "internal": { "type": "boolean" },
            "enabled": { "type": "boolean" }, "active": { "type": "boolean" }, "disableable": { "type": "boolean" }, "capabilities": { "type": "array", "items": { "type": "string" } },
            "requires": { "type": "array", "items": { "type": "string" } }, "eventNamespaces": { "type": "array", "items": { "type": "string" } },
            "apiPrefix": { "type": "string" }, "reason": { "type": "string" }, "config": { "type": "object", "additionalProperties": true }
        }},
        "EventSubject": { "type": "object", "additionalProperties": false, "required": ["kind", "id"], "properties": { "kind": { "type": "string" }, "id": { "type": "string" } } },
        "EventEnvelope": { "type": "object", "additionalProperties": false, "required": ["id", "sequence", "type", "source", "schemaVersion", "timestamp", "data"], "properties": {
            "id": { "type": "string" }, "sequence": { "type": "integer", "minimum": 0 }, "type": { "type": "string" }, "source": { "type": "string" },
            "schemaVersion": { "type": "string" }, "timestamp": { "type": "integer" }, "subject": { "$ref": "#/components/schemas/EventSubject" }, "data": {}
        }},
        "PartEnvelope": { "type": "object", "additionalProperties": false, "required": ["id", "kind", "schemaVersion", "data"], "properties": {
            "id": { "type": "string" }, "kind": { "type": "string" }, "schemaVersion": { "type": "string" }, "data": {}
        }},
        "ApiError": { "type": "object", "additionalProperties": false, "required": ["code", "message", "retryable", "details"], "properties": {
            "code": { "type": "string" }, "message": { "type": "string" }, "retryable": { "type": "boolean" }, "requestId": { "type": "string" }, "details": { "type": "object", "additionalProperties": true }
        }},
        "Artifact": { "type": "object", "additionalProperties": false, "required": ["id", "filename", "mediaType", "size", "sha256", "created", "downloadUrl"], "properties": {
            "id": { "type": "string" }, "filename": { "type": "string" }, "mediaType": { "type": "string" }, "size": { "type": "integer", "minimum": 0 },
            "sha256": { "type": "string", "pattern": "^[0-9a-f]{64}$" }, "created": { "type": "integer", "minimum": 0 }, "sessionId": { "type": "string" }, "downloadUrl": { "type": "string" }
        }},
        "PermissionRequest": { "type": "object", "additionalProperties": false, "required": ["id", "sessionId", "messageId", "title", "permission", "patterns", "always"], "properties": {
            "id": { "type": "string" }, "sessionId": { "type": "string" }, "messageId": { "type": "string" }, "title": { "type": "string" }, "permission": { "type": "string" },
            "patterns": { "type": "array", "items": { "type": "string" } }, "always": { "type": "array", "items": { "type": "string" } }, "tool": {}, "metadata": {}
        }},
        "PermissionReply": { "type": "object", "additionalProperties": false, "properties": {
            "reply": { "type": "string", "enum": ["once", "always", "reject"] }, "response": { "type": "string" }, "message": { "type": "string" }
        }},
        "QuestionRequest": { "type": "object", "additionalProperties": false, "required": ["id", "sessionId", "messageId", "questions"], "properties": {
            "id": { "type": "string" }, "sessionId": { "type": "string" }, "messageId": { "type": "string" }, "questions": { "type": "array", "items": {} }
        }},
        "QuestionReply": { "type": "object", "additionalProperties": false, "required": ["answers"], "properties": {
            "answers": { "type": "array", "items": { "type": "array", "items": { "type": "string" } } }
        }},
        "UnknownObject": { "type": "object", "additionalProperties": true },
        "ProviderList": { "type": "array", "items": { "type": "object", "additionalProperties": true } },
        "Agent": { "type": "object", "additionalProperties": true, "required": ["name"], "properties": {
            "name": { "type": "string" }, "description": { "type": "string" }, "mode": { "type": "string" }, "hidden": { "type": "boolean" }, "color": { "type": "string" }
        }},
        "AgentList": { "type": "array", "items": { "$ref": "#/components/schemas/Agent" } },
        "Command": { "type": "object", "additionalProperties": false, "required": ["name"], "properties": {
            "name": { "type": "string" }, "description": { "type": "string" }, "template": { "type": "string" }, "agent": { "type": "string" }, "model": { "type": "string" }, "subtask": { "type": "boolean" }
        }},
        "CommandList": { "type": "array", "items": { "$ref": "#/components/schemas/Command" } },
        "SkillList": { "type": "array", "items": { "type": "object", "additionalProperties": true } },
        "Tool": { "type": "object", "additionalProperties": true, "required": ["id", "description", "parameters"], "properties": {
            "id": { "type": "string" }, "description": { "type": "string" }, "parameters": {}, "outputSchema": {}
        }},
        "ToolList": { "type": "array", "items": { "$ref": "#/components/schemas/Tool" } },
        "ModelRef": { "type": "object", "additionalProperties": false, "required": ["id", "providerId"], "properties": { "id": { "type": "string" }, "providerId": { "type": "string" }, "variant": { "type": "string" } } },
        "UserModel": { "type": "object", "additionalProperties": false, "required": ["providerId", "modelId"], "properties": { "providerId": { "type": "string" }, "modelId": { "type": "string" }, "variant": { "type": "string" } } },
        "PermissionRule": { "type": "object", "additionalProperties": false, "required": ["permission", "pattern", "action"], "properties": { "permission": { "type": "string" }, "pattern": { "type": "string" }, "action": { "type": "string" } } },
        "SessionTime": { "type": "object", "additionalProperties": false, "required": ["created", "updated"], "properties": { "created": { "type": "integer", "minimum": 0 }, "updated": { "type": "integer", "minimum": 0 }, "compacting": { "type": "integer", "minimum": 0 }, "archived": { "type": "integer" } } },
        "Session": { "type": "object", "additionalProperties": true, "required": ["id", "slug", "projectId", "directory", "title", "version", "time"], "properties": {
            "id": { "type": "string" }, "slug": { "type": "string" }, "projectId": { "type": "string" }, "workspaceId": { "type": "string" }, "directory": { "type": "string" },
            "path": { "type": "string" }, "parentId": { "type": "string" }, "title": { "type": "string" }, "agent": { "type": "string" }, "model": { "$ref": "#/components/schemas/ModelRef" },
            "version": { "type": "string" }, "time": { "$ref": "#/components/schemas/SessionTime" }, "permission": { "type": "array", "items": { "$ref": "#/components/schemas/PermissionRule" } }
        }},
        "CreateSessionRequest": { "type": "object", "additionalProperties": false, "properties": { "parentId": { "type": "string" }, "title": { "type": "string" }, "agent": { "type": "string" }, "model": { "$ref": "#/components/schemas/ModelRef" }, "permission": { "type": "array", "items": { "$ref": "#/components/schemas/PermissionRule" } }, "workspaceId": { "type": "string" } } },
        "UpdateSessionRequest": { "type": "object", "additionalProperties": false, "properties": { "title": { "type": "string" }, "agent": { "type": "string" }, "model": { "$ref": "#/components/schemas/ModelRef" }, "directory": { "type": "string" }, "permission": { "type": "array", "items": { "$ref": "#/components/schemas/PermissionRule" } }, "time": { "type": "object", "properties": { "archived": { "type": "integer" } } } } },
        "PageCursor": { "type": "object", "additionalProperties": false, "properties": { "previous": { "type": "string" }, "next": { "type": "string" } } },
        "SessionPage": { "type": "object", "additionalProperties": false, "required": ["items", "cursor"], "properties": { "items": { "type": "array", "items": { "$ref": "#/components/schemas/Session" } }, "cursor": { "$ref": "#/components/schemas/PageCursor" } } },
        "Message": { "type": "object", "additionalProperties": false, "required": ["info", "parts"], "properties": { "info": { "type": "object", "additionalProperties": true }, "parts": { "type": "array", "items": { "type": "object", "additionalProperties": true } } } },
        "MessagePage": { "type": "object", "additionalProperties": false, "required": ["items", "cursor"], "properties": { "items": { "type": "array", "items": { "$ref": "#/components/schemas/Message" } }, "cursor": { "$ref": "#/components/schemas/PageCursor" } } },
        "MessageList": { "type": "array", "items": { "$ref": "#/components/schemas/Message" } },
        "PromptPart": { "oneOf": [
            { "type": "object", "additionalProperties": false, "required": ["type", "text"], "properties": { "type": { "const": "text" }, "text": { "type": "string" } } },
            { "type": "object", "additionalProperties": false, "required": ["type", "name"], "properties": { "type": { "const": "agent" }, "name": { "type": "string" }, "source": {} } },
            { "type": "object", "additionalProperties": false, "required": ["type", "url", "filename", "mime"], "properties": { "type": { "const": "file" }, "url": { "type": "string" }, "filename": { "type": "string" }, "mime": { "type": "string" } } },
            { "type": "object", "additionalProperties": false, "required": ["type", "prompt", "description", "agent"], "properties": { "type": { "const": "subtask" }, "prompt": { "type": "string" }, "description": { "type": "string" }, "agent": { "type": "string" }, "model": { "$ref": "#/components/schemas/UserModel" }, "command": { "type": "string" } } }
        ]},
        "PromptRequest": { "type": "object", "additionalProperties": false, "anyOf": [{ "required": ["prompt"] }, { "required": ["parts"] }], "properties": {
            "prompt": { "type": "string" }, "parts": { "type": "array", "minItems": 1, "items": { "$ref": "#/components/schemas/PromptPart" } }, "delivery": { "type": "string", "enum": ["steer", "queue"], "default": "steer" },
            "messageId": { "type": "string" }, "model": { "$ref": "#/components/schemas/UserModel" }, "agent": { "type": "string" }, "noReply": { "type": "boolean", "default": false },
            "system": { "type": "string" }, "tools": { "type": "object", "additionalProperties": { "type": "boolean" } }, "author": { "type": "string" }, "variant": { "type": "string" }
        }},
        "SubagentTask": { "type": "object", "additionalProperties": false, "required": ["id", "sessionId", "childSessionId", "agent", "status", "description", "nested"], "properties": { "id": { "type": "string" }, "sessionId": { "type": "string" }, "childSessionId": { "type": "string" }, "agent": { "type": "string" }, "status": { "type": "string" }, "description": { "type": "string" }, "result": { "type": "string" }, "nested": { "type": "boolean" } } },
        "StopSubagentsRequest": { "type": "object", "additionalProperties": false, "properties": { "taskId": { "type": "string" } } },
        "StopSubagentsResult": { "type": "object", "additionalProperties": false, "required": ["stopped", "clearedPrompts"], "properties": { "stopped": { "type": "array", "items": { "type": "string" } }, "clearedPrompts": { "type": "integer", "minimum": 0 } } }
    })
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeSet;

    use super::*;

    const ROUTER_SOURCE: &str = include_str!("app_router.rs");

    #[test]
    fn every_v2_router_method_is_in_openapi_and_vice_versa() {
        let router = router_operations(ROUTER_SOURCE);
        let document = canonical_openapi();
        let mut spec = BTreeSet::new();
        for (path, item) in document["paths"].as_object().unwrap() {
            for method in ["get", "post", "put", "patch", "delete"] {
                if item.get(method).is_some() {
                    spec.insert((method.to_uppercase(), normalize_path(path)));
                }
            }
        }
        assert_eq!(router, spec, "the /v2 router and OpenAPI operations drifted");
    }

    #[test]
    fn canonical_document_contains_only_v2_paths() {
        let document = canonical_openapi();
        let paths = document["paths"].as_object().expect("paths object");
        assert!(!paths.is_empty());
        assert!(paths.keys().all(|path| path.starts_with("/v2/")));
    }

    fn router_operations(source: &str) -> BTreeSet<(String, String)> {
        let mut operations = BTreeSet::new();
        let mut offset = 0;
        while let Some(relative) = source[offset..].find(".route(") {
            let start = offset + relative + ".route(".len();
            let Some(end) = matching_paren(source, start) else { break };
            let invocation = &source[start..end];
            let Some(path) = first_string_literal(invocation) else {
                offset = end + 1;
                continue;
            };
            if path.starts_with("/v2/") {
                for (needle, method) in [
                    ("get(", "GET"), ("post(", "POST"), ("put(", "PUT"),
                    ("patch(", "PATCH"), ("delete(", "DELETE"),
                ] {
                    if invocation.contains(needle) {
                        operations.insert((method.to_string(), normalize_path(path)));
                    }
                }
            }
            offset = end + 1;
        }
        operations
    }

    fn matching_paren(source: &str, start: usize) -> Option<usize> {
        let mut depth = 1usize;
        let mut quoted = false;
        let mut escaped = false;
        for (relative, character) in source[start..].char_indices() {
            if quoted {
                if escaped { escaped = false; }
                else if character == '\\' { escaped = true; }
                else if character == '"' { quoted = false; }
                continue;
            }
            match character {
                '"' => quoted = true,
                '(' => depth += 1,
                ')' => {
                    depth -= 1;
                    if depth == 0 { return Some(start + relative); }
                }
                _ => {}
            }
        }
        None
    }

    fn first_string_literal(source: &str) -> Option<&str> {
        let start = source.find('"')? + 1;
        let end = source[start..].find('"')? + start;
        Some(&source[start..end])
    }

    /// Axum uses `:session_id`; OpenAPI uses `{session_id}`. Parameter names
    /// are deliberately ignored so renames do not create false negatives.
    fn normalize_path(path: &str) -> String {
        path.split('/')
            .map(|segment| {
                if segment.starts_with(':')
                    || (segment.starts_with('{') && segment.ends_with('}'))
                {
                    "{}"
                } else {
                    segment
                }
            })
            .collect::<Vec<_>>()
            .join("/")
    }
}
