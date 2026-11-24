# Workflow automation parity

This is an implementation note, not a description of released Neoism behavior. The reference is [OpenCode v2.4.0](https://opencode.ai/changelog/v2.4.0#automations).

## What OpenCode actually implements

| Surface | OpenCode v2.4.0 | Neoism direction |
| --- | --- | --- |
| Definitions | A persisted tagged trigger (`at`, `every`, `cron`, `event`) and action (`prompt`, `shell`, `code`). See [the schema](https://github.com/anomalyco/opencode/blob/v2.4.0/packages/schema/src/automation.ts). | Keep workspace Markdown and the existing durable projections. Extend the current scheduler rather than adding another scheduler or migrating existing definitions. |
| Management | Agent automation tools, an API, and `/automation`. | Reuse workflow API and chat tool; add native Rune controls on the same API rather than making the frontend own execution. |
| Shell | Creation authorizes the stored command and workspace; unattended execution records the result. See [the shell runner](https://github.com/anomalyco/opencode/blob/v2.4.0/packages/core/src/automation/shell.ts). | Keep approval separate from the editable definition. Bind it to the command, working directory, and definition revision so a file edit cannot silently substitute a different command. Record exit status and bounded output in run history. |
| Code | Bounded execution with no model request; statically identified capabilities are checked at creation. See [the Code Mode runner](https://github.com/anomalyco/opencode/blob/v2.4.0/packages/core/src/automation/code.ts). | Use the composition interpreter with an explicit tool set and the existing permission dispatcher. Re-check current denies and availability; do not grant a saved script access to all tools visible in a chat. |
| MCP events | Named push events with subscription argument validation and a reconnecting stream. Events can carry a resume position; prompt delivery labels the payload as untrusted data. See [the event adapter](https://github.com/anomalyco/opencode/blob/v2.4.0/packages/core/src/automation/event.ts) and [stream policy](https://github.com/anomalyco/opencode/blob/v2.4.0/packages/core/src/mcp/event-stream.ts). | Add an explicit subscription tied to an authenticated server connection and a durable occurrence identity. Do not reinterpret existing tool-list-change notifications as triggers. Bound event size and queue depth; keep payloads separate from executable text. |

## Integration checkpoints

1. **Author and inspect.** Definition validation, preview, and audit views should work before enabling an action. Lifecycle operations use the same backend whether invoked from chat, the API, or Rune.
2. **Dispatch actions.** Add an action-dispatch boundary after the scheduler claims a run. Preserve existing prompt behavior and the current retry and concurrency policies. Verify direct execution does not require a provider or model call and that output survives restart.
3. **Admit non-prompt actions.** Persist revision-scoped authorization separately from the source. Test that editing a command, changing its working directory, or removing an approved tool does not carry forward the old grant. Pausing must prevent future runs.
4. **Ingest events.** Validate subscriptions before activation. Test reconnection, duplicate delivery, revoked access, malformed and oversized payloads, and restart between receipt and dispatch. Never interpolate an external payload into a command or script.
5. **Ship the same contract across clients.** Update the OpenAPI/SDK contract, Rune controls, and the bundled handbook as features land. Keep unsupported options out of the published guide until they are implemented and verified.