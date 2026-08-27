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
            // The CONTRACT version, deliberately not the server build version:
            // a release bump must not invalidate the committed spec snapshot.
            "version": neoism_agent_core::API_VERSION,
            "description": "Canonical, versioned Neoism Agent contract. Unknown event and part payloads are intentionally open."
        },
        "tags": [
            { "name": "system" }, { "name": "plugins" }, { "name": "events" },
            { "name": "sessions" }, { "name": "catalog" }, { "name": "artifacts" }, { "name": "interactions" }, { "name": "subagents" }
        ],
        "security": [{ "BearerAuth": [] }],
        "paths": {},
        "components": {
            "securitySchemes": {
                "BearerAuth": { "type": "http", "scheme": "bearer", "bearerFormat": "Neoism caller token" }
            },
            "parameters": {
                "PluginId": { "name": "plugin_id", "in": "path", "required": true, "schema": { "type": "string" } },
                "ArtifactId": { "name": "artifact_id", "in": "path", "required": true, "schema": { "type": "string" } },
                "RequestId": { "name": "request_id", "in": "path", "required": true, "schema": { "type": "string" } },
                "SessionId": { "name": "session_id", "in": "path", "required": true, "schema": { "type": "string" } },
                "Directory": { "name": "directory", "in": "query", "required": false, "schema": { "type": "string" } }
            },
            "responses": {
                "BadRequest": { "description": "Invalid request", "content": { "application/json": { "schema": { "$ref": "#/components/schemas/ApiError" } } } },
                "Unauthorized": { "description": "Authentication required or invalid", "content": { "application/json": { "schema": { "$ref": "#/components/schemas/ApiError" } } } },
                "Forbidden": { "description": "The caller is not authorized for the resource", "content": { "application/json": { "schema": { "$ref": "#/components/schemas/ApiError" } } } },
                "NotFound": { "description": "Resource not found", "content": { "application/json": { "schema": { "$ref": "#/components/schemas/ApiError" } } } },
                "Conflict": { "description": "Resource state conflict", "content": { "application/json": { "schema": { "$ref": "#/components/schemas/ApiError" } } } },
                "UnsupportedMediaType": { "description": "Unsupported or missing request media type", "content": { "text/plain": { "schema": { "type": "string" } } } },
                "UnprocessableEntity": { "description": "Request extraction failed", "content": { "text/plain": { "schema": { "type": "string" } } } },
                "TooManyRequests": { "description": "Caller quota exceeded", "content": { "application/json": { "schema": { "$ref": "#/components/schemas/ApiError" } } } },
                "InternalError": { "description": "Internal server error", "content": { "application/json": { "schema": { "$ref": "#/components/schemas/ApiError" } } } },
                "NotImplemented": { "description": "Feature is not implemented", "content": { "application/json": { "schema": { "$ref": "#/components/schemas/ApiError" } } } }
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
    paths.insert("/v2/plugins/{plugin_id}/manifest".into(), json!({
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
        ("/v2/config/defaults", "get", "v2.config.defaults"),
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
    apply_authoritative_contract(&mut document);
    document
}

fn apply_authoritative_contract(document: &mut Value) {
    let mut paths = serde_json::Map::new();
    let mut add = |path: &str, method: &str, value: Value| {
        paths.entry(path.to_string()).or_insert_with(|| json!({}))[method] = value;
    };
    let r = |name: &str| json!({ "$ref": format!("#/components/schemas/{name}") });
    let p = |name: &str, location: &str, required: bool, schema: Value| json!({
        "name": name, "in": location, "required": required, "schema": schema
    });
    let path = |name: &str| p(name, "path", true, json!({ "type": "string" }));
    let query = |name: &str, required: bool, schema: Value| p(name, "query", required, schema);
    let header = |name: &str, required: bool, schema: Value| p(name, "header", required, schema);
    let directory = || query("directory", false, json!({ "type": "string" }));
    let json_request = |required: bool, schema: Value| json!({
        "required": required, "content": { "application/json": { "schema": schema } }
    });
    let success = |status: &str, description: &str, schema: Value| json!({
        (status): { "description": description, "content": { "application/json": { "schema": schema } } }
    });
    let empty = |status: &str, description: &str| json!({ (status): { "description": description } });
    let op = |id: &str, tag: &str, parameters: Value, request: Option<Value>, responses: Value| {
        let mut value = json!({
            "tags": [tag], "operationId": id, "parameters": parameters,
            "responses": merge_responses(responses, canonical_errors())
        });
        if let Some(request) = request { value["requestBody"] = request; }
        value
    };

    let mut health = op("v2.health", "system", json!([]), None, success("200", "Health", r("HealthResponse")));
    health["security"] = json!([]);
    add("/v2/health", "get", health);
    add("/v2/meta", "get", op("v2.meta.get", "system", json!([]), None, success("200", "Protocol metadata", r("ApiMeta"))));
    add("/v2/openapi.json", "get", op("v2.openapi.get", "system", json!([]), None, success("200", "OpenAPI 3.1 document", r("OpenApiDocument"))));
    add("/v2/audit", "get", op("v2.audit.list", "system", json!([
        query("limit", false, json!({ "type": "integer", "minimum": 1, "maximum": 1000, "default": 100 }))
    ]), None, success("200", "Audit entries", json!({ "type": "array", "items": r("AuditEntry") }))));
    add("/v2/config/defaults", "get", op("v2.config.defaults", "system", json!([directory()]), None, success("200", "Safe effective agent selections", r("ConfigDefaults"))));
    add("/v2/config", "get", op("v2.config.get", "system", json!([directory()]), None, success("200", "Effective agent configuration", r("ConfigDocument"))));
    add("/v2/config", "patch", op("v2.config.update", "system", json!([directory()]), Some(json_request(true, r("ConfigDocument"))), success("200", "Updated configuration", r("ConfigDocument"))));
    add("/v2/config/validate", "get", op("v2.config.validate", "system", json!([directory()]), None, success("200", "Configuration validation", r("ConfigValidation"))));
    add("/v2/capabilities", "get", op("v2.capabilities.list", "plugins", json!([directory()]), None, success("200", "Capabilities", json!({ "type": "array", "items": r("Capability") }))));
    add("/v2/plugins", "get", op("v2.plugins.list", "plugins", json!([directory()]), None, success("200", "Plugin manifests", json!({ "type": "array", "items": r("PluginManifest") }))));
    add("/v2/plugins/{plugin_id}/manifest", "get", op("v2.plugins.get", "plugins", json!([path("plugin_id"), directory()]), None, success("200", "Plugin manifest", r("PluginManifest"))));
    add("/v2/events", "get", op("v2.events.subscribe", "events", json!([
        header("Last-Event-ID", false, json!({ "type": "integer", "minimum": 0 })),
        query("since", false, json!({ "type": "integer", "minimum": 0 })),
        query("tail", false, json!({ "type": "boolean", "default": false })),
        query("limit", false, json!({ "type": "integer", "minimum": 1, "maximum": 5000, "default": 1000 })),
        query("sessionId", false, json!({ "type": "string" }))
    ]), None, json!({ "200": { "description": "Resumable durable event stream", "content": {
        "text/event-stream": { "schema": { "type": "string", "description": "SSE records whose data field is an Event (the typed, type-discriminated union in components/schemas/Event)" } }
    } } })));

    add("/v2/artifacts", "get", op("v2.artifacts.list", "artifacts", json!([
        query("sessionId", false, json!({ "type": "string" }))
    ]), None, success("200", "Artifacts", json!({ "type": "array", "items": r("Artifact") }))));
    add("/v2/artifacts", "post", op("v2.artifacts.create", "artifacts", json!([
        header("Content-Type", false, json!({ "type": "string", "default": "application/octet-stream" })),
        header("X-Neoism-Filename", false, json!({ "type": "string" })),
        header("X-Neoism-Session-Id", false, json!({ "type": "string" }))
    ]), Some(json!({ "required": true, "content": { "application/octet-stream": { "schema": { "type": "string", "format": "binary", "maxLength": 26214400 } } } })), success("201", "Created artifact", r("Artifact"))));
    add("/v2/artifacts/{artifact_id}", "get", op("v2.artifacts.get", "artifacts", json!([path("artifact_id")]), None, success("200", "Artifact metadata", r("Artifact"))));
    add("/v2/artifacts/{artifact_id}", "delete", op("v2.artifacts.delete", "artifacts", json!([path("artifact_id")]), None, empty("204", "Artifact deleted")));
    add("/v2/artifacts/{artifact_id}/content", "get", op("v2.artifacts.content", "artifacts", json!([path("artifact_id")]), None, json!({
        "200": { "description": "Artifact bytes", "headers": {
            "Content-Type": { "schema": { "type": "string" } }, "Content-Disposition": { "schema": { "type": "string" } }, "ETag": { "schema": { "type": "string" } }
        }, "content": { "application/octet-stream": { "schema": { "type": "string", "format": "binary" } } } }
    })));

    add("/v2/interactions/permissions", "get", op("v2.interactions.permissions.list", "interactions", json!([query("sessionId", false, json!({ "type": "string" }))]), None, success("200", "Pending permissions", json!({ "type": "array", "items": r("PermissionRequest") }))));
    add("/v2/interactions/permissions/{request_id}/reply", "post", op("v2.interactions.permissions.reply", "interactions", json!([path("request_id")]), Some(json_request(true, r("PermissionReply"))), success("200", "Reply accepted", json!({ "type": "boolean" }))));
    add("/v2/interactions/questions", "get", op("v2.interactions.questions.list", "interactions", json!([query("sessionId", false, json!({ "type": "string" }))]), None, success("200", "Pending questions", json!({ "type": "array", "items": r("QuestionRequest") }))));
    add("/v2/interactions/questions/{request_id}/reply", "post", op("v2.interactions.questions.reply", "interactions", json!([path("request_id")]), Some(json_request(true, r("QuestionReply"))), success("200", "Reply accepted", json!({ "type": "boolean" }))));
    add("/v2/interactions/questions/{request_id}/reject", "post", op("v2.interactions.questions.reject", "interactions", json!([path("request_id")]), None, success("200", "Rejection accepted", json!({ "type": "boolean" }))));

    for (route, id, schema) in [
        ("/v2/agents", "v2.agents.list", "AgentList"), ("/v2/commands", "v2.commands.list", "CommandList"),
        ("/v2/providers", "v2.providers.list", "ProviderListResult"), ("/v2/providers/configured", "v2.providers.configured", "ConfigProvidersResult"),
        ("/v2/providers/auth-methods", "v2.providers.authMethods", "ProviderAuthMethods"), ("/v2/skills", "v2.skills.list", "SkillList"),
        ("/v2/tools", "v2.tools.list", "ToolList")
    ] { add(route, "get", op(id, "catalog", json!([directory()]), None, success("200", "Catalog result", r(schema)))); }
    add("/v2/agents/{name}", "get", op("v2.agents.get", "catalog", json!([path("name"), directory()]), None, success("200", "Agent", r("Agent"))));
    add("/v2/providers/{provider_id}/auth", "get", op("v2.providers.auth.get", "catalog", json!([path("provider_id")]), None, success("200", "Authentication state", json!({ "anyOf": [r("AuthInfo"), { "type": "null" }] }))));
    add("/v2/providers/{provider_id}/auth", "put", op("v2.providers.auth.set", "catalog", json!([path("provider_id")]), Some(json_request(true, r("AuthInfo"))), success("200", "Authentication updated", json!({ "type": "boolean" }))));
    add("/v2/providers/{provider_id}/auth", "delete", op("v2.providers.auth.delete", "catalog", json!([path("provider_id")]), None, success("200", "Authentication removed", json!({ "type": "boolean" }))));
    add("/v2/providers/{provider_id}/oauth/authorize", "post", op("v2.providers.oauth.authorize", "catalog", json!([path("provider_id")]), Some(json_request(true, r("ProviderAuthorizeRequest"))), success("200", "OAuth authorization", json!({ "anyOf": [r("ProviderAuthAuthorization"), { "type": "null" }] }))));
    add("/v2/providers/{provider_id}/oauth/callback", "post", op("v2.providers.oauth.callback", "catalog", json!([path("provider_id")]), Some(json_request(true, r("ProviderCallbackRequest"))), success("200", "OAuth callback accepted", json!({ "type": "boolean" }))));

    let session_id = || path("session_id");
    add("/v2/sessions", "get", op("v2.sessions.list", "sessions", json!([
        directory(), query("path", false, json!({ "type": "string" })), query("roots", false, json!({ "type": "string" })),
        query("start", false, json!({ "type": "integer", "minimum": 0 })), query("search", false, json!({ "type": "string" })),
        query("limit", false, json!({ "type": "integer", "minimum": 1 }))
    ]), None, success("200", "Session page", r("SessionPage"))));
    add("/v2/sessions", "post", op("v2.sessions.create", "sessions", json!([directory()]), Some(json_request(false, r("CreateSessionRequest"))), success("200", "Created session", r("Session"))));
    add("/v2/sessions/status", "get", op("v2.sessions.status", "sessions", json!([]), None, success("200", "Session status map", r("SessionStatusMap"))));
    add("/v2/sessions/import", "post", op("v2.sessions.import", "sessions", json!([]), Some(json_request(true, r("ImportSessionRequest"))), success("200", "Imported session", r("ImportSessionResponse"))));
    add("/v2/sessions/export", "post", op("v2.sessions.export", "sessions", json!([]), Some(json_request(true, r("ExportSessionsRequest"))), success("200", "Exported sessions", r("ExportSessionsResponse"))));
    add("/v2/sessions/{session_id}", "get", op("v2.sessions.get", "sessions", json!([session_id()]), None, success("200", "Session", r("Session"))));
    add("/v2/sessions/{session_id}", "patch", op("v2.sessions.update", "sessions", json!([session_id()]), Some(json_request(true, r("UpdateSessionRequest"))), success("200", "Updated session", r("Session"))));
    add("/v2/sessions/{session_id}", "delete", op("v2.sessions.delete", "sessions", json!([session_id()]), None, success("200", "Session deleted", json!({ "type": "boolean" }))));
    add("/v2/sessions/{session_id}/messages", "get", op("v2.sessions.messages", "sessions", json!([
        session_id(), query("limit", false, json!({ "type": "integer", "minimum": 1 })), query("order", false, json!({ "type": "string", "enum": ["asc", "desc"] })),
        query("slim", false, json!({ "type": "boolean" })), query("cursor", false, json!({ "type": "string" }))
    ]), None, success("200", "Message page", r("MessagePage"))));
    add("/v2/sessions/{session_id}/messages/{message_id}", "get", op("v2.sessions.messages.get", "sessions", json!([session_id(), path("message_id")]), None, success("200", "Message", r("Message"))));
    add("/v2/sessions/{session_id}/messages/{message_id}", "delete", op("v2.sessions.messages.delete", "sessions", json!([session_id(), path("message_id")]), None, success("200", "Message deleted", json!({ "type": "boolean" }))));
    add("/v2/sessions/{session_id}/messages/{message_id}/parts/{part_id}", "patch", op("v2.sessions.parts.update", "sessions", json!([session_id(), path("message_id"), path("part_id")]), Some(json_request(true, r("Part"))), success("200", "Updated part", r("Part"))));
    add("/v2/sessions/{session_id}/messages/{message_id}/parts/{part_id}", "delete", op("v2.sessions.parts.delete", "sessions", json!([session_id(), path("message_id"), path("part_id")]), None, success("200", "Part deleted", json!({ "type": "boolean" }))));
    add("/v2/sessions/{session_id}/children", "get", op("v2.sessions.children", "sessions", json!([session_id()]), None, success("200", "Child sessions", r("SessionPage"))));
    add("/v2/sessions/{session_id}/runtime", "get", op("v2.sessions.runtime", "sessions", json!([session_id()]), None, success("200", "Authoritative family runtime lifecycle and execution activity", r("SessionRuntimeSnapshot"))));
    add("/v2/sessions/{session_id}/directory-options", "get", op("v2.sessions.directoryOptions", "sessions", json!([session_id(), query("query", false, json!({ "type": "string" })), query("limit", false, json!({ "type": "integer", "minimum": 1, "maximum": 1000 }))]), None, success("200", "Directory options", json!({ "type": "array", "items": { "type": "string" } }))));
    add("/v2/sessions/{session_id}/todos", "get", op("v2.sessions.todos", "sessions", json!([session_id()]), None, success("200", "Session todos", json!({ "type": "array", "items": r("Todo") }))));
    add("/v2/sessions/{session_id}/fork", "post", op("v2.sessions.fork", "sessions", json!([session_id()]), Some(json_request(false, r("ForkSessionRequest"))), success("200", "Forked session", r("Session"))));
    add("/v2/sessions/{session_id}/diff", "get", op("v2.sessions.diff", "sessions", json!([session_id()]), None, success("200", "Session diff", json!({ "type": "array", "items": r("VcsFileDiff") }))));
    add("/v2/sessions/{session_id}/undo-tree", "get", op("v2.sessions.undoTree", "sessions", json!([session_id()]), None, success("200", "Undo tree", r("SessionUndoTree"))));
    for (suffix, id) in [("prompt", "v2.sessions.prompt"), ("prompt-async", "v2.sessions.promptAsync")] {
        add(&format!("/v2/sessions/{{session_id}}/{suffix}"), "post", op(id, "sessions", json!([session_id()]), Some(json_request(true, r("PromptRequest"))), empty("204", "Prompt accepted")));
    }
    add("/v2/sessions/{session_id}/abort", "post", op("v2.sessions.abort", "sessions", json!([session_id()]), None, success("200", "Whether a run was aborted", json!({ "type": "boolean" }))));
    for (suffix, id, description) in [("compact", "v2.sessions.compact", "Context compacted"), ("wait", "v2.sessions.wait", "Session exists")] {
        add(&format!("/v2/sessions/{{session_id}}/{suffix}"), "post", op(id, "sessions", json!([session_id()]), None, empty("204", description)));
    }
    add("/v2/sessions/{session_id}/context", "get", op("v2.sessions.context", "sessions", json!([session_id()]), None, success("200", "Context messages", r("MessageList"))));
    add("/v2/sessions/{session_id}/queue", "get", op("v2.sessions.queue.list", "sessions", json!([session_id()]), None, success("200", "Prompt queue", r("SessionQueueInfo"))));
    add("/v2/sessions/{session_id}/queue", "delete", op("v2.sessions.queue.clear", "sessions", json!([session_id()]), None, success("200", "Queue mutation", r("SessionQueueMutation"))));
    add("/v2/sessions/{session_id}/queue/pop", "post", op("v2.sessions.queue.pop", "sessions", json!([session_id()]), None, success("200", "Queue mutation", r("SessionQueueMutation"))));
    add("/v2/sessions/{session_id}/commands", "post", op("v2.sessions.commands.execute", "sessions", json!([session_id()]), Some(json_request(true, r("SessionCommandRequest"))), success("200", "Command message", r("Message"))));
    add("/v2/sessions/{session_id}/shell", "post", op("v2.sessions.shell", "sessions", json!([session_id()]), Some(json_request(true, r("SessionShellRequest"))), success("200", "Shell message", r("Message"))));
    add("/v2/sessions/{session_id}/revert", "post", op("v2.sessions.revert", "sessions", json!([session_id()]), Some(json_request(true, r("RevertRequest"))), success("200", "Reverted session", r("Session"))));
    add("/v2/sessions/{session_id}/unrevert", "post", op("v2.sessions.unrevert", "sessions", json!([session_id()]), None, success("200", "Restored session", r("Session"))));
    for (suffix, id) in [("undo", "v2.sessions.undo"), ("redo", "v2.sessions.redo")] {
        add(&format!("/v2/sessions/{{session_id}}/{suffix}"), "post", op(id, "sessions", json!([session_id()]), Some(json_request(false, r("RevertRequest"))), success("200", "Updated session", r("Session"))));
    }
    add("/v2/sessions/{session_id}/summarize", "post", op("v2.sessions.summarize", "sessions", json!([session_id()]), Some(json_request(true, r("EmptyObject"))), success("200", "Session summarized", json!({ "type": "boolean" }))));
    add("/v2/sessions/{session_id}/pin", "post", op("v2.sessions.pin", "sessions", json!([session_id()]), Some(json_request(false, r("SetPinRequest"))), success("200", "Updated session", r("Session"))));
    add("/v2/sessions/{session_id}/jobs/{job_id}", "delete", op("v2.sessions.jobs.cancel", "sessions", json!([session_id(), path("job_id")]), None, success("200", "Job stopping", r("BackgroundJobStopResponse"))));

    // Built-in goals and semantic plugins.
    for (method, id, body) in [("get", "v2.plugins.goals.get", None), ("post", "v2.plugins.goals.set", Some(r("SetGoalRequest"))), ("delete", "v2.plugins.goals.clear", None)] {
        add("/v2/plugins/dev.neoism.goals/{session_id}", method, op(id, "plugins", json!([session_id()]), body.map(|schema| json_request(false, schema)), success("200", "Goal state", r("GoalResponse"))));
    }
    add("/v2/plugins/dev.neoism.goals/{session_id}/research", "post", op("v2.plugins.goals.research", "plugins", json!([session_id()]), Some(json_request(true, r("GoalResearchRequest"))), success("200", "Goal state", r("GoalResponse"))));
    add("/v2/plugins/dev.neoism.semantic/search", "get", op("v2.plugins.semantic.search", "plugins", json!([
        query("q", true, json!({ "type": "string" })), query("limit", false, json!({ "type": "integer", "minimum": 1 })), query("sessionId", false, json!({ "type": "string" }))
    ]), None, success("200", "Semantic search results", r("SemanticSearchResponse"))));

    let workflow_params = || json!([directory()]);
    let workflow_id_params = || json!([path("workflow_id"), directory()]);
    add("/v2/plugins/dev.neoism.workflows", "get", op("v2.plugins.workflows.list", "plugins", workflow_params(), None, success("200", "Workflow catalog", r("WorkflowCatalog"))));
    add("/v2/plugins/dev.neoism.workflows/{workflow_id}", "get", op("v2.plugins.workflows.get", "plugins", workflow_id_params(), None, success("200", "Workflow", r("WorkflowView"))));
    for (suffix, id, schema) in [("activate", "v2.plugins.workflows.activate", "WorkflowProjection"), ("pause", "v2.plugins.workflows.pause", "WorkflowProjection"), ("run", "v2.plugins.workflows.run", "WorkflowRun")] {
        add(&format!("/v2/plugins/dev.neoism.workflows/{{workflow_id}}/{suffix}"), "post", op(id, "plugins", workflow_id_params(), None, success("200", "Workflow result", r(schema))));
    }
    add("/v2/plugins/dev.neoism.workflows/{workflow_id}/preview", "get", op("v2.plugins.workflows.preview", "plugins", workflow_id_params(), None, success("200", "Workflow preview", r("WorkflowPreview"))));
    add("/v2/plugins/dev.neoism.workflows/{workflow_id}/runs", "get", op("v2.plugins.workflows.history", "plugins", json!([path("workflow_id"), directory(), query("limit", false, json!({ "type": "integer", "minimum": 1 }))]), None, success("200", "Workflow runs", r("WorkflowHistory"))));

    let lsp_position = || json!([directory(), query("file", true, json!({ "type": "string" })), query("line", true, json!({ "type": "integer", "minimum": 0 })), query("character", true, json!({ "type": "integer", "minimum": 0 }))]);
    let lsp_document = || json!([directory(), query("file", true, json!({ "type": "string" }))]);
    add("/v2/plugins/dev.neoism.lsp", "get", op("v2.plugins.lsp.status", "plugins", json!([directory()]), None, success("200", "LSP status", json!({ "type": "array", "items": r("LspStatus") }))));
    for (suffix, id, schema) in [
        ("hover", "v2.plugins.lsp.hover", "LspHover"), ("signature-help", "v2.plugins.lsp.signatureHelp", "LspSignatureHelp"),
        ("document-highlights", "v2.plugins.lsp.documentHighlights", "LspDocumentHighlight"), ("definition", "v2.plugins.lsp.definition", "LspLocation"),
        ("references", "v2.plugins.lsp.references", "LspLocation"), ("implementation", "v2.plugins.lsp.implementation", "LspLocation"),
        ("prepare-call-hierarchy", "v2.plugins.lsp.prepareCallHierarchy", "LspCallHierarchyItem"), ("incoming-calls", "v2.plugins.lsp.incomingCalls", "LspCallHierarchyCall"),
        ("outgoing-calls", "v2.plugins.lsp.outgoingCalls", "LspCallHierarchyCall"), ("code-actions", "v2.plugins.lsp.codeActions", "UnknownValue")
    ] { add(&format!("/v2/plugins/dev.neoism.lsp/{suffix}"), "get", op(id, "plugins", lsp_position(), None, success("200", "LSP result", json!({ "type": "array", "items": r(schema) })))); }
    add("/v2/plugins/dev.neoism.lsp/inlay-hints", "get", op("v2.plugins.lsp.inlayHints", "plugins", json!([directory(), query("file", true, json!({ "type": "string" })), query("start_line", true, json!({ "type": "integer", "minimum": 0 })), query("end_line", true, json!({ "type": "integer", "minimum": 0 }))]), None, success("200", "Inlay hints", json!({ "type": "array", "items": r("LspInlayHint") }))));
    for (suffix, id, schema) in [("diagnostics", "v2.plugins.lsp.diagnostics", "LspDiagnostic"), ("document-symbols", "v2.plugins.lsp.documentSymbols", "LspDocumentSymbol"), ("formatting", "v2.plugins.lsp.formatting", "UnknownValue")] {
        add(&format!("/v2/plugins/dev.neoism.lsp/{suffix}"), "get", op(id, "plugins", lsp_document(), None, success("200", "LSP result", json!({ "type": "array", "items": r(schema) }))));
    }
    add("/v2/plugins/dev.neoism.lsp/touch", "post", op("v2.plugins.lsp.touch", "plugins", json!([]), Some(json_request(true, r("LspTouchRequest"))), success("200", "Published LSP notifications", json!({ "type": "array", "items": r("UnknownValue") }))));
    add("/v2/plugins/dev.neoism.lsp/shutdown", "post", op("v2.plugins.lsp.shutdown", "plugins", json!([]), None, success("200", "LSP shutdown result", r("LspShutdownResponse"))));

    let mcp_named = || json!([path("name"), directory()]);
    add("/v2/plugins/dev.neoism.mcp", "get", op("v2.plugins.mcp.status", "plugins", json!([directory()]), None, success("200", "MCP status", r("McpStatusMap"))));
    add("/v2/plugins/dev.neoism.mcp", "post", op("v2.plugins.mcp.add", "plugins", json!([]), Some(json_request(true, r("McpAddRequest"))), success("200", "MCP status", r("McpStatusMap"))));
    add("/v2/plugins/dev.neoism.mcp/catalog", "get", op("v2.plugins.mcp.catalog", "plugins", json!([directory()]), None, success("200", "MCP catalog", r("McpCatalog"))));
    add("/v2/plugins/dev.neoism.mcp/{name}/auth", "post", op("v2.plugins.mcp.auth.start", "plugins", mcp_named(), None, success("200", "MCP authorization", r("McpAuthStartResponse"))));
    add("/v2/plugins/dev.neoism.mcp/{name}/auth", "delete", op("v2.plugins.mcp.auth.remove", "plugins", mcp_named(), None, success("200", "MCP credentials removed", r("McpAuthRemoveResponse"))));
    add("/v2/plugins/dev.neoism.mcp/{name}/auth/callback", "get", op("v2.plugins.mcp.auth.callback.get", "plugins", json!([path("name"), query("code", true, json!({ "type": "string" })), query("state", false, json!({ "type": "string" })), directory()]), None, json!({ "200": { "description": "OAuth completion page", "content": { "text/html": { "schema": { "type": "string" } } } } })));
    add("/v2/plugins/dev.neoism.mcp/{name}/auth/callback", "post", op("v2.plugins.mcp.auth.callback.post", "plugins", mcp_named(), Some(json_request(true, r("CodeRequest"))), success("200", "MCP status", r("McpStatus"))));
    add("/v2/plugins/dev.neoism.mcp/{name}/auth/authenticate", "post", op("v2.plugins.mcp.auth.authenticate", "plugins", mcp_named(), None, success("200", "MCP status", r("McpStatus"))));
    for (suffix, id) in [("connect", "v2.plugins.mcp.connect"), ("disconnect", "v2.plugins.mcp.disconnect")] {
        add(&format!("/v2/plugins/dev.neoism.mcp/{{name}}/{suffix}"), "post", op(id, "plugins", mcp_named(), None, success("200", "Connection changed", json!({ "type": "boolean" }))));
    }
    add("/v2/plugins/dev.neoism.mcp/{name}/config", "patch", op("v2.plugins.mcp.config", "plugins", mcp_named(), Some(json_request(true, r("McpConfigPatch"))), success("200", "MCP catalog entry", r("McpCatalogEntry"))));
    add("/v2/plugins/dev.neoism.mcp/{name}/tools", "get", op("v2.plugins.mcp.tools", "plugins", mcp_named(), None, success("200", "MCP tools", json!({ "type": "array", "items": r("McpTool") }))));
    add("/v2/plugins/dev.neoism.mcp/{name}/tools/{tool_name}", "post", op("v2.plugins.mcp.tools.call", "plugins", json!([path("name"), path("tool_name"), directory()]), Some(json_request(true, r("UnknownValue"))), success("200", "MCP tool result", r("McpToolCallResult"))));
    add("/v2/plugins/dev.neoism.mcp/{name}/resources", "get", op("v2.plugins.mcp.resources", "plugins", mcp_named(), None, success("200", "MCP resources", json!({ "type": "array", "items": r("McpResource") }))));
    add("/v2/plugins/dev.neoism.mcp/{name}/prompts", "get", op("v2.plugins.mcp.prompts", "plugins", mcp_named(), None, success("200", "MCP prompts", json!({ "type": "array", "items": r("McpPrompt") }))));

    for (route, id, schema) in [
        ("/v2/plugins/dev.neoism.vcs", "v2.plugins.vcs.get", "VcsInfo"), ("/v2/plugins/dev.neoism.vcs/status", "v2.plugins.vcs.status", "VcsStatusList"),
        ("/v2/plugins/dev.neoism.vcs/diff", "v2.plugins.vcs.diff", "VcsDiffList")
    ] { add(route, "get", op(id, "plugins", json!([directory()]), None, success("200", "VCS result", r(schema)))); }
    add("/v2/plugins/dev.neoism.vcs/diff/raw", "get", op("v2.plugins.vcs.diff.raw", "plugins", json!([directory()]), None, json!({ "200": { "description": "Unified diff", "content": { "text/x-diff": { "schema": { "type": "string" } } } } })));
    add("/v2/plugins/dev.neoism.vcs/apply", "post", op("v2.plugins.vcs.apply", "plugins", json!([]), Some(json_request(true, r("VcsApplyRequest"))), success("200", "Patch result", r("VcsApplyResult"))));

    add("/v2/plugins/dev.neoism.pty/shells", "get", op("v2.plugins.pty.shells", "plugins", json!([]), None, success("200", "Available shells", json!({ "type": "array", "items": r("Shell") }))));
    add("/v2/plugins/dev.neoism.pty", "get", op("v2.plugins.pty.list", "plugins", json!([]), None, success("200", "PTY sessions", json!({ "type": "array", "items": r("Pty") }))));
    add("/v2/plugins/dev.neoism.pty", "post", op("v2.plugins.pty.create", "plugins", json!([directory()]), Some(json_request(true, r("PtyCreateRequest"))), success("200", "Created PTY", r("Pty"))));
    add("/v2/plugins/dev.neoism.pty/{pty_id}", "get", op("v2.plugins.pty.get", "plugins", json!([path("pty_id")]), None, success("200", "PTY", r("Pty"))));
    add("/v2/plugins/dev.neoism.pty/{pty_id}", "put", op("v2.plugins.pty.update", "plugins", json!([path("pty_id")]), Some(json_request(true, r("PtyUpdateRequest"))), success("200", "Updated PTY", r("Pty"))));
    add("/v2/plugins/dev.neoism.pty/{pty_id}", "delete", op("v2.plugins.pty.remove", "plugins", json!([path("pty_id")]), None, success("200", "PTY removed", json!({ "type": "boolean" }))));
    add("/v2/plugins/dev.neoism.pty/{pty_id}/connect-token", "post", op("v2.plugins.pty.connectToken", "plugins", json!([path("pty_id"), header("X-OpenCode-Ticket", true, json!({ "type": "string", "const": "1" }))]), None, success("200", "Single-use connect ticket", r("PtyConnectToken"))));
    let mut websocket = op("v2.plugins.pty.connect", "plugins", json!([path("pty_id"), query("ticket", true, json!({ "type": "string" })), query("cursor", false, json!({ "type": "integer" }))]), None, json!({ "101": { "description": "WebSocket terminal byte stream" } }));
    websocket["x-neoism-transport"] = json!("websocket");
    add("/v2/plugins/dev.neoism.pty/{pty_id}/connect", "get", websocket);

    add("/v2/plugins/dev.neoism.subagents/sessions/{session_id}/tasks", "get", op("v2.subagents.tasks.list", "subagents", json!([session_id()]), None, success("200", "Subagent tasks", json!({ "type": "array", "items": r("SubagentTask") }))));
    add("/v2/plugins/dev.neoism.subagents/sessions/{session_id}/stop", "post", op("v2.subagents.tasks.stop", "subagents", json!([session_id()]), Some(json_request(true, r("StopSubagentsRequest"))), success("200", "Stopped subagents", r("StopSubagentsResult"))));

    for item in paths.values_mut() {
        let Some(item) = item.as_object_mut() else { continue };
        for method in ["get", "post", "put", "patch", "delete"] {
            let Some(operation) = item.get_mut(method) else { continue };
            let Some(parameters) = operation["parameters"].as_array_mut() else { continue };
            if parameters.iter().any(|parameter| parameter["in"] == "query" && parameter["name"] == "directory") {
                parameters.push(header("X-Neoism-Directory", false, json!({ "type": "string" })));
            }
        }
    }
    document["paths"] = Value::Object(paths);
}

fn canonical_errors() -> Value {
    json!({
        "400": { "$ref": "#/components/responses/BadRequest" },
        "401": { "$ref": "#/components/responses/Unauthorized" },
        "403": { "$ref": "#/components/responses/Forbidden" },
        "404": { "$ref": "#/components/responses/NotFound" },
        "409": { "$ref": "#/components/responses/Conflict" },
        "415": { "$ref": "#/components/responses/UnsupportedMediaType" },
        "422": { "$ref": "#/components/responses/UnprocessableEntity" },
        "429": { "$ref": "#/components/responses/TooManyRequests" },
        "500": { "$ref": "#/components/responses/InternalError" },
        "501": { "$ref": "#/components/responses/NotImplemented" }
    })
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
    let mut schemas = json!({
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
        "SessionRuntimeSnapshot": { "type": "object", "additionalProperties": false, "required": ["rootSessionId", "revision", "branches"], "properties": { "rootSessionId": { "type": "string" }, "revision": { "type": "integer", "minimum": 0 }, "branches": { "type": "array", "items": { "$ref": "#/components/schemas/SubtaskLifecycleSnapshot" } }, "execution": { "anyOf": [{ "$ref": "#/components/schemas/ExecutionActivitySnapshot" }, { "type": "null" }] } } },
        "SubtaskLifecycleSnapshot": { "type": "object", "additionalProperties": false, "required": ["sessionId", "parentSessionId", "status"], "properties": { "sessionId": { "type": "string" }, "parentSessionId": { "type": "string" }, "status": { "type": "string", "enum": ["outstanding", "completed", "failed"] }, "startedAt": { "type": ["integer", "null"], "minimum": 0 } } },
        "ExecutionActivitySnapshot": { "type": "object", "additionalProperties": false, "required": ["executionId", "rootSessionId", "rootMessageId", "completedMs", "activeSegments", "revision", "finished"], "properties": { "executionId": { "type": "string" }, "rootSessionId": { "type": "string" }, "rootMessageId": { "type": "string" }, "completedMs": { "type": "integer", "minimum": 0 }, "activeSegments": { "type": "object", "additionalProperties": { "type": "integer", "minimum": 0 } }, "revision": { "type": "integer", "minimum": 0 }, "finished": { "type": "boolean" } } },
        "Message": { "type": "object", "additionalProperties": false, "required": ["info", "parts"], "properties": { "info": { "type": "object", "additionalProperties": true }, "parts": { "type": "array", "items": { "type": "object", "additionalProperties": true } } } },
        "MessagePage": { "type": "object", "additionalProperties": false, "required": ["items", "cursor"], "properties": { "items": { "type": "array", "items": { "$ref": "#/components/schemas/Message" } }, "cursor": { "$ref": "#/components/schemas/PageCursor" } } },
        "MessageList": { "type": "array", "items": { "$ref": "#/components/schemas/Message" } },
        "PromptPart": { "oneOf": [
            { "type": "object", "additionalProperties": false, "required": ["type", "text"], "properties": { "type": { "const": "text" }, "text": { "type": "string" } } },
            { "type": "object", "additionalProperties": false, "required": ["type", "name"], "properties": { "type": { "const": "agent" }, "name": { "type": "string" }, "source": {} } },
            { "type": "object", "additionalProperties": false, "required": ["type", "url", "filename", "mime"], "properties": { "type": { "const": "file" }, "url": { "type": "string" }, "filename": { "type": "string" }, "mime": { "type": "string" } } },
            { "type": "object", "additionalProperties": false, "required": ["type", "prompt", "description", "agent"], "properties": { "type": { "const": "subtask" }, "prompt": { "type": "string" }, "description": { "type": "string" }, "agent": { "type": "string" }, "model": { "$ref": "#/components/schemas/UserModel" }, "command": { "type": "string" } } }
        ]},
        "PromptRequest": { "type": "object", "additionalProperties": false, "anyOf": [
            { "type": "object", "required": ["prompt"], "properties": { "prompt": { "type": "string" } } },
            { "type": "object", "required": ["parts"], "properties": { "parts": { "type": "array", "minItems": 1, "items": { "$ref": "#/components/schemas/PromptPart" } } } }
        ], "properties": {
            "prompt": { "type": "string" }, "parts": { "type": "array", "minItems": 1, "items": { "$ref": "#/components/schemas/PromptPart" } }, "delivery": { "type": "string", "enum": ["steer", "queue"], "default": "steer" },
            "messageId": { "type": "string" }, "model": { "$ref": "#/components/schemas/UserModel" }, "agent": { "type": "string" }, "noReply": { "type": "boolean", "default": false },
            "system": { "type": "string" }, "tools": { "type": "object", "additionalProperties": { "type": "boolean" } }, "author": { "type": "string" }, "variant": { "type": "string" }
        }},
        "SubagentTask": { "type": "object", "additionalProperties": false, "required": ["id", "sessionId", "childSessionId", "agent", "status", "description", "nested"], "properties": { "id": { "type": "string" }, "sessionId": { "type": "string" }, "childSessionId": { "type": "string" }, "agent": { "type": "string" }, "status": { "type": "string" }, "description": { "type": "string" }, "result": { "type": "string" }, "nested": { "type": "boolean" } } },
        "StopSubagentsRequest": { "type": "object", "additionalProperties": false, "properties": { "taskId": { "type": "string" } } },
        "StopSubagentsResult": { "type": "object", "additionalProperties": false, "required": ["stopped", "clearedPrompts"], "properties": { "stopped": { "type": "array", "items": { "type": "string" } }, "clearedPrompts": { "type": "integer", "minimum": 0 } } }
    });
    schemas.as_object_mut().expect("schema object").extend(
        authoritative_schemas().as_object().expect("authoritative schema object").clone()
    );
    schemas.as_object_mut().expect("schema object").extend(
        typed_event_schemas().as_object().expect("typed event schema object").clone()
    );
    schemas.as_object_mut().expect("schema object").extend(
        typed_part_schemas().as_object().expect("typed part schema object").clone()
    );
    schemas
}

/// Discriminated schemas for every message part, mirroring the core `Part`
/// enum's serde serialization exactly (tag = `type`, kebab-case; camelCase
/// fields). Variants allow additional properties so additive server fields
/// never break older validating clients, but every field the server emits
/// today is declared. Exhaustiveness against the Rust enum is enforced by
/// `every_part_variant_has_a_typed_schema`.
fn typed_part_schemas() -> Value {
    let part_base = |tag: &str, required: Value, properties: Value| {
        let mut props = serde_json::Map::new();
        props.insert("type".into(), json!({ "type": "string", "const": tag }));
        props.insert("id".into(), json!({ "type": "string" }));
        props.insert("sessionId".into(), json!({ "type": "string" }));
        props.insert("messageId".into(), json!({ "type": "string" }));
        for (key, value) in properties.as_object().expect("part properties").clone() {
            props.insert(key, value);
        }
        let mut req = vec![json!("type"), json!("id"), json!("sessionId"), json!("messageId")];
        req.extend(required.as_array().expect("part required").clone());
        json!({ "type": "object", "additionalProperties": true, "required": req, "properties": props })
    };
    let r = |name: &str| json!({ "$ref": format!("#/components/schemas/{name}") });
    json!({
        "PartTime": { "type": "object", "additionalProperties": false, "required": ["start"], "properties": {
            "start": { "type": "integer", "minimum": 0 }, "end": { "type": "integer", "minimum": 0 }
        }},
        "CacheUsage": { "type": "object", "additionalProperties": false, "required": ["read", "write"], "properties": {
            "read": { "type": "integer", "minimum": 0 }, "write": { "type": "integer", "minimum": 0 }
        }},
        "TokenUsage": { "type": "object", "additionalProperties": false, "required": ["input", "output", "reasoning", "cache"], "properties": {
            "total": { "type": "integer", "minimum": 0 }, "input": { "type": "integer", "minimum": 0 },
            "output": { "type": "integer", "minimum": 0 }, "reasoning": { "type": "integer", "minimum": 0 },
            "cache": r("CacheUsage")
        }},
        "UserModelRef": { "type": "object", "additionalProperties": false, "required": ["providerId", "modelId"], "properties": {
            "providerId": { "type": "string" }, "modelId": { "type": "string" }, "variant": { "type": "string" }
        }},
        "ToolState": {
            "description": "Tool call lifecycle, discriminated by `status`.",
            "oneOf": [
                { "$ref": "#/components/schemas/ToolStatePending" },
                { "$ref": "#/components/schemas/ToolStateRunning" },
                { "$ref": "#/components/schemas/ToolStateCompleted" },
                { "$ref": "#/components/schemas/ToolStateError" }
            ],
            "discriminator": { "propertyName": "status" }
        },
        "ToolStatePending": { "type": "object", "additionalProperties": true, "required": ["status", "input", "raw"], "properties": {
            "status": { "type": "string", "const": "pending" }, "input": {}, "raw": { "type": "string" }
        }},
        "ToolStateRunning": { "type": "object", "additionalProperties": true, "required": ["status", "input", "time"], "properties": {
            "status": { "type": "string", "const": "running" }, "input": {}, "time": r("PartTime")
        }},
        "ToolStateCompleted": { "type": "object", "additionalProperties": true, "required": ["status", "input", "output", "metadata", "title", "time"], "properties": {
            "status": { "type": "string", "const": "completed" }, "input": {}, "output": { "type": "string" },
            "metadata": {}, "title": { "type": "string" }, "time": r("PartTime")
        }},
        "ToolStateError": { "type": "object", "additionalProperties": true, "required": ["status", "input", "error", "time"], "properties": {
            "status": { "type": "string", "const": "error" }, "input": {}, "error": { "type": "string" }, "time": r("PartTime")
        }},
        "TextPart": part_base("text", json!(["text"]), json!({
            "text": { "type": "string" }, "synthetic": { "type": "boolean" }, "time": r("PartTime")
        })),
        "CompactionPart": part_base("compaction", json!(["reason", "summary"]), json!({
            "reason": { "type": "string" }, "summary": { "type": "boolean" }, "tailStartMessageId": { "type": "string" }
        })),
        "AgentPart": part_base("agent", json!(["name"]), json!({
            "name": { "type": "string" }, "source": {}
        })),
        "SubtaskPart": part_base("subtask", json!(["prompt", "description", "agent"]), json!({
            "prompt": { "type": "string" }, "description": { "type": "string" }, "agent": { "type": "string" },
            "model": r("UserModelRef"), "command": { "type": "string" }
        })),
        "ReasoningPart": part_base("reasoning", json!(["text", "time"]), json!({
            "text": { "type": "string" }, "time": r("PartTime"), "metadata": {}
        })),
        "ToolPart": part_base("tool", json!(["tool", "callId", "state"]), json!({
            "tool": { "type": "string" }, "callId": { "type": "string" }, "state": r("ToolState"), "metadata": {}
        })),
        "StepStartPart": part_base("step-start", json!([]), json!({
            "snapshot": { "type": "string" }
        })),
        "StepFinishPart": part_base("step-finish", json!(["reason", "tokens", "cost"]), json!({
            "reason": { "type": "string" }, "tokens": r("TokenUsage"), "cost": { "type": "number" }, "snapshot": { "type": "string" }
        })),
        "FilePart": part_base("file", json!(["mime", "url"]), json!({
            "mime": { "type": "string" }, "url": { "type": "string" }, "filename": { "type": "string" }
        })),
        "Part": {
            "description": "One message part, discriminated by `type`. Every part the server emits today is a declared variant; variants tolerate additive fields.",
            "oneOf": [
                { "$ref": "#/components/schemas/TextPart" },
                { "$ref": "#/components/schemas/CompactionPart" },
                { "$ref": "#/components/schemas/AgentPart" },
                { "$ref": "#/components/schemas/SubtaskPart" },
                { "$ref": "#/components/schemas/ReasoningPart" },
                { "$ref": "#/components/schemas/ToolPart" },
                { "$ref": "#/components/schemas/StepStartPart" },
                { "$ref": "#/components/schemas/StepFinishPart" },
                { "$ref": "#/components/schemas/FilePart" }
            ],
            "discriminator": { "propertyName": "type" }
        }
    })
}

/// PascalCase component name for one event type: "message.part.delta" →
/// "EventMessagePartDelta".
fn event_variant_name(event_type: &str) -> String {
    let mut name = String::from("Event");
    for segment in event_type.split(['.', '_']) {
        let mut chars = segment.chars();
        if let Some(first) = chars.next() {
            name.extend(first.to_uppercase());
            name.push_str(chars.as_str());
        }
    }
    name
}

/// One `Event` union variant: the full SSE envelope with `type` pinned to a
/// const and `data` typed to that event's payload.
fn event_envelope_variant(event_type: &str, data: Value) -> Value {
    json!({
        "type": "object", "additionalProperties": false,
        "required": ["id", "sequence", "type", "source", "schemaVersion", "timestamp", "data"],
        "properties": {
            "id": { "type": "string" },
            "sequence": { "type": "integer", "minimum": 0 },
            "type": { "const": event_type },
            "source": { "type": "string" },
            "schemaVersion": { "type": "string" },
            "timestamp": { "type": "integer" },
            "subject": { "$ref": "#/components/schemas/EventSubject" },
            "data": data
        }
    })
}

/// Payload (`data`) schema for one published event type. Shapes follow the
/// actual publish sites; a type with multiple publish shapes carries every
/// varying key as optional. Unknown-extension objects stay open on purpose.
fn event_data_schema(event_type: &str) -> Value {
    use neoism_agent_core::event_type as et;
    let r = |name: &str| json!({ "$ref": format!("#/components/schemas/{name}") });
    let nullable = |schema: Value| json!({ "oneOf": [schema, { "type": "null" }] });
    // Every publish site serializes a real core `Part`; the live user-part
    // broadcast additionally injects `role`/`system`/`author`, which the
    // typed variants tolerate via additionalProperties.
    let part = r("Part");
    let message_info = json!({ "type": "object", "additionalProperties": true, "required": ["role", "id", "sessionId"], "properties": {
        "role": { "type": "string" }, "id": { "type": "string" }, "sessionId": { "type": "string" }
    }});
    match event_type {
        _ if event_type == et::MESSAGE_PART_UPDATED => json!({ "type": "object", "additionalProperties": false, "required": ["sessionID", "part", "time"], "properties": {
            "sessionID": { "type": "string" }, "part": part, "time": { "type": "integer" }
        }}),
        _ if event_type == et::MESSAGE_PART_REMOVED => json!({ "type": "object", "additionalProperties": false, "required": ["sessionID", "messageID", "partID"], "properties": {
            "sessionID": { "type": "string" }, "messageID": { "type": "string" }, "partID": { "type": "string" }
        }}),
        _ if event_type == et::MESSAGE_PART_DELTA => json!({ "type": "object", "additionalProperties": false, "required": ["sessionID", "messageID", "partID", "partType", "field", "delta"], "properties": {
            "sessionID": { "type": "string" }, "messageID": { "type": "string" }, "partID": { "type": "string" },
            "partType": { "type": "string" }, "field": { "type": "string" }, "delta": { "type": "string" }
        }}),
        _ if event_type == et::MESSAGE_UPDATED => json!({ "type": "object", "additionalProperties": false, "required": ["sessionID", "info"], "properties": {
            "sessionID": { "type": "string" }, "info": message_info
        }}),
        _ if event_type == et::MESSAGE_REMOVED => json!({ "type": "object", "additionalProperties": false, "required": ["sessionID", "messageID"], "properties": {
            "sessionID": { "type": "string" }, "messageID": { "type": "string" }
        }}),
        _ if event_type == et::MCP_TOOLS_CHANGED => json!({ "type": "object", "additionalProperties": false, "required": ["server", "directory"], "properties": {
            "server": { "type": "string" }, "directory": { "type": "string" }
        }}),
        _ if event_type == et::LSP_UPDATED => json!({ "type": "object", "additionalProperties": false }),
        _ if event_type == et::PERMISSION_ASKED => json!({ "type": "object", "additionalProperties": true, "required": ["id", "sessionId", "messageId", "title", "permission", "patterns", "always"], "properties": {
            "id": { "type": "string" }, "sessionId": { "type": "string" }, "messageId": { "type": "string" },
            "title": { "type": "string" }, "permission": { "type": "string" },
            "patterns": { "type": "array", "items": { "type": "string" } },
            "always": { "type": "array", "items": { "type": "string" } },
            "tool": { "type": "object", "additionalProperties": true },
            "metadata": nullable(json!({ "type": "object", "additionalProperties": true })),
            "sourceSessionID": { "type": "string" }, "sourceTitle": { "type": "string" },
            "sourceAgent": { "type": "string" }, "parentSessionID": { "type": "string" }
        }}),
        _ if event_type == et::PERMISSION_REPLIED => json!({ "type": "object", "additionalProperties": true, "required": ["requestID", "reply"], "properties": {
            "requestID": { "type": "string" }, "reply": { "type": "string" },
            "info": nullable(r("PermissionRequest"))
        }}),
        _ if event_type == et::QUESTION_ASKED => r("QuestionRequest"),
        _ if event_type == et::QUESTION_REJECTED => json!({ "type": "object", "additionalProperties": false, "required": ["requestID"], "properties": {
            "requestID": { "type": "string" }, "reason": { "type": "string" },
            "info": nullable(r("QuestionRequest"))
        }}),
        _ if event_type == et::QUESTION_REPLIED => json!({ "type": "object", "additionalProperties": false, "required": ["requestID", "reply"], "properties": {
            "requestID": { "type": "string" },
            "reply": { "type": "object", "additionalProperties": false, "required": ["answers"], "properties": {
                "answers": { "type": "array", "items": { "type": "array", "items": { "type": "string" } } }
            }},
            "info": nullable(r("QuestionRequest"))
        }}),
        _ if event_type == et::PTY_CREATED || event_type == et::PTY_UPDATED || event_type == et::PTY_DELETED => json!({ "type": "object", "additionalProperties": false, "required": ["id", "ptyID", "info"], "properties": {
            "id": { "type": "string" }, "ptyID": { "type": "string" }, "info": r("Pty")
        }}),
        _ if event_type == et::PTY_EXITED => json!({ "type": "object", "additionalProperties": false, "required": ["id", "ptyID", "exitStatus"], "properties": {
            "id": { "type": "string" }, "ptyID": { "type": "string" },
            "exitStatus": nullable(json!({ "type": "integer" }))
        }}),
        _ if event_type == et::SESSION_COMPACTION_STARTED => json!({ "type": "object", "additionalProperties": false, "required": ["sessionID", "messageID", "timestamp", "reason"], "properties": {
            "sessionID": { "type": "string" }, "messageID": { "type": "string" },
            "timestamp": { "type": "integer" }, "reason": { "type": "string" }
        }}),
        _ if event_type == et::SESSION_COMPACTION_DELTA => json!({ "type": "object", "additionalProperties": false, "required": ["sessionID", "text"], "properties": {
            "sessionID": { "type": "string" }, "text": { "type": "string" }
        }}),
        _ if event_type == et::SESSION_COMPACTION_ENDED => json!({ "type": "object", "additionalProperties": true, "required": ["sessionID"], "properties": {
            "sessionID": { "type": "string" }, "messageID": { "type": "string" }, "timestamp": { "type": "integer" },
            "text": { "type": "string" }, "kind": { "type": "string" }, "status": { "type": "string" },
            "summary": { "type": "object", "additionalProperties": true },
            "error": { "type": "object", "additionalProperties": true }
        }}),
        _ if event_type == et::SESSION_COMPACTED => json!({ "type": "object", "additionalProperties": false, "required": ["sessionID", "info", "summary"], "properties": {
            "sessionID": { "type": "string" }, "info": r("Session"),
            "summary": { "type": "object", "additionalProperties": false, "required": ["text", "messageID", "throughMessageID", "updated", "kind"], "properties": {
                "text": { "type": "string" }, "messageID": { "type": "string" }, "throughMessageID": { "type": "string" },
                "updated": { "type": "integer" }, "kind": { "type": "string" }
            }}
        }}),
        _ if event_type == et::SESSION_CONTEXT_UPDATED => json!({ "type": "object", "additionalProperties": false, "required": ["sessionID", "epoch"], "properties": {
            "sessionID": { "type": "string" }, "epoch": { "type": "object", "additionalProperties": true }
        }}),
        _ if event_type == et::SESSION_CREATED || event_type == et::SESSION_UPDATED => json!({ "type": "object", "additionalProperties": false, "required": ["sessionID", "info"], "properties": {
            "sessionID": { "type": "string" }, "info": r("Session")
        }}),
        _ if event_type == et::SESSION_DELETED => json!({ "type": "object", "additionalProperties": false, "required": ["sessionID"], "properties": {
            "sessionID": { "type": "string" }
        }}),
        _ if event_type == et::SESSION_ERROR => json!({ "type": "object", "additionalProperties": false, "required": ["sessionID", "error"], "properties": {
            "sessionID": { "type": "string" }, "error": r("ApiError")
        }}),
        _ if event_type == et::SESSION_EXECUTION_UPDATED => json!({ "type": "object", "additionalProperties": false, "required": ["sessionID", "snapshot", "runtime"], "properties": {
            "sessionID": { "type": "string" },
            "snapshot": r("ExecutionActivitySnapshot"), "runtime": r("SessionRuntimeSnapshot")
        }}),
        _ if event_type == et::SESSION_BACKGROUND_TASK_COMPLETED => json!({ "type": "object", "additionalProperties": false, "required": ["sessionID", "parentSessionID", "jobID", "taskID", "status", "title", "command", "cwd", "exitCode", "result"], "properties": {
            "sessionID": { "type": "string" }, "parentSessionID": { "type": "string" },
            "jobID": { "type": "string" }, "taskID": { "type": "string" },
            "status": { "type": "string" }, "title": { "type": "string" },
            "command": { "type": "string" }, "cwd": { "type": "string" },
            "exitCode": nullable(json!({ "type": "integer" })), "result": { "type": "string" }
        }}),
        _ if event_type == et::SESSION_QUEUE_UPDATED => json!({ "type": "object", "additionalProperties": false, "required": ["sessionID", "action", "removed", "queue"], "properties": {
            "sessionID": { "type": "string" }, "action": { "type": "string" }, "removed": { "type": "integer" },
            "queue": r("SessionQueueInfo"), "request": r("PromptRequest"),
            "messageID": { "type": "string" }, "delivery": { "type": "string" }
        }}),
        _ if event_type == et::SESSION_PROMPT_ADMITTED => json!({ "type": "object", "additionalProperties": false, "required": ["sessionID", "delivery", "request"], "properties": {
            "sessionID": { "type": "string" }, "delivery": { "type": "string" }, "request": r("PromptRequest")
        }}),
        _ if event_type == et::SESSION_STATUS => json!({ "type": "object", "additionalProperties": false, "required": ["sessionID", "status"], "properties": {
            "sessionID": { "type": "string" }, "status": r("SessionStatus"),
            "runID": { "type": "string" }, "startedAt": { "type": "integer" }, "queue": { "type": "integer" },
            "parentSessionID": { "type": "string" }, "sourceSessionID": { "type": "string" },
            "sourceTitle": { "type": "string" }, "sourceAgent": { "type": "string" }
        }}),
        _ if event_type == et::SESSION_SUBTASK_COMPLETED => json!({ "type": "object", "additionalProperties": false, "required": ["sessionID", "parentSessionID", "childSessionID", "taskID", "status", "title", "result"], "properties": {
            "sessionID": { "type": "string" }, "parentSessionID": { "type": "string" },
            "childSessionID": { "type": "string" }, "taskID": { "type": "string" },
            "status": { "type": "string" }, "title": { "type": "string" }, "result": { "type": "string" },
            "agent": { "type": "string" }, "sourceAgent": { "type": "string" }
        }}),
        _ if event_type == et::TODO_UPDATED => json!({ "type": "object", "additionalProperties": false, "required": ["sessionID", "todos"], "properties": {
            "sessionID": { "type": "string" }, "todos": { "type": "array", "items": r("Todo") }
        }}),
        _ if event_type == et::WORKFLOW_UPDATED => json!({ "type": "object", "additionalProperties": false, "required": ["aggregateID"], "properties": {
            "aggregateID": { "type": "string" }, "workflow": r("WorkflowProjection"),
            "workflowID": { "type": "string" }, "active": { "type": "boolean" }, "error": { "type": "string" }
        }}),
        _ if event_type == et::WORKFLOW_RUN_UPDATED => json!({ "type": "object", "additionalProperties": false, "required": ["aggregateID", "run"], "properties": {
            "aggregateID": { "type": "string" }, "run": r("WorkflowRun")
        }}),
        other => panic!("event type {other} has no payload schema; add one here"),
    }
}

/// The typed `Event` union plus one envelope variant per published event
/// type. Exhaustiveness against `event_type::ALL` is enforced by test.
fn typed_event_schemas() -> Value {
    let mut schemas = serde_json::Map::new();
    let mut variants = Vec::new();
    for event_type in neoism_agent_core::event_type::ALL {
        let name = event_variant_name(event_type);
        variants.push(json!({ "$ref": format!("#/components/schemas/{name}") }));
        schemas.insert(
            name,
            event_envelope_variant(event_type, event_data_schema(event_type)),
        );
    }
    schemas.insert(
        "Event".to_string(),
        json!({
            "description": "Every SSE record's data field, discriminated by `type`.",
            "oneOf": variants,
            "discriminator": { "propertyName": "type" }
        }),
    );
    Value::Object(schemas)
}

fn authoritative_schemas() -> Value {
    let r = |name: &str| json!({ "$ref": format!("#/components/schemas/{name}") });
    json!({
        "UnknownValue": {},
        "EmptyObject": { "type": "object", "additionalProperties": false },
        "OpenApiDocument": { "type": "object", "additionalProperties": true, "required": ["openapi", "info", "paths"], "properties": {
            "openapi": { "type": "string" }, "info": { "type": "object", "additionalProperties": true }, "paths": { "type": "object", "additionalProperties": true }
        }},
        "HealthResponse": { "type": "object", "additionalProperties": false, "required": ["healthy", "version"], "properties": {
            "healthy": { "type": "boolean", "const": true }, "version": { "type": "string" }
        }},
        "ConfigDocument": { "type": "object", "additionalProperties": true, "description": "Canonical agent configuration; extension/plugin keys are preserved." },
        "ConfigDefaults": {
            "type": "object",
            "additionalProperties": false,
            "required": ["defaultAgent", "model", "variant"],
            "properties": {
                "defaultAgent": { "type": ["string", "null"] },
                "model": { "type": ["string", "null"] },
                "variant": { "type": ["string", "null"] }
            }
        },
        "ConfigValidation": { "type": "object", "additionalProperties": false, "required": ["ok", "diagnostics"], "properties": {
            "ok": { "type": "boolean" }, "diagnostics": { "type": "array", "items": r("ConfigDiagnostic") }
        }},
        "ConfigDiagnostic": { "type": "object", "additionalProperties": false, "required": ["level", "path", "message"], "properties": {
            "level": { "type": "string", "enum": ["error", "warning"] }, "path": { "type": "string" }, "message": { "type": "string" }
        }},
        "ProviderListResult": { "type": "object", "additionalProperties": false, "required": ["all", "default", "connected"], "properties": {
            "all": { "type": "array", "items": r("Provider") }, "default": { "type": "object", "additionalProperties": { "type": "string" } },
            "connected": { "type": "array", "items": { "type": "string" } }
        }},
        "ConfigProvidersResult": { "type": "object", "additionalProperties": false, "required": ["providers", "default"], "properties": {
            "providers": { "type": "array", "items": r("Provider") }, "default": { "type": "object", "additionalProperties": { "type": "string" } }
        }},
        "Provider": { "type": "object", "additionalProperties": false, "required": ["id", "name", "source", "env", "options", "models"], "properties": {
            "id": { "type": "string" }, "name": { "type": "string" }, "source": { "type": "string", "enum": ["env", "config", "custom", "api", "builtin"] },
            "env": { "type": "array", "items": { "type": "string" } }, "key": { "type": "string" }, "options": { "type": "object", "additionalProperties": true },
            "models": { "type": "object", "additionalProperties": r("ProviderModel") }
        }},
        "ProviderModel": { "type": "object", "additionalProperties": true, "required": ["id", "providerId", "name", "api", "capabilities", "cost", "limit", "status", "options", "headers", "releaseDate"], "properties": {
            "id": { "type": "string" }, "providerId": { "type": "string" }, "name": { "type": "string" }, "api": { "type": "object", "additionalProperties": true },
            "status": { "type": "string", "enum": ["alpha", "beta", "deprecated", "active"] }, "releaseDate": { "type": "string" }
        }},
        "ProviderAuthMethods": { "type": "object", "additionalProperties": { "type": "array", "items": r("ProviderAuthMethod") } },
        "ProviderAuthMethod": { "type": "object", "additionalProperties": false, "required": ["type", "label"], "properties": {
            "type": { "type": "string", "enum": ["api", "oauth"] }, "label": { "type": "string" }, "prompts": { "type": "array", "items": r("ProviderAuthPrompt") }
        }},
        "ProviderAuthPrompt": { "type": "object", "additionalProperties": true, "required": ["type", "key", "message"], "properties": {
            "type": { "type": "string", "enum": ["text", "select"] }, "key": { "type": "string" }, "message": { "type": "string" }
        }},
        "AuthInfo": { "oneOf": [
            { "type": "object", "additionalProperties": false, "required": ["type", "key"], "properties": { "type": { "const": "api" }, "key": { "type": "string" }, "metadata": {} } },
            { "type": "object", "additionalProperties": false, "required": ["type", "refresh", "access", "expires"], "properties": { "type": { "const": "oauth" }, "refresh": { "type": "string" }, "access": { "type": "string" }, "expires": { "type": "integer", "minimum": 0 }, "accountId": { "type": "string" }, "enterpriseUrl": { "type": "string" } } },
            { "type": "object", "additionalProperties": false, "required": ["type", "key", "token"], "properties": { "type": { "const": "wellknown" }, "key": { "type": "string" }, "token": { "type": "string" } } }
        ]},
        "ProviderAuthorizeRequest": { "type": "object", "additionalProperties": false, "required": ["method"], "properties": {
            "method": {}, "inputs": { "type": "object", "additionalProperties": { "type": "string" } }
        }},
        "ProviderCallbackRequest": { "type": "object", "additionalProperties": false, "required": ["method"], "properties": { "method": {}, "code": { "type": ["string", "null"] } } },
        "ProviderAuthAuthorization": { "type": "object", "additionalProperties": false, "required": ["url", "method", "instructions"], "properties": {
            "url": { "type": "string" }, "method": { "type": "string", "enum": ["auto", "code"] }, "instructions": { "type": "string" }
        }},
        "Skill": { "type": "object", "additionalProperties": false, "required": ["name"], "properties": { "name": { "type": "string" }, "description": { "type": ["string", "null"] }, "path": { "type": "string" } } },
        "SkillList": { "type": "array", "items": r("Skill") },
        "Agent": { "type": "object", "additionalProperties": false, "required": ["name", "mode", "native", "hidden", "permission", "options"], "properties": {
            "name": { "type": "string" }, "description": { "type": "string" }, "mode": { "type": "string" }, "native": { "type": "boolean" }, "hidden": { "type": "boolean" },
            "topP": { "type": "number" }, "temperature": { "type": "number" }, "color": { "type": "string" }, "permission": { "type": "object", "additionalProperties": true },
            "model": r("ModelRef"), "variant": { "type": "string" }, "prompt": { "type": "string" }, "options": { "type": "object", "additionalProperties": true }, "steps": { "type": "integer", "minimum": 0 }
        }},
        "AgentList": { "type": "array", "items": r("Agent") },

        "MessageInfo": { "type": "object", "additionalProperties": true, "required": ["role", "id", "sessionId", "time"], "properties": {
            "role": { "type": "string", "enum": ["user", "assistant"] }, "id": { "type": "string" }, "sessionId": { "type": "string" }, "time": { "type": "object", "additionalProperties": true }
        }},
        "Message": { "type": "object", "additionalProperties": false, "required": ["info", "parts"], "properties": {
            "info": r("MessageInfo"), "parts": { "type": "array", "items": r("Part") }
        }},
        "MessagePage": { "type": "object", "additionalProperties": false, "required": ["items", "cursor"], "properties": { "items": { "type": "array", "items": r("Message") }, "cursor": r("PageCursor") } },
        "MessageList": { "type": "array", "items": r("Message") },
        "SessionStatusMap": { "type": "object", "additionalProperties": r("SessionStatus") },
        "SessionStatus": { "type": "object", "additionalProperties": true, "required": ["type"], "properties": { "type": { "type": "string", "enum": ["idle", "busy", "retry"] } } },
        "SessionBundle": { "type": "object", "additionalProperties": false, "required": ["version", "session", "messages", "queuedPrompts"], "properties": {
            "version": { "type": "integer", "minimum": 1 }, "session": r("Session"), "messages": { "type": "array", "items": r("Message") },
            "queuedPrompts": { "type": "array", "items": r("QueuedPromptBundleItem") }, "workspaceRoot": { "type": "string" }
        }},
        "QueuedPromptBundleItem": { "type": "object", "additionalProperties": false, "required": ["request", "delivery"], "properties": { "request": r("QueuedPrompt"), "delivery": { "type": "string" } } },
        "QueuedPrompt": { "type": "object", "additionalProperties": false, "required": ["parts", "noReply"], "properties": {
            "messageId": { "type": "string" }, "model": r("UserModel"), "agent": { "type": "string" }, "noReply": { "type": "boolean" }, "system": { "type": "string" },
            "tools": { "type": "object", "additionalProperties": { "type": "boolean" } }, "author": { "type": "string" }, "parts": { "type": "array", "items": r("PromptPart") }
        }},
        "ImportSessionRequest": { "type": "object", "additionalProperties": false, "required": ["bundle", "targetWorkspaceRoot"], "properties": { "bundle": r("SessionBundle"), "targetWorkspaceRoot": { "type": "string" } } },
        "ImportSessionResponse": { "type": "object", "additionalProperties": false, "required": ["sessionId"], "properties": { "sessionId": { "type": "string" } } },
        "ExportSessionsRequest": { "type": "object", "additionalProperties": false, "required": ["workspaceRoot"], "properties": { "workspaceRoot": { "type": "string" } } },
        "ExportSessionsResponse": { "type": "object", "additionalProperties": false, "required": ["bundles"], "properties": { "bundles": { "type": "array", "items": r("SessionBundle") } } },
        "ForkSessionRequest": { "type": "object", "additionalProperties": false, "properties": { "messageId": { "type": "string" } } },
        "Todo": { "type": "object", "additionalProperties": false, "required": ["content", "status", "priority"], "properties": { "content": { "type": "string" }, "status": { "type": "string" }, "priority": { "type": "string" } } },
        "VcsFileDiff": { "type": "object", "additionalProperties": false, "required": ["path", "file", "status", "added", "removed", "additions", "deletions", "patch", "hunks"], "properties": {
            "path": { "type": "string" }, "file": { "type": "string" }, "status": { "type": "string" }, "added": { "type": "integer", "minimum": 0 }, "removed": { "type": "integer", "minimum": 0 },
            "additions": { "type": "integer", "minimum": 0 }, "deletions": { "type": "integer", "minimum": 0 }, "patch": { "type": "string" }, "hunks": { "type": "array", "items": {} }
        }},
        "SessionUndoTree": { "type": "object", "additionalProperties": true, "required": ["sessionID", "nodes"], "properties": { "sessionID": { "type": "string" }, "nodes": { "type": "array", "items": { "type": "object", "additionalProperties": true } } } },
        "SessionQueueItem": { "type": "object", "additionalProperties": false, "required": ["index", "noReply", "partCount"], "properties": { "index": { "type": "integer", "minimum": 0 }, "text": { "type": ["string", "null"] }, "noReply": { "type": "boolean" }, "agent": { "type": ["string", "null"] }, "model": { "anyOf": [r("UserModel"), { "type": "null" }] }, "partCount": { "type": "integer", "minimum": 0 } } },
        "SessionQueueInfo": { "type": "object", "additionalProperties": false, "required": ["sessionId", "count", "running", "worker", "items"], "properties": { "sessionId": { "type": "string" }, "count": { "type": "integer", "minimum": 0 }, "running": { "type": "boolean" }, "worker": { "type": "boolean" }, "items": { "type": "array", "items": r("SessionQueueItem") } } },
        "SessionQueueMutation": { "type": "object", "additionalProperties": false, "required": ["sessionId", "removed", "queue"], "properties": { "sessionId": { "type": "string" }, "removed": { "type": "integer", "minimum": 0 }, "queue": r("SessionQueueInfo") } },
        "SessionCommandRequest": { "type": "object", "additionalProperties": false, "required": ["command"], "properties": { "messageId": { "type": "string" }, "model": r("UserModel"), "agent": { "type": "string" }, "command": { "type": "string" }, "arguments": { "type": "string", "default": "" } } },
        "SessionShellRequest": { "type": "object", "additionalProperties": false, "required": ["command"], "properties": { "messageId": { "type": "string" }, "model": r("UserModel"), "agent": { "type": "string" }, "command": { "type": "string" } } },
        "RevertRequest": { "type": "object", "additionalProperties": false, "properties": { "messageId": { "type": "string" }, "partId": { "type": "string" } } },
        "SetPinRequest": { "type": "object", "additionalProperties": false, "properties": { "pinned": { "type": "boolean" } } },
        "BackgroundJobStopResponse": { "type": "object", "additionalProperties": false, "required": ["jobId", "status"], "properties": { "jobId": { "type": "string" }, "status": { "type": "string", "const": "stopping" } } },
        "GoalResearchNote": { "type": "object", "additionalProperties": false, "required": ["source", "content", "captured"], "properties": { "source": { "type": "string" }, "content": { "type": "string" }, "captured": { "type": "integer", "minimum": 0 } } },
        "SessionGoal": { "type": "object", "additionalProperties": false, "required": ["text", "created", "updated", "paused", "status", "summary", "research"], "properties": { "text": { "type": "string" }, "created": { "type": "integer", "minimum": 0 }, "updated": { "type": "integer", "minimum": 0 }, "paused": { "type": "boolean" }, "status": { "type": "string", "enum": ["active", "complete", "blocked"] }, "summary": { "type": "string" }, "research": { "type": "array", "items": r("GoalResearchNote") } } },
        "GoalResponse": { "type": "object", "additionalProperties": false, "required": ["goal", "researchEnabled"], "properties": { "goal": { "anyOf": [r("SessionGoal"), { "type": "null" }] }, "researchEnabled": { "type": "boolean" } } },
        "SetGoalRequest": { "type": "object", "additionalProperties": false, "properties": { "text": { "type": "string", "default": "" }, "researchUrls": { "type": "array", "items": { "type": "string" }, "default": [] }, "paused": { "type": "boolean", "default": false } } },
        "GoalResearchRequest": { "type": "object", "additionalProperties": false, "required": ["url"], "properties": { "url": { "type": "string" } } },
        "SemanticSearchHit": { "type": "object", "additionalProperties": false, "required": ["sessionId", "messageId", "role", "created", "excerpt", "distance"], "properties": { "sessionId": { "type": "string" }, "messageId": { "type": "string" }, "role": { "type": "string" }, "created": { "type": "integer", "minimum": 0 }, "excerpt": { "type": "string" }, "distance": { "type": "number" } } },
        "SemanticSearchResponse": { "type": "object", "additionalProperties": false, "required": ["available", "hits"], "properties": { "available": { "type": "boolean" }, "hits": { "type": "array", "items": r("SemanticSearchHit") } } },
        "WorkflowSchedule": { "type": "object", "additionalProperties": false, "required": ["frequency", "interval", "timezone"], "properties": { "frequency": { "type": "string" }, "interval": { "type": "integer", "minimum": 1 }, "timezone": { "type": "string" }, "minute": { "type": "integer" }, "time": { "type": "string" }, "weekdays": { "type": "array", "items": { "type": "string" } }, "monthDay": { "type": "integer" }, "date": { "type": "string" }, "at": { "type": "string" } } },
        "WorkflowDefinition": { "type": "object", "additionalProperties": false, "required": ["id", "name", "active", "schedule", "prompt"], "properties": { "id": { "type": "string" }, "name": { "type": "string" }, "active": { "type": "boolean" }, "schedule": r("WorkflowSchedule"), "prompt": { "type": "string" }, "directory": { "type": "string" }, "skill": { "type": "string" }, "agent": { "type": "string" }, "model": r("ModelRef"), "permissions": { "type": "object", "additionalProperties": true } } },
        "WorkflowView": { "type": "object", "additionalProperties": false, "required": ["definition", "sourcePath", "sourceHash", "active"], "properties": { "definition": r("WorkflowDefinition"), "sourcePath": { "type": "string" }, "sourceHash": { "type": "string" }, "active": { "type": "boolean" }, "activationID": { "type": ["string", "null"] }, "lastScheduledAt": { "type": ["integer", "null"] } } },
        "WorkflowDiagnostic": { "type": "object", "additionalProperties": false, "required": ["sourcePath", "message"], "properties": { "sourcePath": { "type": "string" }, "message": { "type": "string" } } },
        "WorkflowCatalog": { "type": "object", "additionalProperties": false, "required": ["workflows", "diagnostics"], "properties": { "workflows": { "type": "array", "items": r("WorkflowView") }, "diagnostics": { "type": "array", "items": r("WorkflowDiagnostic") } } },
        "WorkflowProjection": { "type": "object", "additionalProperties": false, "required": ["activationId", "workflowId", "workspaceRoot", "sourcePath", "sourceHash", "definition", "active", "activatedAt", "updated"], "properties": { "activationId": { "type": "string" }, "workflowId": { "type": "string" }, "workspaceRoot": { "type": "string" }, "sourcePath": { "type": "string" }, "sourceHash": { "type": "string" }, "definition": r("WorkflowDefinition"), "active": { "type": "boolean" }, "activatedAt": { "type": "integer" }, "lastScheduledAt": { "type": ["integer", "null"] }, "updated": { "type": "integer" } } },
        "WorkflowRun": { "type": "object", "additionalProperties": false, "required": ["id", "activationId", "workflowId", "scheduledAt", "status", "trigger", "created"], "properties": { "id": { "type": "string" }, "activationId": { "type": "string" }, "workflowId": { "type": "string" }, "scheduledAt": { "type": "integer" }, "startedAt": { "type": ["integer", "null"] }, "finishedAt": { "type": ["integer", "null"] }, "sessionId": { "type": ["string", "null"] }, "status": { "type": "string" }, "trigger": { "type": "string" }, "error": { "type": ["string", "null"] }, "created": { "type": "integer" } } },
        "WorkflowPreview": { "type": "object", "additionalProperties": false, "required": ["definition", "sourcePath", "upcoming"], "properties": { "definition": r("WorkflowDefinition"), "sourcePath": { "type": "string" }, "upcoming": { "type": "array", "items": { "type": "object", "additionalProperties": false, "required": ["scheduledAt", "local"], "properties": { "scheduledAt": { "type": "integer" }, "local": { "type": "string" } } } } } },
        "WorkflowHistory": { "type": "object", "additionalProperties": false, "required": ["runs"], "properties": { "runs": { "type": "array", "items": r("WorkflowRun") } } },
        "LspPosition": { "type": "object", "additionalProperties": false, "required": ["line", "character"], "properties": { "line": { "type": "integer" }, "character": { "type": "integer" } } },
        "LspRange": { "type": "object", "additionalProperties": false, "required": ["start", "end"], "properties": { "start": r("LspPosition"), "end": r("LspPosition") } },
        "LspLocation": { "type": "object", "additionalProperties": false, "required": ["path"], "properties": { "path": { "type": "string" }, "range": { "anyOf": [r("LspRange"), { "type": "null" }] }, "language": { "type": ["string", "null"] } } },
        "LspHover": { "type": "object", "additionalProperties": false, "required": ["path", "contents"], "properties": { "path": { "type": "string" }, "contents": { "type": "string" }, "kind": { "type": ["string", "null"] }, "range": { "anyOf": [r("LspRange"), { "type": "null" }] }, "language": { "type": ["string", "null"] } } },
        "LspStatus": { "type": "object", "additionalProperties": true, "required": ["id", "name", "status", "language", "command", "command_source", "workspace", "capabilities", "detected"], "properties": { "id": { "type": "string" }, "name": { "type": "string" }, "status": { "type": "string", "enum": ["available", "connected", "error"] }, "language": { "type": "string" }, "command": { "type": "array", "items": { "type": "string" } }, "command_source": { "type": "string" } } },
        "LspSignatureHelp": { "type": "object", "additionalProperties": true, "required": ["path", "signatures"], "properties": { "path": { "type": "string" }, "signatures": { "type": "array", "items": { "type": "object", "additionalProperties": true } } } },
        "LspInlayHint": { "type": "object", "additionalProperties": false, "required": ["path", "line", "character", "label", "padding_left", "padding_right"], "properties": { "path": { "type": "string" }, "line": { "type": "integer" }, "character": { "type": "integer" }, "label": { "type": "string" }, "kind": { "type": ["string", "null"] }, "padding_left": { "type": "boolean" }, "padding_right": { "type": "boolean" }, "language": { "type": ["string", "null"] } } },
        "LspDocumentHighlight": { "type": "object", "additionalProperties": false, "required": ["path"], "properties": { "path": { "type": "string" }, "range": { "anyOf": [r("LspRange"), { "type": "null" }] }, "kind": { "type": ["string", "null"] }, "language": { "type": ["string", "null"] } } },
        "LspCallHierarchyItem": { "type": "object", "additionalProperties": true, "required": ["name", "kind", "path"], "properties": { "name": { "type": "string" }, "kind": { "type": "string" }, "path": { "type": "string" } } },
        "LspCallHierarchyCall": { "type": "object", "additionalProperties": false, "required": ["item", "ranges", "direction"], "properties": { "item": r("LspCallHierarchyItem"), "ranges": { "type": "array", "items": r("LspRange") }, "direction": { "type": "string" }, "language": { "type": ["string", "null"] } } },
        "LspDiagnostic": { "type": "object", "additionalProperties": false, "required": ["path", "severity", "message", "tags", "related_information"], "properties": { "path": { "type": "string" }, "range": { "anyOf": [r("LspRange"), { "type": "null" }] }, "severity": { "type": "string" }, "code": { "type": ["string", "null"] }, "message": { "type": "string" }, "tags": { "type": "array", "items": { "type": "string" } }, "related_information": { "type": "array", "items": { "type": "object", "additionalProperties": true } }, "data": {}, "language": { "type": ["string", "null"] } } },
        "LspDocumentSymbol": { "type": "object", "additionalProperties": true, "required": ["name", "kind", "path", "children"], "properties": { "name": { "type": "string" }, "kind": { "type": "string" }, "path": { "type": "string" }, "children": { "type": "array", "items": r("LspDocumentSymbol") } } },
        "LspTouchRequest": { "type": "object", "additionalProperties": false, "required": ["file"], "properties": { "directory": { "type": "string" }, "file": { "type": "string" }, "text": { "type": ["string", "null"] } } },
        "LspShutdownResponse": { "type": "object", "additionalProperties": false, "required": ["shutdown"], "properties": { "shutdown": { "type": "boolean", "const": true } } },
        "McpStatus": { "oneOf": [
            { "type": "object", "additionalProperties": false, "required": ["status"], "properties": { "status": { "enum": ["connected", "disabled", "needs_auth"] } } },
            { "type": "object", "additionalProperties": false, "required": ["status", "error"], "properties": { "status": { "enum": ["failed", "needs_client_registration"] }, "error": { "type": "string" } } }
        ]},
        "McpStatusMap": { "type": "object", "additionalProperties": r("McpStatus") },
        "McpConfig": { "type": "object", "additionalProperties": true, "required": ["type"], "properties": { "type": { "type": "string", "enum": ["local", "remote"] } } },
        "McpAddRequest": { "type": "object", "additionalProperties": false, "required": ["name", "config"], "properties": { "name": { "type": "string" }, "config": r("McpConfig") } },
        "McpCatalogEntry": { "type": "object", "additionalProperties": false, "required": ["status", "enabled", "runtimeConnected", "oauthCapable", "hasCredentials", "configWritable"], "properties": { "status": r("McpStatus"), "enabled": { "type": "boolean" }, "runtimeConnected": { "type": "boolean" }, "oauthCapable": { "type": "boolean" }, "hasCredentials": { "type": "boolean" }, "configWritable": { "type": "boolean" } } },
        "McpCatalog": { "type": "object", "additionalProperties": r("McpCatalogEntry") },
        "McpAuthStartResponse": { "type": "object", "additionalProperties": false, "required": ["authorizationUrl", "oauthState"], "properties": { "authorizationUrl": { "type": "string" }, "oauthState": { "type": "string" } } },
        "McpAuthRemoveResponse": { "type": "object", "additionalProperties": false, "required": ["success"], "properties": { "success": { "type": "boolean" } } },
        "CodeRequest": { "type": "object", "additionalProperties": false, "required": ["code"], "properties": { "code": { "type": "string" } } },
        "McpConfigPatch": { "type": "object", "additionalProperties": false, "required": ["enabled"], "properties": { "enabled": { "type": "boolean" } } },
        "McpTool": { "type": "object", "additionalProperties": false, "required": ["name", "inputSchema", "client"], "properties": { "name": { "type": "string" }, "description": { "type": "string" }, "inputSchema": {}, "client": { "type": "string" }, "annotations": {} } },
        "McpToolCallResult": { "type": "object", "additionalProperties": false, "required": ["content"], "properties": { "content": { "type": "array", "items": { "type": "object", "additionalProperties": true, "required": ["type"], "properties": { "type": { "type": "string" } } } }, "isError": { "type": "boolean" } } },
        "McpResource": { "type": "object", "additionalProperties": false, "required": ["name", "uri", "client"], "properties": { "name": { "type": "string" }, "uri": { "type": "string" }, "description": { "type": "string" }, "mimeType": { "type": "string" }, "client": { "type": "string" } } },
        "McpPrompt": { "type": "object", "additionalProperties": false, "required": ["name", "arguments", "client"], "properties": { "name": { "type": "string" }, "description": { "type": "string" }, "arguments": { "type": "array", "items": { "type": "object", "additionalProperties": false, "required": ["name", "required"], "properties": { "name": { "type": "string" }, "description": { "type": "string" }, "required": { "type": "boolean" } } } }, "client": { "type": "string" } } },
        "VcsInfo": { "type": "object", "additionalProperties": false, "properties": { "branch": { "type": ["string", "null"] }, "default_branch": { "type": ["string", "null"] } } },
        "VcsFileStatus": { "type": "object", "additionalProperties": false, "required": ["path", "file", "status", "additions", "deletions"], "properties": { "path": { "type": "string" }, "file": { "type": "string" }, "status": { "type": "string" }, "additions": { "type": "integer" }, "deletions": { "type": "integer" } } },
        "VcsStatusList": { "type": "array", "items": r("VcsFileStatus") }, "VcsDiffList": { "type": "array", "items": r("VcsFileDiff") },
        "VcsApplyRequest": { "type": "object", "additionalProperties": true, "required": ["patch"], "properties": { "patch": { "type": "string" }, "directory": { "type": "string" } } },
        "VcsApplyResult": { "type": "object", "additionalProperties": false, "required": ["success"], "properties": { "success": { "type": "boolean" }, "error": { "type": ["string", "null"] } } },
        "Shell": { "type": "object", "additionalProperties": false, "required": ["path", "name", "acceptable"], "properties": { "path": { "type": "string" }, "name": { "type": "string" }, "acceptable": { "type": "boolean" } } },
        "Pty": { "type": "object", "additionalProperties": false, "required": ["id", "command", "cwd", "title", "time"], "properties": { "id": { "type": "string" }, "command": { "type": "array", "items": { "type": "string" } }, "cwd": { "type": "string" }, "title": { "type": "string" }, "time": { "type": "integer", "minimum": 0 } } },
        "PtyCreateRequest": { "type": "object", "additionalProperties": false, "properties": { "command": { "type": "array", "items": { "type": "string" } }, "cwd": { "type": "string" }, "title": { "type": "string" } } },
        "PtyUpdateRequest": { "type": "object", "additionalProperties": false, "properties": { "title": { "type": "string" }, "cwd": { "type": "string" }, "size": { "type": "object", "additionalProperties": false, "required": ["cols", "rows"], "properties": { "cols": { "type": "integer", "minimum": 1 }, "rows": { "type": "integer", "minimum": 1 } } } } },
        "PtyConnectToken": { "type": "object", "additionalProperties": false, "required": ["ticket", "expires_in"], "properties": { "ticket": { "type": "string" }, "expires_in": { "type": "integer", "minimum": 0 } } }
    })
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeSet;

    use super::*;

    const ROUTER_SOURCE: &str = include_str!("app_router.rs");

    #[test]
    fn every_central_v2_router_method_is_in_openapi_and_vice_versa() {
        let router = router_operations(ROUTER_SOURCE);
        let document = canonical_openapi();
        let mut spec = BTreeSet::new();
        for (path, item) in document["paths"].as_object().unwrap() {
            for method in ["get", "post", "put", "patch", "delete"] {
                if item.get(method).is_some() {
                    let path = normalize_path(path);
                    if !plugin_owned_path(&path) {
                        spec.insert((method.to_uppercase(), path));
                    }
                }
            }
        }
        assert_eq!(router, spec, "the /v2 router and OpenAPI operations drifted");
    }

    fn plugin_owned_path(path: &str) -> bool {
        path.starts_with("/v2/plugins/dev.neoism.")
            || ["/v2/agents", "/v2/commands", "/v2/config", "/v2/providers", "/v2/skills"]
                .iter()
                .any(|prefix| path == *prefix || path.starts_with(&format!("{prefix}/")))
    }

    /// The other half of the parity contract: every route a production plugin
    /// snapshot registers must be documented, and every plugin-owned operation
    /// in the spec must still be served by a registered descriptor. Without
    /// this, the ~half of the API dispatched through the plugin fallback could
    /// be renamed or deleted with no test noticing.
    #[tokio::test]
    async fn every_plugin_route_descriptor_is_in_openapi_and_vice_versa() {
        let root = std::env::temp_dir().join(format!(
            "neoism-openapi-plugin-parity-{}",
            neoism_agent_core::Id::ascending(neoism_agent_core::IdKind::Event)
        ));
        std::fs::create_dir_all(&root).unwrap();
        let state = crate::state::AppState::open_database(root.join("state.sqlite3"))
            .await
            .unwrap();
        let snapshot = state
            .plugin_snapshot(&root.to_string_lossy())
            .await;

        let mut descriptors = BTreeSet::new();
        for registered in snapshot.runtime_routes.values() {
            descriptors.insert((
                registered.route.descriptor.method.as_str().to_string(),
                normalize_path(&registered.route.descriptor.path),
            ));
        }
        for registered in snapshot.runtime_websocket_routes.values() {
            descriptors.insert((
                registered.route.descriptor.method.as_str().to_string(),
                normalize_path(&registered.route.descriptor.path),
            ));
        }
        // Release the generation lease before shutdown or the drain times out.
        drop(snapshot);

        let document = canonical_openapi();
        let mut spec = BTreeSet::new();
        for (path, item) in document["paths"].as_object().unwrap() {
            for method in ["get", "post", "put", "patch", "delete"] {
                if item.get(method).is_some() {
                    let path = normalize_path(path);
                    if plugin_owned_path(&path) {
                        spec.insert((method.to_uppercase(), path));
                    }
                }
            }
        }

        let missing_from_spec = descriptors.difference(&spec).collect::<Vec<_>>();
        let missing_from_snapshot = spec.difference(&descriptors).collect::<Vec<_>>();
        assert!(
            missing_from_spec.is_empty() && missing_from_snapshot.is_empty(),
            "plugin routes and OpenAPI drifted.\nregistered but undocumented: {missing_from_spec:#?}\ndocumented but unregistered: {missing_from_snapshot:#?}"
        );

        state.shutdown().await.unwrap();
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn every_part_variant_has_a_typed_schema_and_samples_validate() {
        use neoism_agent_core::{
            AgentPart, CacheUsage, CompactionPart, FilePart, Part, PartTime, ReasoningPart,
            StepFinishPart, StepStartPart, SubtaskPart, TextPart, TokenUsage, ToolPart,
            ToolState,
        };
        let id = || neoism_agent_core::Id::ascending(neoism_agent_core::IdKind::Part);
        let sid = neoism_agent_core::new_session_id();
        let mid = neoism_agent_core::Id::ascending(neoism_agent_core::IdKind::Message);
        let time = PartTime { start: 1, end: Some(2) };
        let tool = |state: ToolState| {
            Part::Tool(ToolPart {
                id: id(), session_id: sid.clone(), message_id: mid.clone(),
                tool: "bash".into(), call_id: "call".into(), state,
                metadata: Some(serde_json::json!({"x": 1})),
            })
        };
        // One sample per Part variant (plus every ToolState) — adding a new
        // variant to the core enum without extending this list is a compile
        // error via the match below.
        let samples = vec![
            Part::Text(TextPart { id: id(), session_id: sid.clone(), message_id: mid.clone(), text: "t".into(), synthetic: Some(true), time: Some(time.clone()) }),
            Part::Compaction(CompactionPart { id: id(), session_id: sid.clone(), message_id: mid.clone(), reason: "auto".into(), summary: true, tail_start_message_id: Some(mid.clone()) }),
            Part::Agent(AgentPart { id: id(), session_id: sid.clone(), message_id: mid.clone(), name: "build".into(), source: None }),
            Part::Subtask(SubtaskPart { id: id(), session_id: sid.clone(), message_id: mid.clone(), prompt: "p".into(), description: "d".into(), agent: "general".into(), model: None, command: None }),
            Part::Reasoning(ReasoningPart { id: id(), session_id: sid.clone(), message_id: mid.clone(), text: "r".into(), time: time.clone(), metadata: None }),
            tool(ToolState::Pending { input: serde_json::json!({}), raw: String::new() }),
            tool(ToolState::Running { input: serde_json::json!({}), time: time.clone() }),
            tool(ToolState::Completed { input: serde_json::json!({}), output: "ok".into(), metadata: serde_json::json!({}), title: "Bash".into(), time: time.clone() }),
            tool(ToolState::Error { input: serde_json::json!({}), error: "boom".into(), time: time.clone() }),
            Part::StepStart(StepStartPart { id: id(), session_id: sid.clone(), message_id: mid.clone(), snapshot: None }),
            Part::StepFinish(StepFinishPart { id: id(), session_id: sid.clone(), message_id: mid.clone(), reason: "stop".into(), tokens: TokenUsage { total: Some(3), input: 1, output: 1, reasoning: 1, cache: CacheUsage { read: 0, write: 0 } }, cost: 0.01, snapshot: None }),
            Part::File(FilePart { id: id(), session_id: sid.clone(), message_id: mid.clone(), mime: "text/plain".into(), url: "artifact://a".into(), filename: None }),
        ];
        // Exhaustiveness: the compiler forces this match to grow with the enum.
        for part in &samples {
            match part {
                Part::Text(_) | Part::Compaction(_) | Part::Agent(_) | Part::Subtask(_)
                | Part::Reasoning(_) | Part::Tool(_) | Part::StepStart(_)
                | Part::StepFinish(_) | Part::File(_) => {}
            }
        }

        let schemas = typed_part_schemas();
        let union_tags: Vec<String> = schemas["Part"]["oneOf"]
            .as_array()
            .unwrap()
            .iter()
            .map(|variant| {
                let name = variant["$ref"].as_str().unwrap().rsplit('/').next().unwrap();
                schemas[name]["properties"]["type"]["const"].as_str().unwrap().to_string()
            })
            .collect();
        assert_eq!(union_tags.len(), 9, "one union variant per Part variant");

        for part in &samples {
            let value = serde_json::to_value(part).unwrap();
            let tag = value["type"].as_str().unwrap();
            let schema_name = schemas["Part"]["oneOf"]
                .as_array().unwrap().iter()
                .map(|variant| variant["$ref"].as_str().unwrap().rsplit('/').next().unwrap())
                .find(|name| schemas[name]["properties"]["type"]["const"] == tag)
                .unwrap_or_else(|| panic!("no schema variant for serialized tag {tag}"));
            let schema = &schemas[schema_name];
            for required in schema["required"].as_array().unwrap() {
                let field = required.as_str().unwrap();
                assert!(
                    value.get(field).is_some(),
                    "{schema_name}: serialized {tag} part missing required field {field}: {value}"
                );
            }
            // Every serialized field must be declared, so the schema never
            // silently under-describes what the server actually emits.
            for (field, _) in value.as_object().unwrap() {
                assert!(
                    schema["properties"].get(field).is_some(),
                    "{schema_name}: emitted field {field} is not declared in the schema"
                );
            }
            if tag == "tool" {
                let state = &value["state"];
                let status = state["status"].as_str().unwrap();
                let state_schema_name = schemas["ToolState"]["oneOf"]
                    .as_array().unwrap().iter()
                    .map(|variant| variant["$ref"].as_str().unwrap().rsplit('/').next().unwrap())
                    .find(|name| schemas[name]["properties"]["status"]["const"] == status)
                    .unwrap_or_else(|| panic!("no ToolState schema for status {status}"));
                for required in schemas[state_schema_name]["required"].as_array().unwrap() {
                    let field = required.as_str().unwrap();
                    assert!(
                        state.get(field).is_some(),
                        "{state_schema_name}: tool state missing required {field}: {state}"
                    );
                }
            }
        }
    }

    #[test]
    fn every_published_event_type_has_exactly_one_typed_union_variant() {
        let document = canonical_openapi();
        let schemas = document["components"]["schemas"]
            .as_object()
            .expect("schemas");
        let union = schemas["Event"]["oneOf"].as_array().expect("Event oneOf");
        let mut variant_types = BTreeSet::new();
        for reference in union {
            let name = reference["$ref"]
                .as_str()
                .expect("variant ref")
                .split('/')
                .next_back()
                .expect("ref name");
            let variant = &schemas[name];
            let pinned = variant["properties"]["type"]["const"]
                .as_str()
                .unwrap_or_else(|| panic!("variant {name} does not pin `type`"));
            assert!(
                variant_types.insert(pinned.to_string()),
                "duplicate Event variant for {pinned}"
            );
            let data = &variant["properties"]["data"];
            assert!(
                data.is_object() && !data.as_object().unwrap().is_empty(),
                "variant {name} has an untyped data payload"
            );
        }
        let published = neoism_agent_core::event_type::ALL
            .iter()
            .map(ToString::to_string)
            .collect::<BTreeSet<_>>();
        assert_eq!(
            variant_types, published,
            "the typed Event union and event_type::ALL drifted"
        );
    }

    #[test]
    fn canonical_document_contains_only_v2_paths() {
        let document = canonical_openapi();
        let paths = document["paths"].as_object().expect("paths object");
        assert!(!paths.is_empty());
        assert!(paths.keys().all(|path| path.starts_with("/v2/")));
    }

    #[test]
    fn every_operation_has_a_unique_id_and_success_response() {
        let document = canonical_openapi();
        let mut ids = BTreeSet::new();
        for (path, item) in document["paths"].as_object().unwrap() {
            for method in ["get", "post", "put", "patch", "delete"] {
                let Some(operation) = item.get(method) else { continue };
                let id = operation["operationId"].as_str().unwrap_or_else(|| {
                    panic!("{method} {path} is missing operationId")
                });
                assert!(ids.insert(id), "duplicate operationId {id}");
                let responses = operation["responses"].as_object().unwrap_or_else(|| {
                    panic!("{method} {path} is missing responses")
                });
                assert!(responses.keys().any(|status| status.starts_with('2'))
                    || operation["x-neoism-transport"] == "websocket",
                    "{method} {path} is missing a successful response");
            }
        }
    }

    #[test]
    fn path_templates_and_parameters_are_complete() {
        let document = canonical_openapi();
        for (path, item) in document["paths"].as_object().unwrap() {
            for method in ["get", "post", "put", "patch", "delete"] {
                let Some(operation) = item.get(method) else { continue };
                let parameters = operation["parameters"].as_array().unwrap();
                for segment in path.split('/').filter(|segment| segment.starts_with('{')) {
                    let name = segment.trim_start_matches('{').trim_end_matches('}');
                    assert!(parameters.iter().any(|parameter| {
                        parameter["in"] == "path" && parameter["name"] == name
                            && parameter["required"] == true
                    }), "{method} {path} does not declare path parameter {name}");
                }
            }
        }
    }

    #[test]
    fn every_local_reference_resolves() {
        let document = canonical_openapi();
        visit_references(&document, &document);
    }

    fn visit_references(value: &Value, document: &Value) {
        match value {
            Value::Object(object) => {
                if let Some(reference) = object.get("$ref").and_then(Value::as_str) {
                    let pointer = reference.strip_prefix('#').unwrap_or_else(|| {
                        panic!("non-local reference {reference} is not canonical")
                    });
                    assert!(document.pointer(pointer).is_some(), "unresolved reference {reference}");
                }
                for child in object.values() { visit_references(child, document); }
            }
            Value::Array(array) => {
                for child in array { visit_references(child, document); }
            }
            _ => {}
        }
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
