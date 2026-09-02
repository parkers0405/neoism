# Neoism Agent Troubleshooting

Start with the smallest failing boundary: agent process, provider connection, model, session, tool, MCP server, or language server.

## Agent pane does not start

1. Open the command palette and try **New Agent** again.
2. Check whether the workspace daemon is connected.
3. Inspect Neoism logs for `neoism-agent-server` startup errors.
4. Validate global `config.json` and workspace `.neoism/config.json` JSONC.
5. Remove or correct a conflicting provider allowlist/denylist.

Neoism supervises the Agent server. A failed child should be reported and restarted rather than leaving a permanently silent pane.

## No providers or models

- Open the provider/model picker and complete a connection.
- Confirm the API-key environment variable exists in the Agent server process.
- Check `enabledProviders` and `disabledProviders`.
- Confirm Models.dev is reachable or a local catalog cache/path exists.
- Verify the configured ID uses `provider/model` and still exists.
- Subscription connections may expose only a subset of public models.

See [[Providers]] and [[Models]].

## Authentication fails

- Reconnect through the picker to replace stored credentials.
- For OAuth, complete the same flow before its state expires.
- For Copilot device flow, finish verification in the browser and wait for polling.
- Ensure a custom base URL matches the expected provider protocol.
- Do not mix an expired stored credential with a new environment key without reconnecting/removing the stored credential.

## Session looks empty or duplicated

A client first loads a snapshot and then follows live events. Reconnect to force a fresh snapshot if the stream was interrupted. Confirm the pane points to the expected session ID and workspace host.

Persisted workspace layouts can outlive a lost daemon PTY or session record. In that case create/attach a current session rather than repeatedly reopening a stale ID.

## Agent appears stuck

- Press `Esc` to interrupt the active response.
- Check `/status` and `/subagents`.
- Inspect visible permission or question cards waiting for input.
- Check background tasks with `background_task_result` or use **Stop**.
- A provider may be streaming no visible text while reasoning or waiting on a tool.
- Restarting the agent process discards live in-memory background-task inventory.

## Permission repeats after approval

The approved pattern may not cover every target in a compound tool call. Shell commands can generate several patterns, and external paths require `external_directory`. Add a precise configured rule when the operation is routinely safe.

Remember that the last matching wildcard rule wins.

## MCP server fails

- Run `/mcp` and inspect `failed`, `needs_auth`, or `needs_client_registration`.
- For local servers, run the configured executable directly to inspect stderr.
- Verify environment substitution and command arguments.
- For remote servers, verify URL, TLS, headers, OAuth callback, and network reachability.
- Increase `timeout` only after identifying a genuinely slow startup.

`MCP server <name> does not support OAuth` describes the effective Neoism configuration, not necessarily the remote service. It means the entry is local, omits `oauth`, or has `"oauth": false`. For an OAuth-capable remote server, add `"oauth": true` or `"oauth": {}` to the effective entry, reopen `/mcp`, and choose **Connect**. Restart Neoism if an old process still has the previous catalog.

For CLI diagnosis, run `neoism mcp auth list`. Use `neoism mcp auth <name> --no-open` to print the authorization URL when automatic browser launching fails. See [[MCP Servers]] for complete examples.

## LSP tool fails

- Check LSP status.
- Confirm the command is installed and visible to the Agent process.
- Confirm extension matching and project root.
- Use zero-based UTF-8 byte positions.
- Review language-server logs/diagnostics.

## Compaction fails

Confirm a usable model remains connected. Interrupting compaction does not delete old messages. Retry after provider recovery, or start a new session with a manual handoff summary.

## Configuration paths

Neoism's main configuration path is shown in [[Getting Started/06 Configure Neoism]]. Workspace overrides live in `.neoism/config.json`; an existing root `neoism.json` is moved there automatically.

## Report a useful failure

Include:

- Neoism version and platform.
- Provider ID and model ID, but never the credential.
- Agent name and whether it was a child session.
- Relevant config with secrets removed.
- Exact error and timestamp.
- Whether desktop, browser, or mobile displayed it.
- The smallest reproducible prompt/tool action.

Do not post `auth.json`, OAuth tokens, API keys, private attachments, or full proprietary tool output.