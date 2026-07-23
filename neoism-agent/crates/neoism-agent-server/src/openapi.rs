use axum::Json;
use serde_json::{json, Value};

pub(crate) async fn openapi_doc() -> Json<Value> {
    Json(json!({
        "openapi": "3.1.1",
        "info": { "title": "neoism", "version": env!("CARGO_PKG_VERSION"), "description": "neoism headless agent api" },
        "paths": {
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
            "/mcp/{name}/auth/callback": { "post": { "operationId": "mcp.auth.callback" } },
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
                        "delivery": { "type": "string", "enum": ["sync", "async"] },
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
    }))
}
