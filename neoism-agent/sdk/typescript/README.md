# Neoism TypeScript SDK

Frontend-neutral clients for the Neoism Agent `/v2` API.

```ts
import { createHttpClient } from "@neoism/sdk-http";
import { subagents } from "@neoism/sdk-plugin-subagents";

const client = createHttpClient({
  baseUrl: "http://127.0.0.1:4096",
  token: process.env.NEOISM_AGENT_TOKEN,
});
const capabilities = await client.capabilities.list();

if (subagents.supported(capabilities)) {
  const tasks = await subagents.client(client).list("session-id");
}

// Any plugin can be consumed without adding it to the SDK core.
const vcs = await client.plugins.request<unknown>("dev.neoism.vcs", "status", {
  query: { directory: "/workspace" },
});

for await (const event of client.transport.events()) {
  console.log(event.sequence, event.type, event.data);
}
```

Unknown events and plugin payloads remain `unknown` and are preserved. The HTTP
transport resumes with `Last-Event-ID`, deduplicates by durable sequence, and
reconnects with bounded exponential backoff.

Loopback servers may run in trusted mode without a token. Non-loopback
`neoism-agent serve` requires `NEOISM_AGENT_TOKEN` or
`NEOISM_AGENT_AUTH_CONFIG`; callers send a configured token as a Bearer token.
`NEOISM_AGENT_ALLOW_UNAUTHENTICATED_REMOTE=1` is an explicit unsafe
escape hatch for deployments that provide authentication in an upstream
gateway.

Hosted deployments can replace the single token with `NEOISM_AGENT_AUTH_CONFIG`:

```json
{
  "tokens": [{
    "token": "secret",
    "tenantId": "team-a",
    "directoryPrefixes": ["/srv/workspaces/team-a"],
    "requestsPerMinute": 600,
    "maxInFlight": 20,
    "maxSessions": 100,
    "maxArtifacts": 1000,
    "maxArtifactBytes": 26214400,
    "artifactRetentionDays": 30
  }]
}
```

Hosted claims scope sessions, event streams, interactions, artifacts, audit
entries, directories, and quotas. Global configuration and provider credential
mutation routes are denied in hosted mode until they have tenant-owned secret
storage.

Set `NEOISM_AGENT_ARTIFACT_SCAN_COMMAND` to an executable that accepts the
temporary upload path and exits successfully only for accepted content. Scanner
execution is limited to 60 seconds and rejected uploads are deleted.

## Contract generation

`neoism-agent/openapi/v2.sha256` fingerprints the deterministic canonical document. A
dependency-free generator produces
`packages/core/src/generated/contract.ts` from its schemas and operations.

```sh
cargo run -p neoism-agent -- openapi
# or, from this directory
npm run contract:print
npm run contract:check
```

Use `npm run contract:update` after an intentional API change. The Rust parity
test checks every router method and path in both directions; `contract:check`
then performs byte-for-byte drift checks on the OpenAPI snapshot and generated
TypeScript types.

## Process plugins

External hook plugins can declare `command` as an executable string or argument
array in their JSON manifest. Neoism invokes the command with one JSON request
on stdin and expects one JSON response on stdout. The protocol is
`neoism-plugin/1`; invocations have configurable bounded timeouts and a 4 MiB
response limit. This subprocess boundary avoids exposing an unstable Rust ABI.
On Linux, Neoism automatically uses Bubblewrap when available. Hosted mode
requires it: plugin files and system runtimes are read-only, `/tmp` is ephemeral,
and network access is disabled unless the plugin manifest sets `network: true`.
Set `sandbox: true` to require isolation locally or
`NEOISM_AGENT_PLUGIN_SANDBOX=off` to explicitly disable automatic local use.
Hook failures and timeouts mark plugin status unhealthy; a later successful
invocation restores health.