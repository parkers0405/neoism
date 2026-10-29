import type { ServerWebSocket } from "bun";
import { applyEdits, modify, parse } from "jsonc-parser";
// An isolated protocol fixture used only by tests. The production frontend has
// no fixture mode and connects through the same daemon transport in every test.
export function createAgentDaemon(config: { questionsOnSwitch?: boolean;rejectSessionUpdates?:boolean; deferCreations?: boolean; configuration?: Record<string, unknown>;slashCommands?:readonly {name:string;description?:string}[]; binaryFiles?: Readonly<Record<string, Uint8Array>>; directoryEntries?: Readonly<Record<string, ReadonlyArray<{ name: string; is_dir: boolean }>>>; fileReadDelayMs?: number;gridPresentation?:boolean;scopedFiles?:Readonly<Record<string,Readonly<Record<string,string>>>>;workspaceCreateError?:string;workspaceCreateDelayMs?:number;promptError?:string;terminalDirectoryChange?:"success"|"failure"|"ack"|"defer";terminalDirectoryResult?:string;gitHandler?:(message:unknown,root:string|undefined,reply:(message:unknown)=>void)=>void;agentConfigDefaults?:Readonly<Record<string,unknown>>;agentDefaultsDelayMs?:Readonly<Record<string,number>>;agentProviderCatalog?:unknown;usageResponse?:unknown;usageDelayMs?:number;deferWorkspaceSubscriptions?:boolean;agentCatalogEvents?:readonly unknown[] } = {}) {
  const workspaces = [{ id: "a", host_id: "host", title: "A", root_dir: "/project/a" }, { id: "b", host_id: "host", title: "B", root_dir: "/project/b" }];
  const threads = [{ session_id: "one", title: "One", directory: "/project/a", agent: "build", model: "openai/base" }, { session_id: "two", title: "Two", directory: "/project/a", agent: "plan", model: "other/second" }];
  const history=new Map<string,unknown[]>(),agentStreams=new Set<string>();
  const requests: Array<{ tag: string; body: Record<string, unknown> }> = [];
  const files = new Map([["README.md", "# Neoism\n" + Array.from({ length: 80 }, (_, i) => `Line ${i + 1}`).join("\n")]]);
  const fileRequests: Array<{ tag: string; body: Record<string, unknown> }> = [];
  const workspaceRequests: Array<{ tag: string; body: Record<string, unknown> }> = [];
  const gitRequests:Array<{message:unknown;root?:string}>=[];
  const searchRequests:Array<{tag:string;body:Record<string,unknown>}>=[];
  const ptyRequests:Array<{tag:string;body:Record<string,unknown>}>=[],socketWorkspaces=new WeakMap<object,string>(),ptyModes=new Map<string,unknown>();let nextPty=0;
  const creations: Array<() => void> = [];
  const workspaceSubscriptions:Array<{workspaceId:string;release:()=>void}>=[];
  const directoryChanges:Array<()=>void>=[];
  const httpRequests: Array<{ path: string; authorization: string | null; directory: string | null }> = [];
  const configRequests: Array<{ tag: string; body: Record<string, unknown> }> = [];
  let configContent = JSON.stringify(config.configuration || { appearance: { theme: "pastel_dark", fonts: { family: "Cascadia Code NF", size: 14 } }, editor: { "vim-mode": true } }, null, 2), configRevision = 1;
  const configDocument = () => ({ content: configContent, display_path: "/fixture/config.json", revision: `r${configRevision}`, writable: true });
  const descriptors = [{ path: "editor.vim-mode", label: "Modal editing", description: "Original Vim engine", value_kind: "boolean", default: true, static_suggestions: [], runtime_suggestions: [], options: [], constraints: { nullable: false }, accepted_kinds: ["boolean"], extensible: false, category: "general", control: { kind: "toggle" } }];
  let answers = 0, asked = false;
  type Selection={agent:string;model:string;thinking:string;connection_id?:string};
  const selections = new Map<string,Selection>(threads.map((thread) => [thread.session_id, { agent: thread.agent, model: thread.model, thinking: "medium" }]));
  const socketAgentStreams=new Map<ServerWebSocket<unknown>,Set<string>>();
  const streamSubscribers=(sessionId:string)=>[...socketAgentStreams.values()].filter(streams=>streams.has(sessionId)).length;
  const sockets=new Set<ServerWebSocket<unknown>>(),gitWatchers=new Map<string,ServerWebSocket<unknown>>();
  const broadcast=(family:string,message:unknown)=>{const wire=JSON.stringify({[family+"Reply"]:{request_id:0,message}});for(const socket of sockets)socket.send(wire);};
  const push = (message:unknown)=>broadcast("Agent",message);
  const pushSubscribed=(message:any)=>{const body=typeof message==="object"&&message?Object.values(message)[0]as any:undefined;for(const[socket,streams]of socketAgentStreams)if(!body?.session_id||streams.has(body.session_id))socket.send(JSON.stringify({AgentReply:{request_id:0,message}}));};
  const pushGit=(message:unknown)=>{
    const token=(message as {RepoStatus?:{token?:string}})?.RepoStatus?.token;
    if(token){gitWatchers.get(token)?.send(JSON.stringify({GitReply:{request_id:0,message}}));return;}
    broadcast("Git",message);
  };
  const pushCrdt=(message:unknown)=>broadcast("Crdt",message);
  const crdtRequests:Array<{tag:string;body:Record<string,unknown>}>=[];
  const pushPty=(message:unknown)=>broadcast("Pty",message);
  const pushWorkspace=(message:unknown)=>broadcast("Workspace",message);
  const catalogStreams=new Map<ReadableStreamDefaultController<Uint8Array>,string|undefined>();
  const catalogInScope=(event:unknown,directory?:string)=>{if(!directory||!event||typeof event!=="object")return true;const data=(event as {data?:any;properties?:any}).data||(event as {properties?:any}).properties;const scope=data?.snapshot?.directory||data?.info?.directory;return !scope||scope===directory;};
  const pushCatalog=(event:unknown)=>{const bytes=new TextEncoder().encode(`data: ${JSON.stringify(event)}\n\n`);for(const[stream,directory]of catalogStreams)if(catalogInScope(event,directory)){try{stream.enqueue(bytes);}catch{catalogStreams.delete(stream);}}};
  const server = Bun.serve({ hostname: "127.0.0.1", port: 0,
    fetch: async (request, server) => {
      const url = new URL(request.url);
      if (url.pathname === "/session") return server.upgrade(request) ? undefined : new Response("", { status: 400 });
      if (url.pathname === "/health") return Response.json({ok:true});
      httpRequests.push({ path: url.pathname, authorization: request.headers.get("authorization"), directory: url.searchParams.get("directory") });
      const match = url.pathname.match(/^\/agent\/workspaces\/([^/]+)\/v2\/(.*)$/);
      if (!match) return new Response("Not found", { status: 404 });
      if(!workspaces.some(workspace=>workspace.id===match[1]))return new Response("Unknown workspace",{status:404});
      if(match[2]==="session-catalog/events"){
        const directory=url.searchParams.get("directory")||undefined;
        let controller:ReadableStreamDefaultController<Uint8Array>|undefined;
        const stream=new ReadableStream<Uint8Array>({start(value){controller=value;catalogStreams.set(value,directory);for(const event of config.agentCatalogEvents||[])if(catalogInScope(event,directory))value.enqueue(new TextEncoder().encode(`data: ${JSON.stringify(event)}\n\n`));},cancel(){if(controller)catalogStreams.delete(controller);}});
        return new Response(stream,{headers:{"content-type":"text/event-stream","cache-control":"no-cache"}});
      }
      if(match[2]==="session-projects")return Response.json({items:[...new Set(threads.map(thread=>thread.directory))].map((directory,index)=>({directory,updatedMs:100-index})),pinnedSessions:[]});
      if(match[2]==="config/defaults"){const delay=config.agentDefaultsDelayMs?.[match[1]];if(delay)await Bun.sleep(delay);return Response.json(config.agentConfigDefaults?.[match[1]]||{defaultAgent:null,model:null,variant:null});}
      if(match[2]==="providers/openai/usage"){if(config.usageDelayMs)await Bun.sleep(config.usageDelayMs);return Response.json(config.usageResponse||{accounts:[]});}
      if (match[2] === "providers/configured") return Response.json(config.agentProviderCatalog||{ default: {}, providers: [{ id: "openai", name: "OpenAI", models: {
        base: { id: "base", name: "Base", limit: { context: 128000 } }, advanced: { id: "advanced", name: "Advanced", limit: { context: 256000 } },
      } }] });
      if (match[2] === "commands") return Response.json(config.slashCommands||[]);
      if (match[2] === "agents") return Response.json([{ name: "build", description: "Build" }, { name: "plan", description: "Plan" }, { name: "internal", hidden: true }]);
      if (match[2] === "skills") return Response.json([{ name: "review", description: "Review" }]);
      const workspace = workspaces.find((w) => w.id === match[1])!;
      const session = (id: string) => {
        const selection:Selection = selections.get(id) || { agent: "build", model: "openai/base", thinking: "medium" };
        const slash = selection.model.indexOf("/");
        return { id, title: "Created", directory: workspace.root_dir, agent: selection.agent,
          model: { providerId: selection.model.slice(0, slash), id: selection.model.slice(slash + 1), variant: selection.thinking,connectionId:selection.connection_id } };
      };
      const payload = request.method === "POST" || request.method === "PATCH" ? await request.json().catch(() => ({})) : {};
      if(match[2]==="sessions"&&request.method==="GET")return Response.json({items:threads.filter(thread=>!url.searchParams.get("directory")||thread.directory===url.searchParams.get("directory")).map(thread=>{const selected=selections.get(thread.session_id)||thread,model=selected.model?.split("/");return{id:thread.session_id,title:thread.title,directory:thread.directory,agent:selected.agent,...(model&&model.length>1?{model:{providerId:model[0],id:model.slice(1).join("/")}}:{})};}),cursor:{}});
      if (match[2] === "sessions" && request.method === "POST") {
        const complete = () => Response.json(session("created-a"));
        requests.push({ tag: "CreateThread", body: { directory: workspace.root_dir, ...payload } });
        if (config.deferCreations === false) return complete();
        return new Promise<Response>((resolve) => creations.push(() => resolve(complete())));
      }
      const route = match[2].match(/^sessions\/([^/]+)(?:\/(.*))?$/);
      if (route) {
        const id = decodeURIComponent(route[1]);
        if (request.method === "PATCH") {
          if(config.rejectSessionUpdates)return Response.json({code:"not_found",message:"Session not found",retryable:false,details:{}},{status:404});
          const old:Selection = selections.get(id) || { agent: "build", model: "openai/base", thinking: "medium" };
          const next:Selection = { agent: payload.agent || old.agent, model: payload.model ? `${payload.model.providerId}/${payload.model.id}` : old.model, thinking: payload.model?.variant || old.thinking,connection_id:payload.model?payload.model.connectionId:old.connection_id };
          selections.set(id, next); requests.push({ tag: "UpdateSession", body: { session_id: id, ...payload } });
          return Response.json(session(id));
        }
        if (route[2] === "prompt") {
          requests.push({ tag: "SubmitPrompt", body: { session_id: id, message_id: payload.messageId,
            author:payload.author,
            text: payload.parts?.find((p: any) => p.type === "text")?.text, mode: payload.agent,
            model: payload.model ? `${payload.model.providerId}/${payload.model.modelId}` : undefined,
            thinking: payload.model?.variant, delivery: payload.delivery,
            attachments: payload.parts?.filter((p: any) => p.type === "file").map((p: any) => ({ kind: p.mime, path: p.filename, bytes: [...Buffer.from(p.url.split(",")[1], "base64")] })),
          } });
          if(config.promptError)return Response.json({code:"fixture.prompt",message:config.promptError,retryable:false,details:{}},{status:400});
          return new Response(null, { status: 204 });
        }
        if (route[2] === "queue" && request.method === "DELETE") {
          requests.push({ tag: "ClearQueue", body: { session_id: id } }); push({ QueueUpdate: { session_id: id, count: 0 } });
          return Response.json({ count: 0 });
        }
        if (route[2] === "abort") { requests.push({ tag: "CancelInflight", body: { session_id: id } }); return Response.json(true); }
      }
      const interaction = match[2].match(/^interactions\/(questions|permissions)\/([^/]+)\/(reply|reject)$/);
      if (interaction) {
        const id = decodeURIComponent(interaction[2]);
        if (interaction[1] === "permissions") {
          requests.push({ tag: "ApproveTool", body: { request_id: id, decision: payload.reply === "always" ? "Always" : payload.reply === "once" ? "Yes" : "No" } });
          return Response.json(true);
        }
        requests.push({ tag: interaction[3] === "reject" ? "RejectQuestion" : "AnswerQuestion", body: { request_id: id, session_id: "one", answers: payload.answers } });
        answers++;
        if (answers === 1) return Response.json({ code: "fixture.retry", message: "Try again", retryable: true, details: {} }, { status: 503 });
        return Response.json(true);
      }
      return new Response("Not found", { status: 404 });
    },
    websocket: {
      open(client) { sockets.add(client);socketAgentStreams.set(client,new Set()); },
      close(client){sockets.delete(client);const streams=socketAgentStreams.get(client);socketAgentStreams.delete(client);for(const id of streams||[])if(!streamSubscribers(id))agentStreams.delete(id);for(const[token,owner]of gitWatchers)if(owner===client)gitWatchers.delete(token);},
      message(client, data) {
        const [family, envelope] = Object.entries(JSON.parse(String(data)))[0] as [string, { request_id: number; message: unknown;workspace_root?:string }];
        const reply = (message: unknown) => client.send(JSON.stringify({ [family + "Reply"]: { request_id: envelope.request_id, message } }));
        const body = typeof envelope.message === "string" ? {} : envelope.message as Record<string, unknown>;
        const tag = typeof envelope.message === "string" ? envelope.message : Object.keys(body)[0];
        const payload = (body[tag] || {}) as Record<string, unknown>;
        if(family==="Crdt"){crdtRequests.push({tag,body:payload});return;}
        if(family==="Git"){if(tag==="WatchStatus"){for(const[token,owner]of gitWatchers)if(owner===client)gitWatchers.delete(token);gitWatchers.set(String(payload.token),client);}else if(tag==="UnwatchStatus")gitWatchers.delete(String(payload.token));gitRequests.push({message:envelope.message,root:envelope.workspace_root});if(config.gitHandler)config.gitHandler(envelope.message,envelope.workspace_root,reply);else reply("Ack");return;}
        if (family === "Workspace") {
          workspaceRequests.push({ tag, body: payload });
          if (tag === "Hello") reply({ HelloAck: { accepted: true } });
          if (tag === "RequestHostWorkspaceTree") reply({ HostWorkspaceTree: { hosts: [{ id: "host", label: "Host" }], workspaces, tabs: [] } });
          if (tag === "ResolveInitialWorkspace") reply({ InitialWorkspaceResolved: { workspace: workspaces[0], reason: "host_active" } });
          if(tag==="CreateWorkspace"){
            const id=String(payload.workspace_id||crypto.randomUUID());
            let workspace=workspaces.find(entry=>entry.id===id);
            if(!workspace){workspace={id,host_id:"host",title:String(payload.title||"Workspace"),root_dir:payload.root_dir==="~"?"/home/fixture-remote":String(payload.root_dir||"/project")};workspaces.push(workspace);}
            reply({HostWorkspaceUpserted:{workspace}});
          }
          if(tag==="CreateHostWorkspace"){
            const complete=()=>{
              if(config.workspaceCreateError){reply({Error:{message:config.workspaceCreateError}});return;}
              const workspace={id:String(payload.workspace_id),host_id:String(payload.host_id),title:String(payload.title||"Workspace"),root_dir:String(payload.root_dir||"/project")};
              workspaces.push(workspace);reply({HostWorkspaceUpserted:{workspace}});
              reply({HostWorkspaceList:{workspaces}});
            };
            if(config.workspaceCreateDelayMs)setTimeout(complete,config.workspaceCreateDelayMs);else complete();
          }
          if (tag === "SubscribeWorkspace") {socketWorkspaces.set(client,String(payload.workspace_id));const complete=()=>reply({ HostWorkspaceUpserted: { workspace: workspaces.find((w) => w.id === payload.workspace_id) } });if(config.deferWorkspaceSubscriptions)workspaceSubscriptions.push({workspaceId:String(payload.workspace_id),release:complete});else complete();}
          if(tag==="SetWorkspaceRoot"){const workspace=workspaces.find(entry=>entry.id===payload.workspace_id);if(workspace){workspace.root_dir=String(payload.root_dir);reply({HostWorkspaceUpserted:{workspace}});}else reply({Error:{message:"Unknown workspace"}});}
          if(tag==="BindEditorSurface")reply({EditorSurfaceChanged:{surface:{surface_id:payload.surface_id,session_id:payload.session_id,workspace_id:socketWorkspaces.get(client)||"a",path:payload.path}}});
        }
        if(family==="Search"){
          searchRequests.push({tag,body:payload});const req_id=payload.req_id??envelope.request_id,query=String(payload.query||"").toLowerCase();
          if(tag==="CollectFiles")reply({CollectFilesResult:{req_id,paths:[...files.keys()]}});
          else if(tag==="SearchFiles")reply({SearchFilesResult:{req_id,hits:[...files.keys()].filter(path=>path.toLowerCase().includes(query)).map(path=>({path,score:100}))}});
          else if(tag==="SearchGrep")reply({SearchGrepResult:{req_id,hits:[...files].flatMap(([path,text])=>text.split("\n").flatMap((line,index)=>line.toLowerCase().includes(query)?[{path,line:index+1,column:line.toLowerCase().indexOf(query),text:line,score:100}]:[]))}});
          else if(tag==="SearchDirectories")reply({SearchDirectoriesResult:{req_id,hits:(config.directoryEntries?.[String(payload.cwd)]||[]).filter(entry=>entry.is_dir&&entry.name.toLowerCase().includes(query)).map(entry=>({path:String(payload.cwd).replace(/\/$/,"")+"/"+entry.name,score:100}))}});
        }
        if (family === "Files") {
          fileRequests.push({ tag, body: payload });
          if (tag === "ReadFile") {
            const send = () => reply({ FileContent: { path: payload.path, bytes: [...(config.binaryFiles?.[String(payload.path)] || new TextEncoder().encode(config.scopedFiles?.[envelope.workspace_root||""]?.[String(payload.path)]??files.get(String(payload.path))??""))] } });
            if (config.fileReadDelayMs) setTimeout(send, config.fileReadDelayMs); else send();
          }
          else if (tag === "WriteFile") { files.set(String(payload.path), new TextDecoder().decode(Uint8Array.from(payload.bytes as number[]))); reply({ FileWritten: { path: payload.path } }); }
          else reply({ DirListing: { path: String(payload.path || ""), entries: config.directoryEntries?.[String(payload.path || "")] || [...files.keys()].map((name) => ({ name, is_dir: false })) } });
        }
        if (family === "Pty") {
          ptyRequests.push({tag,body:payload});
          if(tag==="ChangeTerminalDirectory"){
            const complete=()=>{
              if(!payload.directory||config.terminalDirectoryChange==="failure")reply({TerminalDirectoryChangeFailed:{session_id:payload.session_id,request_id:payload.request_id,message:payload.directory?"Shell is busy or unsupported":"Directory request cancelled"}});
              else if(config.terminalDirectoryChange==="ack")reply("Ack");
              else reply({TerminalDirectoryChanged:{session_id:payload.session_id,request_id:payload.request_id,directory:config.terminalDirectoryResult||payload.directory}});
            };
            if(config.terminalDirectoryChange==="defer"&&payload.directory)directoryChanges.push(complete);else complete();
            return;
          }
          if (tag === "CreatePty" || tag === "AttachPty") {
            const id=tag==="CreatePty"?(++nextPty===1?"fixture-pty":`fixture-pty-${nextPty}`):String(payload.session_id);
            if(tag==="CreatePty")ptyModes.set(id,payload.presentation||"blocks");
            reply({ PtyCreated: { session_id:id, shell: "/bin/sh",...(config.gridPresentation===false?{}:{presentation:ptyModes.get(id)||"grid"}) } });
            if (tag === "AttachPty") reply({ PtyOutput: { session_id:id, offset: 0, bytes: [36, 32] } });
          } else reply("Ack");
        }
        if (family === "Config") {
          configRequests.push({ tag, body: payload });
          if (tag === "GetConfig") reply({ Config: { value: parse(configContent) } });
          else if (tag === "GetConfigSchema") reply({ ConfigSchema: { descriptors } });
          else if (tag === "GetConfigDocument" || tag === "EnsureConfigDocument") reply({ ConfigDocument: { document: configDocument() } });
          else if (tag === "SaveConfigDocument") {
            if (payload.expected_revision !== `r${configRevision}`) reply({ Error: { message: "Config revision conflict: reload before saving" } });
            else { configContent = String(payload.content); configRevision++; reply({ ConfigDocumentSaved: { document: configDocument() } }); }
          } else if (tag === "SetSetting") {
            configContent = applyEdits(configContent, modify(configContent, String(payload.key).split("."), payload.value, { formattingOptions: { insertSpaces: true, tabSize: 2 } }));
            configRevision++; reply({ SettingWritten: { key: payload.key } });
          } else if (tag === "SetKeybind") reply({ KeybindWritten: { action: payload.action } });
          else if (tag === "ListExtensions") reply({ Extensions: { entries: [{ id: "rust", name: "Rust Analyzer", version: "1", description: "Language server", author: "Neoism", status: "installed", categories: ["Language Servers"], languages: ["Rust"] }] } });
          else if (tag === "ListMashupPacks") reply({ MashupPacks: { entries: [] } });
          else reply({ Error: { message: `Unsupported fixture config operation: ${tag}` } });
          return;
        }
        if (family !== "Agent") return;
        requests.push({ tag, body: payload });
        if (tag === "ListThreads") reply({ ThreadList: { threads: threads.filter((t) => t.directory === payload.directory) } });
        if (tag === "ListProviders") reply({ ProviderCatalog: { providers: [{ id: "openai", name: "OpenAI", models: [{ id: "base", name: "Base", context_limit: 128000 }, { id: "advanced", name: "Advanced", context_limit: 256000 }] }] } });
        if (tag === "ListAgents") reply({ AgentCatalog: { agents: [{ name: "build", description: "Build" }, { name: "plan", description: "Plan" }] } });
        if (tag === "ListSkills") reply({ SkillCatalog: { skills: [{ name: "review", description: "Review" }] } });
        if (tag === "SwitchThread") {
          socketAgentStreams.get(client)!.add(String(payload.session_id));agentStreams.add(String(payload.session_id));reply({ThreadSwitched:{session_id:payload.session_id}});
          const thread = threads.find((t) => t.session_id === payload.session_id);
          if (thread) reply({ ProviderState: { session_id: thread.session_id, ...selections.get(thread.session_id), context_limit: 128000 } });
          if (config.questionsOnSwitch && !asked) {
            asked = true; push({ QuestionAsked: { session_id: payload.session_id, request: { id: "native-question", questions: [
              { question: "Color?", options: [{ label: "Red", description: "Warm" }, { label: "Blue", description: "Cool" }] },
              { question: "When?", options: [] },
            ] } } });
          }
        }
        if(tag==="ResumeStream"){socketAgentStreams.get(client)!.add(String(payload.session_id));agentStreams.add(String(payload.session_id));}
        if(tag==="StopStream"){const id=String(payload.session_id);socketAgentStreams.get(client)!.delete(id);if(!streamSubscribers(id))agentStreams.delete(id);}
        if (tag === "GetHistory") reply({ HistoryChunk: { session_id: payload.session_id, messages: history.get(String(payload.session_id))||[] } });
        if (tag === "CreateThread") {
          const complete = () => reply({ ThreadCreated: { session_id: "created-a", title: "Created", directory: payload.directory } });
          if (config.deferCreations === false) complete(); else creations.push(complete);
        }
        if (["SetAgent", "SetModel", "SetThinkingMode"].includes(tag)) {
          const id = String(payload.session_id), selection = selections.get(id) || { agent: "build", model: "openai/base", thinking: "medium" };
          if (tag === "SetAgent") selection.agent = String(payload.agent);
          if (tag === "SetModel") selection.model = String(payload.model);
          if (tag === "SetThinkingMode") selection.thinking = String(payload.thinking);
          selections.set(id, selection); reply({ ProviderState: { session_id: id, ...selection,provider_id:selection.model.split("/")[0], context_limit: 128000 } });
        }
        if (tag === "AnswerQuestion" && config.questionsOnSwitch) {
          answers++;
          if (answers === 1) reply({ QuestionReplyFailed: { request_id: payload.request_id, error: "Try again" } });
          else reply({ QuestionRemoved: { session_id: payload.session_id, request_id: payload.request_id } });
        }
        if (tag === "ApproveTool") reply({ PermissionRemoved: { session_id: payload.session_id, request_id: payload.request_id } });
        if (tag === "ClearQueue") reply({ QueueUpdate: { session_id: payload.session_id, count: 0 } });
      },
    },
  });
  return { endpoint: `ws://127.0.0.1:${server.port}/session`, workspaces, threads,history,agentStreams,streamSubscribers,pushSubscribed,pushCrdt,crdtRequests, requests, httpRequests, files, fileRequests, workspaceRequests,searchRequests,gitRequests,ptyRequests, creations,workspaceSubscriptions,directoryChanges, push,pushCatalog,pushGit,pushPty,pushWorkspace, configRequests, configDocument, descriptors,
    mutateConfigDocument: (content: string) => { configContent = content; configRevision++; }, stop: () => {for(const stream of catalogStreams.keys()){try{stream.close();}catch{}}catalogStreams.clear();server.stop(true);} };
}
