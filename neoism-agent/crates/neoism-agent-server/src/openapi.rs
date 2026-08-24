use axum::Json;
use serde_json::{json, Value};

pub(crate) async fn openapi_doc() -> Json<Value> {
    Json(openapi_document())
}

pub(crate) async fn canonical_openapi_doc() -> Json<Value> {
    Json(canonical_openapi())
}

/// The canonical contract served from `/v2/openapi.json`. The legacy paths in
/// the historical `/doc` document are retained below, but `/v2` is assembled
/// here so it can be snapshotted and checked independently.
fn openapi_document() -> Value {
    let mut document = handwritten_legacy_document();
    let canonical = canonical_openapi();
    let paths = document["paths"].as_object_mut().expect("paths object");
    paths.retain(|path, _| !path.starts_with("/v2"));
    paths.extend(
        canonical["paths"]
            .as_object()
            .expect("canonical paths")
            .clone(),
    );
    document["components"] = canonical["components"].clone();
    document["tags"] = canonical["tags"].clone();
    document
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
            "messageId": { "type": "string" }, "messageID": { "type": "string", "deprecated": true }, "model": { "$ref": "#/components/schemas/UserModel" }, "agent": { "type": "string" }, "noReply": { "type": "boolean", "default": false },
            "system": { "type": "string" }, "tools": { "type": "object", "additionalProperties": { "type": "boolean" } }, "variant": { "type": "string" }
        }},
        "SubagentTask": { "type": "object", "additionalProperties": false, "required": ["id", "sessionId", "childSessionId", "agent", "status", "description", "nested"], "properties": { "id": { "type": "string" }, "sessionId": { "type": "string" }, "childSessionId": { "type": "string" }, "agent": { "type": "string" }, "status": { "type": "string" }, "description": { "type": "string" }, "result": { "type": "string" }, "nested": { "type": "boolean" } } },
        "StopSubagentsRequest": { "type": "object", "additionalProperties": false, "properties": { "taskId": { "type": "string" } } },
        "StopSubagentsResult": { "type": "object", "additionalProperties": false, "required": ["stopped", "clearedPrompts"], "properties": { "stopped": { "type": "array", "items": { "type": "string" } }, "clearedPrompts": { "type": "integer", "minimum": 0 } } }
    })
}

