import {
  capabilityEnabled,
  createContractClient,
  type CapabilityInfo,
  type NeoismClient,
  type PluginSdk,
  type StopSubagentsResult,
  type SubagentTask,
} from "@neoism/sdk-core";

export type { SubagentTask } from "@neoism/sdk-core";

export interface SubagentsClient {
  list(sessionId: string): Promise<SubagentTask[]>;
  stop(sessionId: string, taskId?: string): Promise<StopSubagentsResult>;
}

export const subagents: PluginSdk<SubagentsClient> = {
  id: "dev.neoism.subagents",
  capability: "neoism.subagents",
  supported(capabilities: readonly CapabilityInfo[]) {
    return capabilityEnabled(capabilities, this.capability);
  },
  client(core: NeoismClient): SubagentsClient {
    const operations = createContractClient(core.transport);
    return {
      list: (sessionId) => operations.request("v2.subagents.tasks.list", { path: { session_id: sessionId } }),
      stop: (sessionId, taskId) => operations.request("v2.subagents.tasks.stop", {
        path: { session_id: sessionId }, body: taskId ? { taskId } : {},
      }),
    };
  },
};