// Explicitly paid, real-model experiment. Fresh chats; never import fake history.
import {mkdtempSync,mkdirSync,writeFileSync,appendFileSync,readFileSync} from "node:fs";
import {join} from "node:path";
import {tmpdir} from "node:os";
import * as Effect from "effect/Effect";
import {loadConfig} from "../src/config";
import {DaemonConnection} from "../src/daemon/client";
import {AgentSdk} from "../src/daemon/sdk";
import {launchNative} from "../tests/support/native-launch";

if(!process.argv.includes("--run-real-models"))throw Error("Explicit --run-real-models consent required; uses configured paid provider");
const duration=Number(process.env.REAL_EXPERIMENT_SECONDS||480);
if(!Number.isInteger(duration)||duration<60||duration>900)throw Error("Duration must be 60..900 seconds");
const directory=mkdtempSync(join(tmpdir(),"neoism-real-agent-experiment-"));
const state=join(directory,"session.json");
const config=loadConfig([],process.env),connection=new DaemonConnection(config),sdk=new AgentSdk(config);
const projects:Array<{id:string;root:string;name:string;session:string;children:Set<string>;expectedChildren:number;surfaces:any[];prompt:string}>=[];
const actions:Array<{phase:string;ms:number;unixMs:number}>=[];
const tools=new Map<string,string>();
const tabIds=new Map<string,string>();
const ptys:string[]=[],createdWorkspaces:string[]=[];
let terminalCommands=0;
const tracePath=join(directory,"frontend-trace.log");writeFileSync(tracePath,"",{mode:0o600});
const eventsPath=join(directory,"events.jsonl");writeFileSync(eventsPath,"",{mode:0o600});
let traceBytes=0,tracePending="",stallCount=0;
const slowestStalls:any[]=[];
const onStderr=(chunk:string)=>{
 if(traceBytes<20*1024*1024){appendFileSync(tracePath,chunk);traceBytes+=Buffer.byteLength(chunk);}
 tracePending+=chunk;
 const lines=tracePending.split("\n");tracePending=lines.pop()||"";
 for(const line of lines)if(line.startsWith("[neoism-gpuix-stall] ")){
  try{
   const event=JSON.parse(line.slice("[neoism-gpuix-stall] ".length));stallCount++;
   slowestStalls.push({unixMs:event.unixMs,reason:event.reason,delayMs:event.delayMs,cpuMs:event.cpuMs,trigger:event.slowOperation,operations:event.operations.toSorted((a:any,b:any)=>b.durationMs-a.durationMs).slice(0,5)});
   slowestStalls.sort((a,b)=>Math.max(b.delayMs,b.trigger?.durationMs||0)-Math.max(a.delayMs,a.trigger?.durationMs||0));slowestStalls.length=Math.min(10,slowestStalls.length);
  }catch{}
 }
 if(tracePending.length>65536)tracePending="";
};
let app:Awaited<ReturnType<typeof launchNative>>|undefined,observer:ReturnType<typeof setInterval>|undefined;
let observing=false,peakRunningChildren=0,idleSince=0,started=0;
const flatten=(nodes:any[]):any[]=>nodes.flatMap(node=>[node,...flatten(node.children||[])]);
const log=(phase:string,data:unknown={})=>{
 const line=JSON.stringify({phase,unixMs:Date.now(),data});appendFileSync(eventsPath,line+"\n");console.log(line);
};
const specs=[
  {name:"checkout",modules:["money","inventory","orders"],requirements:"money.ts: sumCents and allocateCents must use safe nonnegative integers and preserve totals. inventory.ts: mergeStock and reserveStock must merge duplicate SKUs and reject over-reservation without mutating inputs. orders.ts: calculateOrder must combine line quantities, integer-cent discounts and tax with deterministic rounding. Export explicit types and document rounding. Each child owns one module and its matching tests."},
  {name:"telemetry",modules:["statistics","windows"],requirements:"statistics.ts: summarize finite numeric samples with count/min/max/mean and percentile using a documented interpolation rule; empty inputs must be explicit. windows.ts: aggregate timestamped samples into fixed windows and compute rolling averages with clear boundary handling. Reject invalid samples and do not mutate input arrays. Each child owns one module and its matching tests."},
];
async function observe(){
 if(observing)return;observing=true;
 try{
  let active=0,runningChildren=0;
  for(const project of projects){
   const api=sdk.forSession(project.id,project.session);
   const [queue,history]=await Promise.all([api.sessions.queue(project.session),api.sessions.messages(project.session,{limit:100,order:"desc",slim:true})]);
   if(queue.running||queue.worker||queue.count)active++;
   for(const message of history.items)for(const part of message.parts){
    if(part.type!=="tool")continue;
    tools.set(`${message.info.id}:${part.id}`,part.tool);
    if(part.tool==="task"){
     const metadata=(part.state.status==="completed"||part.state.status==="running"?part.state.metadata:undefined) as Record<string,unknown>|undefined;
     const child=metadata?.sessionId??metadata?.sessionID;
     if(typeof child==="string")project.children.add(child);
    }
   }
   for(const child of project.children){
    const q=await sdk.forSession(project.id,child).sessions.queue(child);
    if(q.running||q.worker||q.count){runningChildren++;active++;}
   }
  }
  peakRunningChildren=Math.max(peakRunningChildren,runningChildren);
  idleSince=active?0:(idleSince||Date.now());
  log("activity",{active,runningChildren,children:projects.reduce((n,p)=>n+p.children.size,0),toolParts:tools.size});
 }catch(error){log("observe-error",String(error));}finally{observing=false;}
}
async function activate(project:typeof projects[number],surface:any){
 const before=performance.now();
 await app!.getByTestId(`workspace:${JSON.stringify(["local",project.id])}`).click();
 // Workspace restoration can asynchronously replace the tab tree after click.
 await Bun.sleep(350);
 const {tree}=await app!.call("getTree",{});
 const cachedTab=tabIds.get(surface.id);
 const tab=flatten(tree?[tree]:[]).find(node=>/^pane-tab:\d+$/.test(node.testId||"")&&(cachedTab?node.testId===cachedTab:flatten([node]).some(child=>child.text===surface.title)));
 if(!tab)throw Error(`Missing tab ${surface.title}`);
 tabIds.set(surface.id,tab.testId);
 const strip=flatten(tree?[tree]:[]).find(node=>node.testId?.startsWith("pane-tab-scroll:"));
 if(strip?.bounds)for(let attempt=0;attempt<4;attempt++){
  const bounds=await app!.getByTestId(tab.testId).bounds(),viewport=strip.bounds;
  const delta=bounds.x<viewport.x?viewport.x-bounds.x:bounds.x+bounds.width>viewport.x+viewport.width?viewport.x+viewport.width-bounds.x-bounds.width:0;
  if(Math.abs(delta)<1)break;
  await app!.call("scrollWheel",{x:viewport.x+viewport.width/2,y:viewport.y+viewport.height/2,deltaX:delta,deltaY:0});await Bun.sleep(100);
 }
 await app!.getByTestId(tab.testId).click();
 const testId=surface.kind==="agent"?"composer":surface.kind==="terminal"?"terminal":surface.kind==="git"?"git-review":surface.path.endsWith(".md")?"rich-markdown-editor":"code-editor";
 let view=app!.getByTestId(testId);await view.waitFor({timeoutMs:30000});
 if(surface.kind==="agent"&&await app!.getByTestId("conversation").count())view=app!.getByTestId("conversation");
 const measurement={phase:`activate:${project.name}:${surface.title}`,ms:performance.now()-before,unixMs:Date.now()};actions.push(measurement);log("action",measurement);
 return view;
}
try{
 connection.start();const deadline=Date.now()+15000;
 while(connection.state.phase!=="connected"){if(Date.now()>deadline)throw Error("Real daemon connection unavailable");await Bun.sleep(50);}
 for(const spec of specs){
  const root=join(directory,spec.name);mkdirSync(join(root,"src"),{recursive:true});mkdirSync(join(root,"tests"));
  writeFileSync(join(root,"package.json"),JSON.stringify({name:spec.name,private:true,type:"module",scripts:{test:"node --experimental-strip-types --test tests/*.test.ts"}}));
  writeFileSync(join(root,"tsconfig.json"),JSON.stringify({compilerOptions:{target:"ES2022",module:"NodeNext",moduleResolution:"NodeNext",strict:true,noEmit:true,allowImportingTsExtensions:true},include:["src/**/*.ts"]}));
  writeFileSync(join(root,"README.md"),`# ${spec.name}\n\n${spec.requirements}\n\nUse only Node built-ins. No network, dependencies, commits, or edits outside this disposable project. Use node:test and node:assert/strict in tests/*.test.ts. Keep TypeScript erasable for Node strip-types. Run npm test.\n`);
  for(const module of spec.modules)writeFileSync(join(root,"src",`${module}.ts`),`// Implement the ${module} module described in README.md.\nexport {};\n`);
  await Bun.$`git -C ${root} init -q`.quiet();await Bun.$`git -C ${root} add .`.quiet();
  await Bun.$`git -C ${root} -c user.name=Experiment -c user.email=experiment@example.invalid commit -qm baseline`.quiet();
  const id=`neoism-real-experiment-${crypto.randomUUID()}`;
  await Effect.runPromise(connection.request("Workspace",{CreateWorkspace:{workspace_id:id,title:`Experiment ${spec.name}`,root_dir:root}}));
  createdWorkspaces.push(id);
  const session=await sdk.createSession(id,{directory:root,title:`Build ${spec.name}`,agent:"build",model:{providerId:"openai",id:"gpt-6.1-sol",variant:"medium"},permission:[{permission:"bash",pattern:"*",action:"allow"},{permission:"edit",pattern:"*",action:"allow"},{permission:"task",pattern:"*",action:"allow"}]});
  const surfaces:any[]=[{id:`agent:${session.id}`,kind:"agent",title:session.title,workspaceId:id,sessionId:session.id,rootDir:root},...spec.modules.map(module=>({id:`file:${id}:${module}`,kind:"file",title:`${module}.ts`,path:`src/${module}.ts`,workspaceId:id,rootDir:root})),{id:`file:${id}:readme`,kind:"file",title:"README.md",path:"README.md",workspaceId:id,rootDir:root},{id:`git:${id}`,kind:"git",title:"Working changes",workspaceId:id,rootDir:root,git:{mode:"working",root}}];
  const prompt=`Implement this real small project completely. Work ONLY in ${root}. Read README.md. Start exactly ${spec.modules.length} native general subagents concurrently using the task tool, one for each module and its matching tests; give each real implementation work, not just review. Do not create further nested agents or worktrees. While they work, inspect the contracts and plan integration. Each child must use LSP diagnostics or symbols for its TypeScript module, implement the functions, add comprehensive node:test tests, and run relevant tests. Then integrate, run npm test, fix failures, and give a concise completion report. Use apply_patch for file mutations, read/grep/glob for inspection. No packages, network, commits, or files outside ${root}; only Node built-ins. Do not stop after a plan. ${spec.requirements}`;
  projects.push({id,root,name:spec.name,session:session.id,children:new Set(),expectedChildren:spec.modules.length,surfaces,prompt});
 }
 const pty:any=await Effect.runPromise(connection.request("Pty",{CreatePty:{cwd:projects[0].root,cols:100,rows:30,presentation:"grid",shell:"/bin/bash"}}));
 const ptyId=pty.PtyCreated?.session_id;if(typeof ptyId!=="string")throw Error("Real PTY creation failed");ptys.push(ptyId);
 projects[0].surfaces.push({id:`terminal:${ptyId}`,kind:"terminal",title:"Live Bash tests",workspaceId:projects[0].id,rootDir:projects[0].root,sessionId:ptyId});
 writeFileSync(state,JSON.stringify({version:1,activeWorkspaceId:projects[0].id,activeSurfaceId:projects[0].surfaces[0].id,surfaces:projects.flatMap(p=>p.surfaces),editors:{},providerStates:{},drafts:{},composerDrafts:{},ui:{sidebar:"files",openWorkspaceIds:projects.map(p=>p.id)}}),{mode:0o600});
 writeFileSync(join(directory,"experiment.json"),JSON.stringify(projects.map(({id,root,session})=>({id,root,session}))),{mode:0o600});
 app=await launchNative({command:process.execPath,args:["--preload","./scripts/display-binding-preload.ts","src/main.tsx","--workspace",projects[0].id],cwd:process.cwd(),env:{...process.env,NEOISM_GPUIX_STATE_PATH:state,NEOISM_GPUIX_PROFILE:"0",NEOISM_GPUIX_STALL_TRACE:"1"},onStderr});
 await Bun.sleep(3500);const info=await app.call("initialize",{protocolVersion:1,client:"real-agent-experiment"});
 if(process.env.HYPRLAND_INSTANCE_SIGNATURE){
  await Bun.$`hyprctl dispatch ${`hl.dsp.focus({window="pid:${info.pid}"})`}`.quiet();
  const window=JSON.parse(await Bun.$`hyprctl activewindow -j`.text());
  if(window.pid===info.pid){if(!window.floating)await Bun.$`hyprctl dispatch ${'hl.dsp.window.float({action="toggle"})'}`.quiet();await Bun.$`hyprctl dispatch ${'hl.dsp.window.resize({x=1260,y=674,relative=false})'}`.quiet();await Bun.sleep(400);}
 }
 log("ready",{directory,pid:info.pid,workspaces:projects.map(p=>({id:p.id,session:p.session,root:p.root})),tabs:projects.reduce((n,p)=>n+p.surfaces.length,0)});
 for(const project of projects){
  await activate(project,project.surfaces[0]);
  // fill() synthesizes thousands of synchronous key events, unlike a real paste.
  // Submit via the production daemon SDK; all model/tool events render normally.
  await sdk.forSession(project.id,project.session).sessions.prompt(project.session,{prompt:project.prompt,agent:"build",model:{providerId:"openai",modelId:"gpt-6.1-sol",variant:"medium"}});
  log("submitted",{project:project.name,session:project.session});
 }
 started=Date.now();observer=setInterval(()=>void observe(),5000);await observe();
 let round=0;
 while(Date.now()-started<duration*1000){
  for(const project of projects)for(const surface of project.surfaces){
   if(Date.now()-started>=duration*1000)break;
   const view=await activate(project,surface);await Bun.sleep(300);
   if(surface.kind==="agent")await view.press("end");
   if(surface.kind==="terminal"){
    const command="printf '\\033[32mLive project checks\\033[0m\\n'; node --version; git diff --stat; npm test\n";
    connection.send("Pty",{PtyInput:{session_id:surface.sessionId,bytes:[...new TextEncoder().encode(command)]}});terminalCommands++;
    await Bun.sleep(800);
   }
   const bounds=await view.bounds(),before=performance.now();
   for(let n=0;n<30;n++){
    await app.call("scrollWheel",{x:bounds.x+bounds.width/2,y:bounds.y+bounds.height/2,deltaX:0,deltaY:(n<15?1:-1)*12});await Bun.sleep(16);
   }
   actions.push({phase:`scroll:${project.name}:${surface.title}`,ms:performance.now()-before,unixMs:Date.now()});
   await Bun.sleep(surface.kind==="agent"?1600:350);
  }
  round++;await observe();
  if(idleSince&&Date.now()-idleSince>15000&&round>=3&&projects.every(p=>p.children.size>0))break;
 }
 const verification=[];
 for(const project of projects){
  const test=await Bun.$`npm test`.cwd(project.root).nothrow().quiet();
  const diff=await Bun.$`git -C ${project.root} diff --stat`.quiet();
  verification.push({project:project.name,exitCode:test.exitCode,tests:test.stdout.toString().slice(-4000),diff:diff.stdout.toString(),children:[...project.children]});
 }
 const counts:Record<string,number>={};for(const tool of tools.values())counts[tool]=(counts[tool]||0)+1;
 const audit=[],auditedTools:Record<string,number>={};
 for(const project of projects)for(const id of [project.session,...project.children]){
  const api=sdk.forSession(project.id,id);
  const [page,queue]=await Promise.all([api.sessions.messages(id,{limit:100,order:"desc",slim:true}),api.sessions.queue(id)]);
  const statuses:Record<string,number>={};
  for(const message of page.items)for(const part of message.parts)if(part.type==="tool"){
   auditedTools[part.tool]=(auditedTools[part.tool]||0)+1;statuses[part.state.status]=(statuses[part.state.status]||0)+1;
  }
  audit.push({id,messages:page.items.length,statuses,running:!!(queue.running||queue.worker||queue.count)});
 }
 const completed=!!idleSince&&audit.every(session=>!session.running)&&projects.every(p=>p.children.size===p.expectedChildren);
 const summary={directory,eventsPath,completed,tracePath,traceBytes,traceCapped:traceBytes>=20*1024*1024,stallCount,slowestStalls,elapsedSeconds:(Date.now()-started)/1000,rounds:round,peakRunningChildren,terminalCommands,toolParts:counts,auditedTools,audit,auditScope:"last 100 messages per real session; top-level parts exclude nested compose calls",verification,actions:actions.length,actionScope:"automation wall time includes deliberate settling and scroll pacing; not presentation latency",slowestActions:actions.toSorted((a,b)=>b.ms-a.ms).slice(0,15)};
 writeFileSync(join(directory,"results.json"),JSON.stringify(summary,null,2),{mode:0o600});log("complete",summary);
 if(!completed||verification.some(v=>v.exitCode!==0)||!terminalCommands||!auditedTools.bash||!auditedTools.lsp)process.exitCode=1;
}catch(error){
 if(app){try{writeFileSync(join(directory,"failure-tree.json"),JSON.stringify(await app.call("getTree",{})),{mode:0o600});}catch{}}
 log("failure",{message:String(error),stallCount,slowestStalls});throw error;
}finally{
 if(observer)clearInterval(observer);
 while(observing)await Bun.sleep(50);
 try{
  const saved=JSON.parse(readFileSync(state,"utf8"));
  for(const [child,parent]of Object.entries(saved.ui?.agentParentSessions||{})){
   const project=projects.find(p=>p.session===parent);project?.children.add(child);
  }
 }catch{}
 for(const project of projects){
  for(const id of [...project.children,project.session])await sdk.forSession(project.id,id).sessions.abort(id).catch(()=>{});
 }
 await app?.close();
 for(const id of ptys)await Effect.runPromise(connection.request("Pty",{ClosePty:{session_id:id}})).catch(()=>{});
 for(const id of createdWorkspaces)await Effect.runPromise(connection.request("Workspace",{CloseHostWorkspace:{workspace_id:id}})).catch(()=>{});
 sdk.dispose();connection.stop();
 log("cleanup",{directory,preservedProjects:true,existingWorkspacesUntouched:true});
}