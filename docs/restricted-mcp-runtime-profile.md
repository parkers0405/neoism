# Restricted MCP runtime profile

`restricted-mcp-v1` is an immutable hosting profile for a trusted API controller whose only model-visible tools are a fixed remote MCP gateway. It is selected once, before the HTTP socket is bound:

```sh
NEOISM_RUNTIME_PROFILE=restricted-mcp-v1 \
NEOISM_RESTRICTED_CONFIG_ROOT=/etc/neoism/restricted-worker \
neoism-agent serve --hostname 127.0.0.1 --port 4096
```

`NEOISM_RESTRICTED_CONFIG_ROOT` must be an existing directory containing `agent.json`. Startup fails for an unknown profile, a missing/invalid root file, unknown root keys, local MCP entries, disabled MCP entries, custom plugins, or missing fixed agents/models/providers/remote MCP servers. The validated document is snapshotted at boot and exposed to every workspace as a single read-only config layer; later file or API changes do not affect the process.

Allowed root keys are `$schema`, `provider`, `enabledProviders`, `disabledProviders`, `model`, `variant`, `defaultAgent`, `agent`, and `mcp`. Every configured agent must resolve to a `provider/model`; only configured agent and model references are accepted on session or prompt requests. Per-invocation `system`, text `parts`, and `messageId` remain supported. Prompt tool overrides, non-text delegation/file parts, session permission overrides, provider/MCP/config mutations, and local MCP are rejected.

The profile registers only config plumbing, system prompt composition, read-only agents, providers, and MCP. It does not discover filesystem plugins/config, register filesystem/shell/web/browser/computer/memory/artifact/skill/command/workflow/goal/VCS/custom/delegation/question tools, or install the built-in computer MCP. A central HTTP gate permits only read-only metadata, agents/providers, scoped events, the required session API, and catalog/tool execution for allowlisted remote MCP names. The session API includes narrowly scoped `DELETE /v2/sessions/:id/queue` so trusted cancellation can clear queued follow-ups before `POST /abort`; queue pop and all other queue mutations remain forbidden, and normal session validation/authentication still applies. A second tool-runtime gate admits only `execute` and `mcp__<configured-server>__*`; MCP connection also rechecks that the boot entry is remote.

## Attestation

`GET /v2/capabilities` includes this non-disableable entry only in the restricted process:

```json
{
  "id": "runtime.restricted-mcp-v1",
  "version": "1",
  "enabled": true,
  "disableable": false,
  "source": "runtime",
  "reason": "immutable;providers=<ids>;mcpServers=<names>;agents=<names>"
}
```

The reason contains identifiers only, never URLs, headers, credentials, prompts, or permission values. A controller should require the exact capability and its independently pinned executable/image digest; plugin manifests are not accepted as profile attestation.

## Limitations

This profile constrains Neoism's process capabilities, configuration, HTTP surface, and model tool dispatch. It does not sandbox the configured remote MCP service, the selected model provider, the host network, or the operating-system account running Neoism. Those endpoints and process-level egress/storage controls remain deployment responsibilities.

## Native verification

The server test `restricted_profile_real_http_model_mcp_resume_and_denial_e2e` serves the native Axum router on a real TCP listener with a local OpenAI-compatible fake model and a separate remote-transport MCP mock. It verifies an ordinary configured `synapse/fake` turn without an MCP call, `execute` gateway search/call against only `synapse`, HTTP denial of shell/commands/delegation/files/tool-model-agent-variant overrides/config/provider-auth/plugin/local-MCP/legacy prompt routes, session-scoped SSE, immutable config after on-disk replacement, and a resumed second turn.

`restricted_profile_real_http_abort_settles_without_late_model_output` holds a local fake model stream open until explicitly released, queues a follow-up, executes the production cancellation sequence (`DELETE` queue then `POST` abort) through real HTTP, and verifies canonical interrupted message closure, an idle runtime family, an empty/non-running durable queue, bounded `/wait`, and no provider output committed after cancellation. The same test proves nonexistent-session queue clear still reaches session validation while queue pop, prompt tool elevation, and config mutation remain forbidden. `restricted_profile_restart_preserves_safe_history_and_rejects_stored_elevation` reopens the same persisted database under a newly constructed restricted process state, verifies attestation and safe history/profile continuity through a resumed turn, rejects a persisted permission elevation, and then removes a formerly allowed agent from the boot profile and proves its stored session can no longer run. All native verification uses loopback-only fake services and performs no external or paid calls.