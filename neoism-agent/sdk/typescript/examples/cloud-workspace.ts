import {
  CloudRuntimeClient, HostWorkspaceClient, connectWorkspaceWorker,
  type Allocation, type HostWorkspaceClientOptions,
} from "@neoism/sdk";

/** Infrastructure-only; keep this bridge credential on your trusted host. */
export async function ensureCloudWorkspace(input: {
  bridgeEndpoint: string;
  hostBearer: string;
  allocation: Allocation;
}) {
  const cloud = new CloudRuntimeClient({
    endpoint: input.bridgeEndpoint,
    bearer: input.hostBearer,
    provider: input.allocation.provider,
    capabilities: {
      isolation: "virtual_machine", stop_start: true,
      cpu_limit: true, memory_limit: true, disk_limit: true, durable_workspace: true,
    },
  });
  return cloud.ensure(input.allocation);
}

/** Application-facing: your app owns accounts, authorization and billing.
 * Its host policy chooses tenant, resource spec and worker scopes, not this request.
 * This callback is external gateway auth, never cloud infrastructure credentials.
 */
export async function connectApplicationWorkspace(input: {
  gatewayEndpoint: string;
  workspace: string;
  appToken: HostWorkspaceClientOptions["token"];
  trustedWorkerOrigins?: readonly string[];
  signal?: AbortSignal;
}) {
  const host = new HostWorkspaceClient({
    endpoint: input.gatewayEndpoint,
    token: input.appToken,
    ...(input.trustedWorkerOrigins ? { trustedWorkerOrigins: input.trustedWorkerOrigins } : {}),
  });
  const worker = await connectWorkspaceWorker({
    host, workspace: input.workspace,
    ...(input.signal ? { signal: input.signal } : {}),
  });
  const runtime = await worker.runtime.get();
  return { worker, runtime, capabilities: await worker.capabilities.list() };
}
