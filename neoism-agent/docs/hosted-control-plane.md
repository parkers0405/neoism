# Hosted control plane embedding

Neoism Agent can run as a shared tenant-aware control plane while standalone Neoism remains local and native. A hosted process must inject identity, execution, credential, and artifact services and explicitly enable strict hosted validation. Tenant identity must come from the resolver, never from model input, directories, request bodies, or sandbox identifiers.

## Host construction

Create a small host binary that depends on `neoism-agent-server` and `neoism-agent-service-api` at the same version. The concrete adapters may verify a host-signed JWT locally or call private host services.

```rust,no_run
use std::sync::Arc;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let services = neoism_agent_server::services_with_workspace_search(workspace_search())
        .with_tenant_resolver(Arc::new(SynapseTenantResolver::new()))
        .with_execution(Arc::new(SynapseDaytonaProvider::new()))
        .with_artifacts(Arc::new(SynapseArtifactStore::new()))
        .with_provider_credentials(Arc::new(SynapseProviderCredentials::new()))
        .with_mcp_credentials(Arc::new(SynapseMcpCredentials::new()))
        .for_hosted_control_plane();

    neoism_agent_server::listen(
        neoism_agent_server::ServerOptions {
            hostname: "0.0.0.0".into(),
            port: 4096,
            cors: Vec::new(),
        },
        services,
    )
    .await?;
    Ok(())
}
```

`for_hosted_control_plane()` makes startup fail unless the host supplies a tenant resolver, tenant-scoped provider credentials, tenant-scoped MCP credentials, a non-native execution provider, and a shared artifact store. Do not set `NEOISM_AGENT_ALLOW_UNAUTHENTICATED_REMOTE=1` in production.

## Identity mapping

| Agent field | Host value |
|---|---|
| `tenant_id` | Company or organization ID |
| `subject` | Human or service-account ID |
| `actor_type` | `Human` or `ServiceAccount` |
| `workspace_id` | Optional project/workspace association |
| `root_id` | Stable project/worktree identity |
| `session_id` | Durable conversation session |
| `execution_id` | Turn or process-operation identity |

The host remains authoritative for membership, SSO, service-account lifecycle, billing, entitlements, token issuance, and project authorization. Neoism owns session execution state, canonical messages, control leases, participant activity, tool policy, and tenant-qualified artifacts.

## TypeScript client

Create one client per short-lived host credential or otherwise rotate the transport before token expiry.

```ts
import { createHttpClient } from "@neoism/sdk";

const neoism = createHttpClient({
  baseUrl: process.env.NEOISM_AGENT_URL!,
  token: internalNeoismToken,
});

const session = await neoism.sessions.create({ title: "Team conversation" });
const lease = await neoism.sessions.claimControl(session.id, {
  expectedRevision: 0,
  leaseSeconds: 60,
});

await neoism.sessions.prompt(session.id, {
  messageId: stablePromptId,
  prompt: userPrompt,
});

for await (const event of neoism.events.subscribe({ sessionId: session.id })) {
  // Reconcile durable state from event sequence and canonical session APIs.
}

await neoism.sessions.releaseControl(session.id, lease.revision);
```

A service account and a human operate the same tenant-owned session. A takeover claims the control lease with the latest revision; it does not clone or transfer ownership of the session.

## Execution and artifacts

Hosted model and remote MCP operations allocate no sandbox. The first `sandbox_exec` operation acquires an `ExecutionLease` using trusted tenant, subject, root, session, execution, provider, quota, network, and lifetime fields. Native execution is never a hosted fallback. The provider materializes the expected workspace revision and returns a changed-path manifest and new revision; Neoism commits that revision with tenant/root compare-and-swap semantics.

Artifact metadata remains in Neoism. `ArtifactBlobStore` stores payload bytes in shared object storage so every server replica can read them. Object keys should include a tenant prefix, while authorization must still use the trusted tenant passed to the adapter.

## Unsupported hosted capabilities

Hosted workflow routes remain fail-closed until workflow definitions, activations, runs, recovery, and scheduling are tenant-owned. Native PTY, stdio MCP, LSP, process plugins, formatters, VCS processes, and external ACP processes remain local-only unless a future execution-provider contract explicitly brokers them. Capability discovery reports these boundaries.