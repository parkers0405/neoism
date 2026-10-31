import { afterEach, expect, test } from "bun:test";
import { mkdtempSync, rmSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { AgentSdk, agentSdkEndpoint, sdkModel, sdkPrompt } from "../src/daemon/sdk";
const cleanups: Array<() => void> = [];
afterEach(() => cleanups.splice(0).reverse().forEach((close) => close()));

test("SDK endpoints preserve the daemon origin and proxy prefix with an explicit workspace", () => {
  expect(agentSdkEndpoint("wss://host.example/neoism/session?token=secret", "space/a")).toEqual({
    baseUrl: "https://host.example/neoism/agent/workspaces/space%2Fa/", socket: undefined, token: "secret",
  });
  expect(agentSdkEndpoint("ws+unix:///run/user/1000/neoism.sock:/session", "a")).toEqual({
    baseUrl: "http://localhost/agent/workspaces/a/", socket: "/run/user/1000/neoism.sock", token: undefined,
  });
  expect(() => agentSdkEndpoint("ws+unix:///missing-separator", "a")).toThrow();
});

test("the real SDK sends prompts through the scoped local proxy with header authentication and exact attachment bytes", async () => {
  const directory = mkdtempSync(join(tmpdir(), "neoism-sdk-"));
  cleanups.push(() => rmSync(directory, { recursive: true, force: true }));
  const socket = join(directory, "daemon.sock");
  let received: { path: string; authorization: string | null; body: any } | undefined;
  const server = Bun.serve({ ...(process.platform==="win32"?{hostname:"127.0.0.1",port:0}:{unix:socket}), fetch: async (request) => {
    received = { path: new URL(request.url).pathname, authorization: request.headers.get("authorization"), body: await request.json() };
    return new Response(null, { status: 204 });
  } });
  cleanups.push(() => server.stop(true));
  const sdk = new AgentSdk({ endpoint: process.platform==="win32"?`${server.url.origin.replace(/^http/,"ws")}/session`:`ws+unix://${socket}:/session`, authToken: "operator", pairingToken: "device" });
  cleanups.push(() => sdk.dispose());
  const selection = { agent: "plan", model: "openai/model/variant", thinking: "high" };
  await sdk.forWorkspace("a").sessions.prompt("session/one", sdkPrompt("Review", [{ kind: "image/png", path: "capture.png", bytes: [0, 127, 128, 255] }], selection, "queue"));
  expect(received?.path).toBe("/agent/workspaces/a/v2/sessions/session%2Fone/prompt");
  expect(received?.authorization).toBe("Bearer device");
  expect(received?.body).toMatchObject({ agent: "plan", delivery: "queue", model: { providerId: "openai", modelId: "model/variant", variant: "high" },
    parts: [{ type: "text", text: "Review" }, { type: "file", filename: "capture.png", mime: "image/png", url: "data:image/png;base64,AH+A/w==" }] });
  expect(sdk.forWorkspace("a")).toBe(sdk.forWorkspace("a"));
  expect(sdk.forWorkspace("b")).not.toBe(sdk.forWorkspace("a"));
  expect(sdkModel(selection)).toEqual({ providerId: "openai", id: "model/variant", variant: "high" });
});

test("SDK mutations are never replayed on transport failure and disposal aborts pending requests", async () => {
  let attempts = 0, aborted = false;
  const fetcher = (async (_input: unknown, init?: RequestInit) => {
    attempts++;
    return new Promise<Response>((_resolve, reject) => init?.signal?.addEventListener("abort", () => { aborted = true; reject(new Error("aborted")); }, { once: true }));
  }) as typeof fetch;
  const sdk = new AgentSdk({ endpoint: "ws://127.0.0.1:7878/session" }, fetcher);
  const request = sdk.forWorkspace("a").sessions.abort("one");
  await Bun.sleep(5); sdk.dispose();
  await expect(request).rejects.toThrow();
  expect(aborted).toBe(true); expect(attempts).toBe(1);
});

test("catalog SSE uses the scoped authenticated local SDK stream and drains a baseline larger than the WebSocket reply budget",async()=>{
 const directory=mkdtempSync(join(tmpdir(),"neoism-sdk-catalog-")),socket=join(directory,"daemon.sock");
 cleanups.push(()=>rmSync(directory,{recursive:true,force:true}));
 const requests:Array<{path:string;scope:string|null;directory:string|null;authorization:string|null}>=[];
 const events=Array.from({length:825},(_,index)=>({id:`catalog-${index}`,sequence:index+1,schemaVersion:"2",source:"fixture",timestamp:index,type:"session.catalog.family",data:{sessionID:`root-${index}`,snapshot:{branches:[],familyRevision:1}}}));
 const server=Bun.serve({...(process.platform==="win32"?{hostname:"127.0.0.1",port:0}:{unix:socket}),fetch(request){
  const url=new URL(request.url);requests.push({path:url.pathname,scope:url.searchParams.get("scope"),directory:url.searchParams.get("directory"),authorization:request.headers.get("authorization")});
  return new Response(new ReadableStream({start(controller){controller.enqueue(new TextEncoder().encode(events.map(event=>`data: ${JSON.stringify(event)}\r\n\r\n`).join("")));}}),{headers:{"content-type":"text/event-stream"}});
 }});cleanups.push(()=>server.stop(true));
 const sdk=new AgentSdk({endpoint:process.platform==="win32"?`${server.url.origin.replace(/^http/,"ws")}/session`:`ws+unix://${socket}:/session`,authToken:"operator",pairingToken:"device"});cleanups.push(()=>sdk.dispose());
 let resets=0;const ids:string[]=[];
 for await(const event of sdk.catalogEvents("space/a",{onReset:()=>resets++})){ids.push(event.id);if(ids.length===825)break;}
 expect(resets).toBe(1);
 expect(ids).toEqual(events.map(event=>event.id));
 expect(requests[0]).toEqual({path:"/agent/workspaces/space%2Fa/v2/session-catalog/events",scope:"all",directory:null,authorization:"Bearer device"});
 const stop=new AbortController();
 for await(const _event of sdk.catalogEvents("b",{directory:"/project with spaces",signal:stop.signal})){stop.abort();break;}
 expect(requests[1]).toEqual({path:"/agent/workspaces/b/v2/session-catalog/events",scope:null,directory:"/project with spaces",authorization:"Bearer device"});
});

test("legacy catalog reads use a daemon-granted capability while paired reads and explicit workspace clients stay scoped",async()=>{
 const requests:Array<{path:string;authorization:string|null;method:string}>=[];
 const fetcher=(async(input:Parameters<typeof fetch>[0],init?:RequestInit)=>{
  const url=new URL(String(input)),authorization=new Headers(init?.headers).get("authorization");requests.push({path:url.pathname,authorization,method:init?.method||"GET"});
  if(url.pathname==="/agent/system/info")return authorization==="Bearer master"?Response.json({systemId:"fixture",capabilityVersion:1}):Response.json({code:"denied",message:"Denied",retryable:false,details:{}},{status:403});
  if(url.pathname.endsWith("/v2/session-projects"))return Response.json({items:[{directory:"/legacy",updatedMs:1}],pinnedSessions:[]});
  if(url.pathname.endsWith("/v2/sessions"))return Response.json({items:[],cursor:{next:null}});
  return new Response(null,{status:204});
 })as typeof fetch;
 const sdk=new AgentSdk({endpoint:"ws://localhost:7878/session",authToken:"master"},fetcher);cleanups.push(()=>sdk.dispose());
 await Promise.all([sdk.catalogProjects("a"),sdk.catalogSessions("b",{directory:"/legacy",roots:true,limit:5})]);
 expect(requests.filter(request=>request.path==="/agent/system/info")).toHaveLength(1);
 expect(requests.some(request=>request.path==="/agent/system/v2/session-projects")).toBe(true);
 expect(requests.some(request=>request.path==="/agent/system/v2/sessions")).toBe(true);
 await sdk.forWorkspace("a").sessions.prompt("one",sdkPrompt("hello",[],{model:"openai/base"},"steer"));
 expect(requests.at(-1)?.path).toBe("/agent/workspaces/a/v2/sessions/one/prompt");
 const paired=new AgentSdk({endpoint:"ws://localhost:7878/session?token=legacy",authToken:"master",pairingToken:"device"},fetcher);cleanups.push(()=>paired.dispose());
 const start=requests.length;await paired.catalogProjects("a");
 expect(requests.slice(start)).toEqual([{path:"/agent/workspaces/a/v2/session-projects",authorization:"Bearer device",method:"GET"}]);
 const denied=new AgentSdk({endpoint:"ws://localhost:7878/session",authToken:"not-master"},fetcher);cleanups.push(()=>denied.dispose());
 await denied.catalogProjects("b");expect(requests.at(-1)?.path).toBe("/agent/workspaces/b/v2/session-projects");
});

test("reopened conversations send once through their catalog capability while creation and provider settings stay workspace scoped",async()=>{
 const requests:Array<{path:string;method:string;body?:any}>=[];
 const fetcher=(async(input:Parameters<typeof fetch>[0],init?:RequestInit)=>{
  const path=new URL(String(input)).pathname,method=init?.method||"GET";
  requests.push({path,method,body:typeof init?.body==="string"?JSON.parse(init.body):undefined});
  if(path==="/agent/system/info")return Response.json({systemId:"fixture",capabilityVersion:1});
  if(path.includes("/agent/workspaces/current/v2/sessions/old-session"))return Response.json({code:"not_found",message:"Session not found",retryable:false},{status:404});
  if(path.endsWith("/prompt"))return new Response(null,{status:204});
  if(path.endsWith("/v2/sessions"))return Response.json({id:"new-session",title:"New",directory:"/current",time:{created:1,updated:1}});
  return Response.json({id:"old-session",title:"Old conversation",directory:"/old",time:{created:1,updated:1}});
 })as typeof fetch;
 const sdk=new AgentSdk({endpoint:"ws://localhost:7878/session",authToken:"master"},fetcher);cleanups.push(()=>sdk.dispose());
 const session=sdk.forSession("current","old-session");
 await session.sessions.prompt("old-session",sdkPrompt("Continue",[],{model:"openai/base"},"steer"));
 await session.sessions.update("old-session",{agent:"plan"});
 await session.sessions.abort("old-session");
  await session.interactions.permissions.reply("permission-one","once");
  await session.interactions.questions.reply("question-one",[["yes"]]);
 await session.sessions.create({directory:"/current"});
 await session.transport.request({path:"v2/providers/configured"});
 expect(requests.filter(r=>r.path.endsWith("/prompt"))).toEqual([expect.objectContaining({path:"/agent/system/v2/sessions/old-session/prompt",method:"POST",body:expect.objectContaining({parts:[{type:"text",text:"Continue"}]})})]);
 expect(requests.some(r=>r.path==="/agent/system/v2/sessions/old-session"&&r.method==="PATCH")).toBe(true);
 expect(requests.some(r=>r.path==="/agent/system/v2/sessions/old-session/abort")).toBe(true);
 expect(requests.some(r=>r.path==="/agent/system/v2/interactions/permissions/permission-one/reply")).toBe(true);
 expect(requests.some(r=>r.path==="/agent/system/v2/interactions/questions/question-one/reply")).toBe(true);
 expect(requests.some(r=>r.path==="/agent/workspaces/current/v2/sessions"&&r.method==="POST")).toBe(true);
 expect(requests.at(-1)?.path).toBe("/agent/workspaces/current/v2/providers/configured");
 expect(requests.filter(r=>r.path==="/agent/system/info")).toHaveLength(1);
});

test("session routing keeps paired and denied callers scoped and never retries a failed send",async()=>{
 for(const paired of [true,false]){
  const requests:string[]=[];
  const fetcher=(async(input:Parameters<typeof fetch>[0])=>{
   const path=new URL(String(input)).pathname;requests.push(path);
   if(path.endsWith("/info"))return Response.json({code:"forbidden",message:"Denied",retryable:false},{status:403});
   return Response.json({code:"not_found",message:"Session not found",retryable:false},{status:404});
  })as typeof fetch;
  const sdk=new AgentSdk({endpoint:"ws://localhost:7878/session",authToken:"operator",...(paired?{pairingToken:"guest"}:{})},fetcher);cleanups.push(()=>sdk.dispose());
  await expect(sdk.forSession("shared","old").sessions.prompt("old",sdkPrompt("Continue",[],{},"steer"))).rejects.toThrow();
  expect(requests).toEqual([...(paired?[]:["/agent/system/info"]),"/agent/workspaces/shared/v2/sessions/old/prompt"]);
 }
});
