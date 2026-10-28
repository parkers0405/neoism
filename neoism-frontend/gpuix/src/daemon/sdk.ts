import { createHttpClient, type NeoismClient, type ModelRef, type PromptRequest } from "@neoism/sdk";
import type { AppConfig } from "../config";
import type { Attachment, ProviderState } from "./protocol";

export function sdkModel(selection: ProviderState): ModelRef | undefined {
  const ref = selection.model;
  if (!ref) return undefined;
  const slash = ref.indexOf("/");
  if (slash <= 0 || slash === ref.length - 1) throw new Error("Select a provider/model");
  return { providerId: ref.slice(0, slash), id: ref.slice(slash + 1),
    ...(selection.connection_id ? { connectionId: selection.connection_id } : {}),
    ...(selection.thinking ? { variant: selection.thinking } : {}) };
}
export function sdkPrompt(text: string, attachments: readonly Attachment[], selection: ProviderState,
  delivery: "steer" | "queue"): PromptRequest {
  const model = sdkModel(selection);
  return { messageId: crypto.randomUUID(), agent: selection.agent || undefined, delivery,
    ...(model ? { model: { providerId: model.providerId, modelId: model.id, variant: model.variant, connectionId: model.connectionId } } : {}),
    parts: [{ type: "text", text }, ...attachments.filter((a) => a.bytes.length > 0).map((a) => ({
      type: "file" as const, filename: a.path || "image", mime: a.kind,
      url: `data:${a.kind};base64,${Buffer.from(a.bytes).toString("base64")}`,
    }))] };
}

/** Reach the Agent through the same daemon and workspace as the native UI. */
export function agentSdkEndpoint(endpoint: string, workspaceId: string) {
  const url = new URL(endpoint);
  let socket: string | undefined;
  if (url.protocol === "ws+unix:") {
    const separator = url.pathname.lastIndexOf(":/");
    if (separator < 1) throw new Error("Invalid daemon Unix socket endpoint");
    socket = decodeURIComponent(url.pathname.slice(0, separator));
    url.pathname = url.pathname.slice(separator + 1);
  } else if (!["ws:", "wss:"].includes(url.protocol)) throw new Error("Unsupported daemon endpoint");
  const base = socket ? new URL(`http://localhost${url.pathname}`) : new URL(url);
  base.protocol = url.protocol === "wss:" ? "https:" : "http:";
  base.pathname = base.pathname.replace(/\/session\/?$/, "").replace(/\/$/, "")
    + `/agent/workspaces/${encodeURIComponent(workspaceId)}/`;
  base.search = ""; base.hash = "";
  return { baseUrl: base.toString(), socket, token: url.searchParams.get("token") || undefined };
}

export class AgentSdk {
  private clients = new Map<string, NeoismClient>();
  private abort = new AbortController();
  constructor(private config: Pick<AppConfig, "endpoint" | "authToken" | "pairingToken">,
    private fetcher: typeof fetch = globalThis.fetch) {}
  forWorkspace(workspaceId: string): NeoismClient {
    let client = this.clients.get(workspaceId);
    if (client) return client;
    const endpoint = agentSdkEndpoint(this.config.endpoint, workspaceId);
    const fetcher = this.fetcher, signal = this.abort.signal;
    const request = (async (input: Parameters<typeof fetch>[0], init?: RequestInit) => fetcher(input, {
      ...init, signal: AbortSignal.any([signal, ...(init?.signal ? [init.signal] : [])]),
      ...(endpoint.socket ? { unix: endpoint.socket } : {}),
    })) as typeof fetch;
    client = createHttpClient({ baseUrl: endpoint.baseUrl,
      token: this.config.pairingToken || this.config.authToken || endpoint.token, fetch: request });
    this.clients.set(workspaceId, client);
    return client;
  }
  dispose() { this.abort.abort(); this.clients.clear(); }
}
