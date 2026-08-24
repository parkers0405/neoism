import {
  capabilityEnabled,
  type CapabilityInfo,
  type NeoismClient,
  type PluginSdk,
} from "@neoism/sdk-core";

export interface SubagentTask {
  id: string;
  sessionId: string;
  childSessionId?: string;
  agent: string;
  status: "queued" | "running" | "pending" | "completed" | "error" | "stopped";
  description?: string;
  result?: string;
}

export interface SubagentsClient {
  list(sessionId: string): Promise<SubagentTask[]>;
  stop(sessionId: string, taskId?: string): Promise<void>;
}

export const subagents: PluginSdk<SubagentsClient> = {
  id: "dev.neoism.subagents",
  capability: "neoism.subagents",
  supported(capabilities: readonly CapabilityInfo[]) {
    return capabilityEnabled(capabilities, this.capability);
  },
  client(core: NeoismClient): SubagentsClient {
    const prefix = "/v2/plugins/dev.neoism.subagents";
    return {
      list: (sessionId) => core.transport.request<SubagentTask[]>({
        path: `${prefix}/sessions/${encodeURIComponent(sessionId)}/tasks`,
      }),
      async stop(sessionId, taskId) {
        await core.transport.request<void>({
          method: "POST",
          path: `${prefix}/sessions/${encodeURIComponent(sessionId)}/stop`,
          body: taskId ? { taskId } : {},
        });
      },
    };
  },
};