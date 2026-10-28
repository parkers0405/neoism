import { createHttpClient, type NeoismClient, type ModelRef, type PromptRequest } from "@neoism/sdk";
import type { AppConfig } from "../config";
import type { Attachment, ProviderState,Thread } from "./protocol";

/** DTO projection only: concrete SDK ModelRef keeps the provider-qualified
 * identity used by the canonical Agent wire history/provider state. */
export function sdkThread(session:{id:string;title:string;directory:string;agent?:string;model?:{providerId:string;id:string}}):Thread{
  return{session_id:session.id,title:session.title,directory:session.directory,agent:session.agent,model:session.model?`${session.model.providerId}/${session.model.id}`:undefined};
}

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
  private masterCatalog?:Promise<ReturnType<typeof agentSdkEndpoint>|undefined>;
  private catalogReaders=new Map<string,NeoismClient>();
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
  /** The catalog's initial family/activity baseline can exceed the daemon
   * WebSocket reply queue. Read its SSE endpoint directly through the SDK's
   * existing authenticated parser/reconnect loop, with HTTP backpressure. */
  private async catalogEndpoint(workspaceId:string){
    const raw=agentSdkEndpoint(this.config.endpoint,workspaceId),scoped={...raw,token:this.config.pairingToken||this.config.authToken||raw.token};
    // Existing desktop catalogs include legacy sessions outside a workspace's
    // tenant. Only a daemon-confirmed master catalog capability may read them.
    // A paired client always keeps its authorized workspace scope.
    if(!this.config.authToken||this.config.pairingToken)return scoped;
    if(!this.masterCatalog)this.masterCatalog=(async()=>{
      const endpoint={...scoped,baseUrl:scoped.baseUrl.replace(/\/agent\/workspaces\/[^/]+\/$/,"/agent/system/"),token:this.config.authToken};
      const fetcher=this.fetcher,signal=this.abort.signal;
      const request=((input:Parameters<typeof fetch>[0],init?:RequestInit)=>fetcher(input,{...init,signal:AbortSignal.any([signal,...(init?.signal?[init.signal]:[])]),...(endpoint.socket?{unix:endpoint.socket}:{})}))as typeof fetch;
      const client=createHttpClient({baseUrl:endpoint.baseUrl,token:endpoint.token,fetch:request});
      try{
        const info=await client.transport.request<{systemId?:unknown;capabilityVersion?:unknown}>({path:"info"});
        return typeof info.systemId==="string"&&typeof info.capabilityVersion==="number"?endpoint:undefined;
      }catch{return undefined;}
    })();
    return await this.masterCatalog||scoped;
  }
  private async catalogReader(workspaceId:string){
    const endpoint=await this.catalogEndpoint(workspaceId);let client=this.catalogReaders.get(endpoint.baseUrl);
    if(!client){
      const fetcher=this.fetcher,signal=this.abort.signal;
      const request=((input:Parameters<typeof fetch>[0],init?:RequestInit)=>fetcher(input,{...init,signal:AbortSignal.any([signal,...(init?.signal?[init.signal]:[])]),...(endpoint.socket?{unix:endpoint.socket}:{})}))as typeof fetch;
      client=createHttpClient({baseUrl:endpoint.baseUrl,token:endpoint.token||this.config.pairingToken||this.config.authToken,fetch:request});this.catalogReaders.set(endpoint.baseUrl,client);
    }
    return client;
  }
  async catalogProjects(workspaceId:string){return(await this.catalogReader(workspaceId)).operations.request("v2.sessions.projects",{});}
  async catalogSessions(workspaceId:string,options:Parameters<NeoismClient["sessions"]["list"]>[0]){return(await this.catalogReader(workspaceId)).sessions.list(options);}
  async *catalogEvents(workspaceId:string,options:{directory?:string;signal?:AbortSignal;onReset?:()=>void}={}):ReturnType<NeoismClient["events"]["subscribe"]>{
    const endpoint=await this.catalogEndpoint(workspaceId);
    const signal=AbortSignal.any([this.abort.signal,...(options.signal?[options.signal]:[])]);
    const fetcher=this.fetcher;
    const request=(async(input:Parameters<typeof fetch>[0],init?:RequestInit)=>{
      const url=new URL(input instanceof Request?input.url:String(input));
      // This dedicated SDK client is used only for events. Retain the exact
      // workspace proxy prefix; authorization never changes origin or scope.
      if(!url.pathname.endsWith("/v2/events"))throw new Error("Invalid catalog stream request");
      url.pathname=url.pathname.slice(0,-"/v2/events".length)+"/v2/session-catalog/events";
      url.search="";
      if(options.directory)url.searchParams.set("directory",options.directory);
      else url.searchParams.set("scope","all");
      const response=await fetcher(url.toString(),{...init,signal:AbortSignal.any([signal,...(init?.signal?[init.signal]:[])]),...(endpoint.socket?{unix:endpoint.socket}:{})});
      if(response.ok&&!response.redirected)options.onReset?.();
      return response;
    })as typeof fetch;
    yield*createHttpClient({baseUrl:endpoint.baseUrl,token:endpoint.token||this.config.pairingToken||this.config.authToken,fetch:request}).events.subscribe({signal});
  }
  dispose() { this.abort.abort(); this.clients.clear();this.catalogReaders.clear(); }
}
