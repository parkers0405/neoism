// Replay a completed real project's tests without issuing any model requests.
import {mkdtempSync,writeFileSync,appendFileSync,readFileSync} from "node:fs";
import {join,resolve} from "node:path";
import * as Effect from "effect/Effect";
import {loadConfig} from "../src/config";
import {DaemonConnection} from "../src/daemon/client";
import {launchNative} from "../tests/support/native-launch";
const project=resolve(process.argv[2]||"");
if(!project.startsWith("/tmp/neoism-real-agent-experiment-")||!project.endsWith("/checkout"))throw Error("Pass a completed disposable checkout project");
const dir=mkdtempSync("/tmp/neoism-terminal-replay-"),trace=join(dir,"trace.log");writeFileSync(trace,"",{mode:0o600});
const client=new DaemonConnection(loadConfig([],process.env)),workspace=`terminal-replay-${crypto.randomUUID()}`;
let app:Awaited<ReturnType<typeof launchNative>>|undefined,pty:string|undefined,phase="startup",pending="",traceBytes=0;
const events:any[]=[],metrics:any[]=[],stalls:any[]=[];
const onStderr=(chunk:string)=>{
 if(traceBytes<20*1024*1024){appendFileSync(trace,chunk);traceBytes+=Buffer.byteLength(chunk);}
 pending+=chunk;const lines=pending.split("\n");pending=lines.pop()||"";
 for(const line of lines){
  if(line.startsWith("[neoism-gpuix-stall] ")){const data=JSON.parse(line.slice(line.indexOf("{")));stalls.push({phase,unixMs:data.unixMs,delayMs:data.delayMs,trigger:data.slowOperation,nativeDraw:data.nativeDrawStats});}
  if(line.startsWith("[neoism-gpuix-profile] ")){const data=JSON.parse(line.slice(line.indexOf("{")));metrics.push({phase,unixMs:Date.now(),operations:data.operations});}
 }
 if(pending.length>65536)pending="";
};
try{
 client.start();const deadline=Date.now()+15000;
 while(client.state.phase!=="connected"){if(Date.now()>deadline)throw Error("Daemon unavailable");await Bun.sleep(50);}
 await Effect.runPromise(client.request("Workspace",{CreateWorkspace:{workspace_id:workspace,title:"Terminal replay",root_dir:project}}));
 const result:any=await Effect.runPromise(client.request("Pty",{CreatePty:{cwd:project,cols:100,rows:30,presentation:"grid",shell:"/bin/bash"}}));
 pty=result.PtyCreated?.session_id;if(!pty)throw Error("PTY creation failed");
 const surface={id:`terminal:${pty}`,kind:"terminal",title:"Replay",workspaceId:workspace,rootDir:project,sessionId:pty},state=join(dir,"state.json");
 writeFileSync(state,JSON.stringify({version:1,activeWorkspaceId:workspace,activeSurfaceId:surface.id,surfaces:[surface],editors:{},providerStates:{},drafts:{},composerDrafts:{},ui:{openWorkspaceIds:[workspace]}}),{mode:0o600});
 app=await launchNative({command:process.execPath,args:["--preload","./scripts/display-binding-preload.ts","src/main.tsx","--workspace",workspace],cwd:process.cwd(),env:{...process.env,NEOISM_GPUIX_STATE_PATH:state,NEOISM_GPUIX_PROFILE:"1",NEOISM_GPUIX_STALL_TRACE:"1"},onStderr});
 await Bun.sleep(3000);const info=await app.call("initialize",{protocolVersion:1,client:"terminal-replay"});
 if(process.env.HYPRLAND_INSTANCE_SIGNATURE){await Bun.$`hyprctl dispatch ${`hl.dsp.focus({window="pid:${info.pid}"})`}`.quiet();const window=JSON.parse(await Bun.$`hyprctl activewindow -j`.text());if(window.pid===info.pid){if(!window.floating)await Bun.$`hyprctl dispatch ${'hl.dsp.window.float({action="toggle"})'}`.quiet();await Bun.$`hyprctl dispatch ${'hl.dsp.window.resize({x=1260,y=674,relative=false})'}`.quiet();}}
 const saved=JSON.parse(readFileSync(state,"utf8"));
 const routes=saved.ui?.layouts?.[workspace]?.routes||{};
 const leaf=Object.entries(routes).find(([,route])=>(route as {id?:string}).id===surface.id)?.[0]||"1";
 await app.getByTestId(`pane-tab:${leaf}`).click();
 const terminal=app.getByTestId("terminal");await terminal.waitFor({timeoutMs:10000});await terminal.click();
 for(const mode of ["idle","tests","burst","idle-after"]){
  phase=mode;events.push({phase,unixMs:Date.now()});console.log(JSON.stringify({phase,dir,pid:info.pid}));
  if(mode==="tests")for(let n=0;n<8;n++){client.send("Pty",{PtyInput:{session_id:pty,bytes:[...new TextEncoder().encode("npm test\n")]}});await Bun.sleep(1500);}
  else if(mode==="burst"){
   // A finite, realistic log burst; no unbounded yes/cat process.
   const command="node -e 'let i=0;const t=setInterval(()=>{console.log(\"build output \"+(i++)+\" progress=ok\");if(i===1000)clearInterval(t)},1)'\n";
   client.send("Pty",{PtyInput:{session_id:pty,bytes:[...new TextEncoder().encode(command)]}});await Bun.sleep(6000);
  }else await Bun.sleep(5000);
 }
 const report={dir,pid:info.pid,events,stalls,metrics};writeFileSync(join(dir,"results.json"),JSON.stringify(report,null,2),{mode:0o600});
 console.log(JSON.stringify({dir,stalls,metrics:metrics.map(m=>({phase:m.phase,operations:Object.fromEntries(Object.entries(m.operations).filter(([name])=>["renderer.applyBatch","native.ghostty.feed","core.ghostty.feed","animation.batch","terminal.paint"].includes(name)))}))}));
}finally{await app?.close();if(pty)await Effect.runPromise(client.request("Pty",{ClosePty:{session_id:pty}})).catch(()=>{});await Effect.runPromise(client.request("Workspace",{CloseHostWorkspace:{workspace_id:workspace}})).catch(()=>{});client.stop();}