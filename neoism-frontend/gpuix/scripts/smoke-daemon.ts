import { Effect, ManagedRuntime, Schema } from "effect";
import { loadConfig, publicEndpoint } from "../src/config";
import { Daemon, DaemonConnection, daemonLayer } from "../src/daemon/client";
import { DirectorySchema, TreeSchema, expectVariant } from "../src/daemon/protocol";
import { AgentSdk } from "../src/daemon/sdk";

// Read-only: never spawn a PTY, submit a prompt, or alter the host's active workspace.
const config = loadConfig();
const connection = new DaemonConnection(config);
const runtime = ManagedRuntime.make(daemonLayer(connection));
const agent = new AgentSdk(config);
let unsubscribe = () => {};
try {
  await runtime.runPromise(Daemon);
  await new Promise<void>((resolve, reject) => {
    const timeout = setTimeout(() => reject(new Error("Daemon connection timed out")), 9000);
    unsubscribe = connection.subscribeStatus((state) => {
      if (state.phase === "connected") { clearTimeout(timeout); resolve(); }
      if (state.phase === "auth-rejected") { clearTimeout(timeout); reject(new Error(state.reason || "Authentication rejected")); }
    });
  });
  const tree = Schema.decodeUnknownSync(TreeSchema)(expectVariant(await Effect.runPromise(connection.request("Workspace", "RequestHostWorkspaceTree")), "HostWorkspaceTree"));
  const workspace = tree.workspaces.find((w) => w.id === config.workspaceId) || tree.workspaces.find((w) => w.root_dir);
  if (!workspace?.root_dir) throw new Error("No daemon workspace with a directory to inspect");
  const directory = Schema.decodeUnknownSync(DirectorySchema)(expectVariant(await Effect.runPromise(connection.request("Files", { ListDir: { path: "" } }, workspace.root_dir)), "DirListing"));
  const sdk = agent.forWorkspace(workspace.id);
  const [providers, agents] = await Promise.all([sdk.catalog.providers.configured(workspace.root_dir), sdk.catalog.agents.list(workspace.root_dir)]);
  console.log(`Connected: ${publicEndpoint(config.endpoint)}\nWorkspaces: ${tree.workspaces.length}\nFile listing: ${directory.entries.length} entries\nSDK catalogs: ${providers.providers.length} providers, ${agents.length} agents\nRead-only daemon and SDK smoke passed.`);
} finally { agent.dispose(); unsubscribe(); await runtime.dispose(); }
