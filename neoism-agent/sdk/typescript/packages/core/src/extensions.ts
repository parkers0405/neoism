import type { NeoismClient } from "./client.js";
import type { CapabilityInfo, EventEnvelope } from "./types.js";

export interface PluginSdk<TClient> {
  readonly id: string;
  readonly capability: string;
  supported(capabilities: readonly CapabilityInfo[]): boolean;
  client(core: NeoismClient): TClient;
}

export interface PluginUseOptions {
  directory?: string;
  minimumVersion?: string;
}

export class CapabilityUnavailableError extends Error {
  constructor(
    readonly capability: string,
    readonly pluginId: string,
    readonly minimumVersion?: string,
  ) {
    super(
      minimumVersion
        ? `Capability ${capability} >= ${minimumVersion} is unavailable`
        : `Capability ${capability} is unavailable`,
    );
    this.name = "CapabilityUnavailableError";
  }
}

export interface EventReducerExtension<State> {
  readonly pluginId: string;
  readonly eventTypes: readonly string[];
  reduce(state: State, event: EventEnvelope): State;
}

export function capabilityEnabled(
  capabilities: readonly CapabilityInfo[],
  id: string,
): boolean {
  return capabilities.some((capability) => capability.id === id && capability.enabled);
}

export function reduceEvent<State>(
  state: State,
  event: EventEnvelope,
  extensions: readonly EventReducerExtension<State>[],
): State {
  const extension = extensions.find((candidate) => candidate.eventTypes.includes(event.type));
  return extension?.reduce(state, event) ?? state;
}