fn handwritten_legacy_document() -> Value {
    json!({
        "openapi": "3.1.1",
        "info": { "title": "neoism", "version": env!("CARGO_PKG_VERSION"), "description": "neoism headless agent api" },
        "paths": {
            "/v2/meta": { "get": { "operationId": "v2.meta.get", "responses": { "200": { "description": "Server protocol metadata", "content": { "application/json": { "schema": { "$ref": "#/components/schemas/ApiMeta" } } } } } } },
            "/v2/capabilities": { "get": { "operationId": "v2.capability.list", "responses": { "200": { "description": "Enabled and available capabilities", "content": { "application/json": { "schema": { "type": "array", "items": { "$ref": "#/components/schemas/Capability" } } } } } } } },
            "/v2/plugins": { "get": { "operationId": "v2.plugin.list", "responses": { "200": { "description": "Internal and external plugin manifests", "content": { "application/json": { "schema": { "type": "array", "items": { "$ref": "#/components/schemas/PluginManifest" } } } } } } } },
            "/v2/plugins/{pluginID}": { "get": { "operationId": "v2.plugin.get" } },
            "/v2/events": { "get": { "operationId": "v2.event.subscribe", "parameters": [
                { "name": "Last-Event-ID", "in": "header", "schema": { "type": "integer", "minimum": 0 } },
                { "name": "since", "in": "query", "schema": { "type": "integer", "minimum": 0 } },
                { "name": "sessionId", "in": "query", "schema": { "type": "string" } }
            ], "responses": { "200": { "description": "Durable ordered event stream", "content": { "text/event-stream": { "schema": { "$ref": "#/components/schemas/EventEnvelope" } } } } } } },
            "/v2/sessions": { "get": { "operationId": "v2.sessions.list" }, "post": { "operationId": "v2.sessions.create" } },
            "/v2/sessions/{sessionID}": { "get": { "operationId": "v2.sessions.get" }, "patch": { "operationId": "v2.sessions.update" }, "delete": { "operationId": "v2.sessions.delete" } },
            "/v2/sessions/{sessionID}/messages": { "get": { "operationId": "v2.sessions.messages" } },
            "/v2/sessions/{sessionID}/children": { "get": { "operationId": "v2.sessions.children" } },
            "/v2/sessions/{sessionID}/prompt": { "post": { "operationId": "v2.sessions.prompt", "requestBody": { "required": true, "content": { "application/json": { "schema": { "$ref": "#/components/schemas/PromptRequest" } } } } } },
            "/v2/sessions/{sessionID}/prompt-async": { "post": { "operationId": "v2.sessions.promptAsync", "requestBody": { "required": true, "content": { "application/json": { "schema": { "$ref": "#/components/schemas/PromptRequest" } } } } } },
            "/v2/sessions/{sessionID}/abort": { "post": { "operationId": "v2.sessions.abort" } },
            "/v2/sessions/{sessionID}/compact": { "post": { "operationId": "v2.sessions.compact" } },
            "/v2/sessions/{sessionID}/wait": { "post": { "operationId": "v2.sessions.wait" } },
            "/v2/sessions/{sessionID}/context": { "get": { "operationId": "v2.sessions.context" } },
            "/v2/plugins/dev.neoism.subagents/sessions/{sessionID}/tasks": { "get": { "operationId": "v2.subagents.tasks" } },
            "/v2/plugins/dev.neoism.subagents/sessions/{sessionID}/stop": { "post": { "operationId": "v2.subagents.stop" } },
            "/global/health": { "get": { "operationId": "global.health" } },
            "/global/event": { "get": { "operationId": "global.event" } },
            "/global/config": { "get": { "operationId": "global.config.get" }, "patch": { "operationId": "global.config.update" } },
            "/global/dispose": { "post": { "operationId": "global.dispose" } },
            "/global/upgrade": { "post": { "operationId": "global.upgrade" } },
            "/event": { "get": { "operationId": "event.subscribe" } },
            "/instance/dispose": { "post": { "operationId": "instance.dispose" } },
            "/path": { "get": { "operationId": "path.get" } },
            "/vcs": { "get": { "operationId": "vcs.get" } },
            "/vcs/diff": { "get": { "operationId": "vcs.diff" } },
            "/vcs/status": { "get": { "operationId": "vcs.status" } },
            "/vcs/diff/raw": { "get": { "operationId": "vcs.diff.raw" } },
            "/vcs/apply": { "post": { "operationId": "vcs.apply" } },
            "/command": { "get": { "operationId": "command.list" } },
            "/agent": { "get": { "operationId": "app.agents" } },
            "/agent/{name}": { "get": { "operationId": "app.agent" } },
            "/skill": { "get": { "operationId": "app.skills" } },
            "/workflow": { "get": { "operationId": "workflow.list" } },
            "/workflow/{workflowID}": { "get": { "operationId": "workflow.get" } },
            "/workflow/{workflowID}/activate": { "post": { "operationId": "workflow.activate" } },
            "/workflow/{workflowID}/pause": { "post": { "operationId": "workflow.pause" } },
            "/workflow/{workflowID}/run": { "post": { "operationId": "workflow.run" } },
            "/workflow/{workflowID}/preview": { "get": { "operationId": "workflow.preview" } },
            "/workflow/{workflowID}/runs": { "get": { "operationId": "workflow.runs" } },
            "/plugin": { "get": { "operationId": "app.plugins" } },
            "/lsp": { "get": { "operationId": "lsp.status" } },
            "/lsp/hover": { "get": { "operationId": "lsp.hover" } },
            "/lsp/signature-help": { "get": { "operationId": "lsp.signatureHelp" } },
            "/lsp/inlay-hints": { "get": { "operationId": "lsp.inlayHints" } },
            "/lsp/document-highlights": { "get": { "operationId": "lsp.documentHighlights" } },
            "/lsp/definition": { "get": { "operationId": "lsp.definition" } },
            "/lsp/document-symbols": { "get": { "operationId": "lsp.documentSymbols" } },
            "/formatter": { "get": { "operationId": "formatter.status" } },
            "/find": { "get": { "operationId": "find.text" } },
            "/find/file": { "get": { "operationId": "find.files" } },
            "/find/symbol": { "get": { "operationId": "find.symbols" } },
            "/file": { "get": { "operationId": "file.list" } },
            "/file/content": { "get": { "operationId": "file.read" } },
            "/file/status": { "get": { "operationId": "file.status" } },
            "/project": { "get": { "operationId": "project.list" } },
            "/project/current": { "get": { "operationId": "project.current" } },
            "/project/git/init": { "post": { "operationId": "project.initGit" } },
            "/project/{projectID}": { "patch": { "operationId": "project.update" } },
            "/config": { "get": { "operationId": "config.get" }, "patch": { "operationId": "config.update" } },
            "/config/providers": { "get": { "operationId": "config.providers" } },
            "/provider": { "get": { "operationId": "provider.list" } },
            "/provider/auth": { "get": { "operationId": "provider.auth" } },
            "/auth/{providerID}": { "get": { "operationId": "auth.get" }, "put": { "operationId": "auth.set" }, "delete": { "operationId": "auth.remove" } },
            "/provider/{providerID}/oauth/authorize": { "post": { "operationId": "provider.oauth.authorize" } },
            "/provider/{providerID}/oauth/callback": { "post": { "operationId": "provider.oauth.callback" } },
            "/permission": { "get": { "operationId": "permission.list" } },
            "/permission/{requestID}/reply": { "post": { "operationId": "permission.reply" } },
            "/question": { "get": { "operationId": "question.list" } },
            "/question/{requestID}/reply": { "post": { "operationId": "question.reply" } },
            "/question/{requestID}/reject": { "post": { "operationId": "question.reject" } },
            "/pty/shells": { "get": { "operationId": "pty.shells" } },
            "/pty": { "get": { "operationId": "pty.list" }, "post": { "operationId": "pty.create" } },
            "/pty/{ptyID}": { "get": { "operationId": "pty.get" }, "put": { "operationId": "pty.update" }, "delete": { "operationId": "pty.remove" } },
            "/pty/{ptyID}/connect-token": { "post": { "operationId": "pty.connectToken" } },
            "/pty/{ptyID}/connect": { "get": { "operationId": "pty.connect" } },
            "/sync/start": { "post": { "operationId": "sync.start" } },
            "/sync/replay": { "post": { "operationId": "sync.replay" } },
            "/sync/steal": { "post": { "operationId": "sync.steal" } },
            "/sync/history": { "post": { "operationId": "sync.history.list" } },
            "/mcp": { "get": { "operationId": "mcp.status" }, "post": { "operationId": "mcp.add" } },
            "/mcp/{name}/auth": { "post": { "operationId": "mcp.auth.start" }, "delete": { "operationId": "mcp.auth.remove" } },
            "/mcp/{name}/auth/callback": { "get": { "operationId": "mcp.auth.callback.browser" }, "post": { "operationId": "mcp.auth.callback" } },
            "/mcp/{name}/auth/authenticate": { "post": { "operationId": "mcp.auth.authenticate" } },
            "/mcp/{name}/connect": { "post": { "operationId": "mcp.connect" } },
            "/mcp/{name}/disconnect": { "post": { "operationId": "mcp.disconnect" } },
            "/mcp/{name}/tools": { "get": { "operationId": "mcp.tools" } },
            "/mcp/{name}/tools/{toolName}": { "post": { "operationId": "mcp.tool.call" } },
            "/mcp/{name}/resources": { "get": { "operationId": "mcp.resources" } },
            "/mcp/{name}/prompts": { "get": { "operationId": "mcp.prompts" } },
            "/experimental/console": { "get": { "operationId": "experimental.console.get" } },
            "/experimental/console/orgs": { "get": { "operationId": "experimental.console.listOrgs" } },
            "/experimental/console/switch": { "post": { "operationId": "experimental.console.switchOrg" } },
            "/experimental/tool/ids": { "get": { "operationId": "tool.ids" } },
            "/experimental/tool": { "get": { "operationId": "tool.list" } },
            "/experimental/tool/{toolID}/execute": { "post": { "operationId": "tool.execute" } },
            "/experimental/worktree": { "get": { "operationId": "worktree.list" }, "post": { "operationId": "worktree.create" }, "delete": { "operationId": "worktree.remove" } },
            "/experimental/worktree/reset": { "post": { "operationId": "worktree.reset" } },
            "/experimental/session": { "get": { "operationId": "experimental.session.list" } },
            "/experimental/resource": { "get": { "operationId": "experimental.resource.list" } },
            "/session": { "get": { "operationId": "session.list" }, "post": { "operationId": "session.create" } },
            "/session/status": { "get": { "operationId": "session.status" } },
            "/session/{sessionID}": { "get": { "operationId": "session.get" }, "delete": { "operationId": "session.delete" }, "patch": { "operationId": "session.update" } },
            "/session/{sessionID}/children": { "get": { "operationId": "session.children" } },
            "/session/{sessionID}/todo": { "get": { "operationId": "session.todo" } },
            "/session/{sessionID}/init": { "post": { "operationId": "session.init" } },
            "/session/{sessionID}/fork": { "post": { "operationId": "session.fork" } },
            "/session/{sessionID}/abort": { "post": { "operationId": "session.abort" } },
            "/session/{sessionID}/share": { "post": { "operationId": "session.share" }, "delete": { "operationId": "session.unshare" } },
            "/session/{sessionID}/diff": { "get": { "operationId": "session.diff" } },
            "/session/{sessionID}/undo": { "get": { "operationId": "session.undo" } },
            "/session/{sessionID}/undo/tree": { "get": { "operationId": "session.undo.tree" } },
            "/session/{sessionID}/summarize": { "post": { "operationId": "session.summarize" } },
            "/session/{sessionID}/message": { "get": { "operationId": "session.messages" }, "post": { "operationId": "session.prompt" } },
            "/session/{sessionID}/message/{messageID}": { "get": { "operationId": "session.message" }, "delete": { "operationId": "session.deleteMessage" } },
            "/session/{sessionID}/message/{messageID}/part/{partID}": { "delete": { "operationId": "part.delete" }, "patch": { "operationId": "part.update" } },
            "/session/{sessionID}/queue": { "get": { "operationId": "session.queue" }, "delete": { "operationId": "session.queue.clear" } },
            "/session/{sessionID}/queue/pop": { "post": { "operationId": "session.queue.pop" } },
            "/session/{sessionID}/prompt_async": { "post": { "operationId": "session.prompt_async" } },
            "/session/{sessionID}/command": { "post": { "operationId": "session.command" } },
            "/session/{sessionID}/shell": { "post": { "operationId": "session.shell" } },
            "/session/{sessionID}/revert": { "post": { "operationId": "session.revert" } },
            "/session/{sessionID}/unrevert": { "post": { "operationId": "session.unrevert" } },
            "/session/{sessionID}/permissions/{permissionID}": { "post": { "operationId": "permission.respond" } },
            "/api/session": { "get": { "operationId": "v2.session.list" } },
            "/api/session/{sessionID}": { "get": { "operationId": "v2.session.get" }, "delete": { "operationId": "v2.session.delete" }, "patch": { "operationId": "v2.session.update" } },
            "/api/session/{sessionID}/children": { "get": { "operationId": "v2.session.children" } },
            "/api/session/{sessionID}/todo": { "get": { "operationId": "v2.session.todo" } },
            "/api/session/{sessionID}/fork": { "post": { "operationId": "v2.session.fork" } },
            "/api/session/{sessionID}/diff": { "get": { "operationId": "v2.session.diff" } },
            "/api/session/{sessionID}/undo": { "get": { "operationId": "v2.session.undo" } },
            "/api/session/{sessionID}/undo/tree": { "get": { "operationId": "v2.session.undo.tree" } },
            "/api/session/{sessionID}/summarize": { "post": { "operationId": "v2.session.summarize" } },
            "/api/session/{sessionID}/message": { "get": { "operationId": "v2.session.messages" } },
            "/api/session/{sessionID}/message/{messageID}": { "get": { "operationId": "v2.session.message" }, "delete": { "operationId": "v2.session.deleteMessage" } },
            "/api/session/{sessionID}/message/{messageID}/part/{partID}": { "delete": { "operationId": "v2.part.delete" }, "patch": { "operationId": "v2.part.update" } },
            "/api/session/{sessionID}/prompt": { "post": { "operationId": "v2.session.prompt" } },
            "/api/session/{sessionID}/prompt_async": { "post": { "operationId": "v2.session.prompt_async" } },
            "/api/session/{sessionID}/abort": { "post": { "operationId": "v2.session.abort" } },
            "/api/session/{sessionID}/command": { "post": { "operationId": "v2.session.command" } },
            "/api/session/{sessionID}/shell": { "post": { "operationId": "v2.session.shell" } },
            "/api/session/{sessionID}/queue": { "get": { "operationId": "v2.session.queue" }, "delete": { "operationId": "v2.session.queue.clear" } },
            "/api/session/{sessionID}/queue/pop": { "post": { "operationId": "v2.session.queue.pop" } },
            "/api/session/{sessionID}/revert": { "post": { "operationId": "v2.session.revert" } },
            "/api/session/{sessionID}/unrevert": { "post": { "operationId": "v2.session.unrevert" } },
            "/api/session/{sessionID}/compact": { "post": { "operationId": "v2.session.compact" } },
            "/api/session/{sessionID}/wait": { "post": { "operationId": "v2.session.wait" } },
            "/api/session/{sessionID}/context": { "get": { "operationId": "v2.session.context" } }
        },
        "components": {
            "schemas": {
                "ApiMeta": {
                    "type": "object",
                    "required": ["apiVersion", "serverVersion", "pluginApiVersion", "eventSchemaVersion", "partSchemaVersion", "generation"],
                    "properties": {
                        "apiVersion": { "type": "string" },
                        "serverVersion": { "type": "string" },
                        "pluginApiVersion": { "type": "string" },
                        "eventSchemaVersion": { "type": "string" },
                        "partSchemaVersion": { "type": "string" },
                        "generation": { "type": "integer", "minimum": 0 }
                    }
                },
                "Capability": {
                    "type": "object",
                    "required": ["id", "version", "enabled", "disableable", "source"],
                    "properties": {
                        "id": { "type": "string" },
                        "version": { "type": "string" },
                        "enabled": { "type": "boolean" },
                        "disableable": { "type": "boolean" },
                        "source": { "type": "string" },
                        "pluginId": { "type": "string" },
                        "apiPrefix": { "type": "string" },
                        "reason": { "type": "string" }
                    }
                },
                "PluginManifest": {
                    "type": "object",
                    "required": ["id", "name", "version", "pluginApi", "internal", "enabled", "active", "disableable", "capabilities", "requires", "eventNamespaces", "config"],
                    "properties": {
                        "id": { "type": "string" },
                        "name": { "type": "string" },
                        "version": { "type": "string" },
                        "pluginApi": { "type": "string" },
                        "internal": { "type": "boolean" },
                        "enabled": { "type": "boolean" },
                        "active": { "type": "boolean" },
                        "disableable": { "type": "boolean" },
                        "capabilities": { "type": "array", "items": { "type": "string" } },
                        "requires": { "type": "array", "items": { "type": "string" } },
                        "eventNamespaces": { "type": "array", "items": { "type": "string" } },
                        "apiPrefix": { "type": "string" },
                        "reason": { "type": "string" },
                        "config": { "type": "object", "additionalProperties": true }
                    }
                },
                "EventEnvelope": {
                    "type": "object",
                    "required": ["id", "sequence", "type", "source", "schemaVersion", "timestamp", "data"],
                    "properties": {
                        "id": { "type": "string" },
                        "sequence": { "type": "integer", "minimum": 0 },
                        "type": { "type": "string" },
                        "source": { "type": "string" },
                        "schemaVersion": { "type": "string" },
                        "timestamp": { "type": "integer" },
                        "subject": { "type": "object", "required": ["kind", "id"], "properties": { "kind": { "type": "string" }, "id": { "type": "string" } } },
                        "data": {}
                    }
                },
                "PartEnvelope": {
                    "type": "object",
                    "required": ["id", "kind", "schemaVersion", "data"],
                    "properties": {
                        "id": { "type": "string" },
                        "kind": { "type": "string" },
                        "schemaVersion": { "type": "string" },
                        "data": {}
                    }
                },
                "ApiError": {
                    "type": "object",
                    "required": ["code", "message", "retryable", "details"],
                    "properties": {
                        "code": { "type": "string" },
                        "message": { "type": "string" },
                        "retryable": { "type": "boolean" },
                        "requestId": { "type": "string" },
                        "details": { "type": "object", "additionalProperties": true }
                    }
                },
                "PromptRequest": {
                    "type": "object",
                    "required": ["parts"],
                    "properties": {
                        "messageId": { "type": "string" },
                        "messageID": { "type": "string", "deprecated": true },
                        "model": { "$ref": "#/components/schemas/UserModel" },
                        "agent": { "type": "string" },
                        "noReply": { "type": "boolean", "default": false },
                        "system": { "type": "string" },
                        "tools": { "type": "object", "additionalProperties": { "type": "boolean" } },
                        "parts": { "type": "array", "items": { "$ref": "#/components/schemas/PromptPart" } },
                        "prompt": { "type": "string", "description": "v2 convenience field converted to a text part when parts is omitted" },
                        "delivery": { "type": "string", "enum": ["steer", "queue"], "default": "steer" },
                        "variant": { "type": "string" }
                    }
                },
                "PromptPart": {
                    "oneOf": [
                        { "$ref": "#/components/schemas/TextPromptPart" },
                        { "$ref": "#/components/schemas/AgentPromptPart" },
                        { "$ref": "#/components/schemas/FilePromptPart" },
                        { "$ref": "#/components/schemas/SubtaskPromptPart" }
                    ],
                    "discriminator": { "propertyName": "type" }
                },
                "TextPromptPart": {
                    "type": "object",
                    "required": ["type", "text"],
                    "properties": { "type": { "const": "text" }, "text": { "type": "string" } }
                },
                "AgentPromptPart": {
                    "type": "object",
                    "required": ["type", "name"],
                    "properties": {
                        "type": { "const": "agent" },
                        "name": { "type": "string" },
                        "source": { "type": "object" }
                    }
                },
                "FilePromptPart": {
                    "type": "object",
                    "required": ["type", "url", "filename", "mime"],
                    "properties": {
                        "type": { "const": "file" },
                        "url": { "type": "string" },
                        "filename": { "type": "string" },
                        "mime": { "type": "string" }
                    }
                },
                "SubtaskPromptPart": {
                    "type": "object",
                    "required": ["type", "prompt", "description", "agent"],
                    "properties": {
                        "type": { "const": "subtask" },
                        "prompt": { "type": "string" },
                        "description": { "type": "string" },
                        "agent": { "type": "string" },
                        "model": { "$ref": "#/components/schemas/UserModel" },
                        "command": { "type": "string" }
                    }
                },
                "UserModel": {
                    "type": "object",
                    "required": ["providerId", "modelId"],
                    "properties": {
                        "providerId": { "type": "string" },
                        "modelId": { "type": "string" },
                        "variant": { "type": "string" }
                    }
                },
                "Page": {
                    "type": "object",
                    "required": ["items", "cursor"],
                    "properties": {
                        "items": { "type": "array", "items": {} },
                        "cursor": { "type": "object" }
                    }
                }
            }
        }
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
