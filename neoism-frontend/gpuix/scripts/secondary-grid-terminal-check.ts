import { Effect, Schema } from "effect";
import { mkdtempSync, mkdirSync, writeFileSync, rmSync, existsSync } from "node:fs";
import { join, resolve } from "node:path";
import { tmpdir } from "node:os";
import { DaemonConnection } from "../src/daemon/client";
import { TreeSchema, expectVariant, isRecord, variant } from "../src/daemon/protocol";
// An isolated temporary daemon: never connects to the user's daemon or PTYs.
const binary=resolve(process.argv[2] || "../../target/debug/neoism-workspace-daemon");
const directory=mkdtempSync(join(tmpdir(),"neoism-grid-daemon-proof-")),home=join(directory,"home"),profiles=join(directory,"profiles"),workspace=join(directory,"workspace"),target=join(workspace,"repo 'quoted' ; $(touch INJECTED)");
for(const path of [home,profiles,workspace,target])mkdirSync(path,{recursive:true});
writeFileSync(join(home,".bashrc"),"PS1='NEOISM_GRID>>> '\nprintf 'PROFILE_%s\\n' LOADED\n");
writeFileSync(join(profiles,".zshenv"),"export GRID_ZSHENV=loaded\n");
writeFileSync(join(profiles,".zshrc"),"PROMPT='NEOISM_GRID>>> '\nprintf 'PROFILE_%s:%s\\n' LOADED $GRID_ZSHENV\n");
// PTY proof requires no Agent work. Satisfy only startup namespace association
// locally so this isolated host cannot contact the user's Agent service.
const agent=Bun.serve({hostname:"127.0.0.1",port:0,fetch(request){return new URL(request.url).pathname==="/v2/hosting/associate"?Response.json({workspaceId:"isolated-grid-proof"}):new Response("Not used by PTY proof",{status:404});}});
const shell=process.env.GRID_TEST_SHELL||"/bin/bash",nonce=`grid-proof-${process.pid}`;
const child=Bun.spawn([binary,"--neoism-internal-fast-workspace-daemon","--addr","127.0.0.1:0","--no-unix-socket","--ephemeral","--state-dir",join(directory,"state"),"--temporary-parent-pid",String(process.pid),"--workspace",workspace],{cwd:workspace,detached:true,env:{...process.env,HOME:home,ZDOTDIR:profiles,SHELL:shell,XDG_CONFIG_HOME:join(directory,"xdg-config"),XDG_DATA_HOME:join(directory,"xdg-data"),XDG_CACHE_HOME:join(directory,"xdg-cache"),XDG_RUNTIME_DIR:join(directory,"xdg-runtime"),NEOISM_CONFIG_DIR:join(directory,"config"),NEOISM_WORKSPACE_REGISTRY:join(directory,"registry.json"),NEOISM_DAEMON_DATA_DIR:join(directory,"data"),NEOISM_AGENT_SERVER:`http://127.0.0.1:${agent.port}`,NEOISM_FAST_READINESS_NONCE:nonce,NEOISM_DAEMON_TOKEN:nonce,NEOISM_REQUIRE_AUTH:"0"},stdout:"pipe",stderr:Bun.file(join(directory,"daemon.log"))});
let client:DaemonConnection|undefined,sessionId:string|undefined,output="",cwd:unknown,changed:unknown,stage="readiness";
const until=async(check:()=>boolean,description:string)=>{const end=Date.now()+15000;while(!check()){if(Date.now()>end)throw new Error(`Timeout ${description}: ${output.slice(-2000)}`);await Bun.sleep(10);}};
try {
  const reader=child.stdout.getReader();let line="";const deadline=Date.now()+15000;
  while(!line.includes("\n")){const result=await Promise.race([reader.read(),Bun.sleep(Math.max(1,deadline-Date.now())).then(()=>{throw new Error("Daemon readiness timeout");})]);if(result.done)throw new Error("Daemon exited before readiness");line+=new TextDecoder().decode(result.value);}
  const ready=JSON.parse(line.split("\n")[0]!);if(ready.nonce!==nonce)throw new Error("Wrong readiness nonce");
  client=new DaemonConnection({endpoint:`ws://${ready.address}/session`,authToken:nonce});
  client.subscribe(event=>{const entry=variant(event.message);if(event.family!=="Pty"||!entry||!isRecord(entry[1])||entry[1].session_id!==sessionId)return;const [tag,body]=entry;if(tag==="PtyOutput"&&Array.isArray(body.bytes))output+=new TextDecoder().decode(Uint8Array.from(body.bytes));if(tag==="SessionCwd")cwd=body.cwd;if((tag==="TerminalDirectoryChanged"||tag==="TerminalDirectoryChangeFailed")&&body.request_id==="grid-proof-cd")changed=body;});
  stage="connection";client.start();await until(()=>client!.state.phase==="connected","connection");
  const request=(family:Parameters<DaemonConnection["request"]>[0],message:unknown)=>Effect.runPromise(client!.request(family,message as never));
  const tree=()=>request("Workspace","RequestHostWorkspaceTree").then(reply=>Schema.decodeUnknownSync(TreeSchema)(expectVariant(reply,"HostWorkspaceTree")));
  const host=(await tree()).workspaces.find(entry=>entry.root_dir===workspace);if(!host)throw new Error("Missing workspace");
  await request("Workspace",{SubscribeWorkspace:{workspace_id:host.id}});stage="spawn";
  const created=expectVariant(await request("Pty",{CreatePty:{cwd:workspace,cols:120,rows:24,shell,presentation:"grid"}}),"PtyCreated");
  if(!isRecord(created)||created.presentation!=="grid"||typeof created.session_id!=="string")throw new Error("Grid mode not acknowledged");sessionId=created.session_id;
  await request("Workspace",{BindEditorSurface:{surface_id:`terminal:${sessionId}`,session_id:sessionId,path:null}});
  await until(()=>output.includes("NEOISM_GRID>>> "),"configured prompt");
  if(!output.includes("PROFILE_LOADED")||!output.includes("\x1b]777;neoism-control;")||!output.includes("\x1b]7;file://"))throw new Error("Missing actual profile/control/cwd output");
  if(shell.endsWith("zsh")&&!output.includes("PROFILE_LOADED:loaded"))throw new Error("ZDOTDIR/.zshenv not loaded");
  if((await tree()).workspaces.find(entry=>entry.id===host.id)?.main_session_id!==sessionId)throw new Error("Root terminal not durably registered");
  stage="busy directory change";output="";
  const input=(command:string)=>request("Pty",{PtyInput:{session_id:sessionId,bytes:[...new TextEncoder().encode(command+"\r")]}});
  await input("stty -echo; printf 'ECHO_%s\\n' DISABLED");await until(()=>output.includes("ECHO_DISABLED")&&output.includes("NEOISM_GRID>>> "),"echo disabled");output="";
  await input("printf 'BUSY_%s\\n' READY; if read -t 0.5 answer; then printf 'BAD_INPUT=%s\\n' \"$answer\"; else printf 'NO_%s\\n' INPUT; fi");
  await until(()=>output.includes("BUSY_READY"),"busy read");
  await request("Pty",{ChangeTerminalDirectory:{session_id:sessionId,directory:target,request_id:"grid-proof-cd"}});
  await until(()=>changed!==undefined,"directory confirmation");
  if(!isRecord(changed)||changed.directory!==target||!output.includes("NO_INPUT")||/(?:^|[\r\n])BAD_INPUT=/.test(output))throw new Error(`Unsafe directory control: ${JSON.stringify(changed)} ${output}`);
  await until(()=>cwd===target,"observed cwd");
  if((await tree()).workspaces.find(entry=>entry.id===host.id)?.root_dir!==workspace)throw new Error("Shell cwd changed shared host root");
  stage="retained shell";output="";await input("printf 'LIVE_PID_%s\\n' \"$$\"; printf 'DONE_%s\\n' PID");await until(()=>/LIVE_PID_\d+/.test(output)&&output.includes("DONE_PID"),"shell PID");const pid=output.match(/LIVE_PID_\d+/)![0];
  client.stop();client.start();await until(()=>client!.state.phase==="connected","reconnect");
  const attached=expectVariant(await request("Pty",{AttachPty:{session_id:sessionId,cursor:null}}),"PtyCreated");if(!isRecord(attached)||attached.presentation!=="grid")throw new Error("Attach lost Grid metadata");
  output="";await input("printf 'LIVE_PID_%s\\n' \"$$\"; printf 'DONE_%s\\n' PID");await until(()=>output.includes(pid)&&output.includes("DONE_PID"),"same shell PID");
  if(existsSync(join(workspace,"INJECTED")))throw new Error("Directory injection executed");
  console.log(`Isolated ${shell}: configured visible prompt, OSC cwd/lifecycle, durable main binding, busy-safe quoted cd, local-only cwd, same shell after reconnect passed.`);
} catch(error){console.error(`Grid candidate check failed at ${stage}`);console.error((await Bun.file(join(directory,"daemon.log")).text()).slice(-6000));throw error;}
finally {
  if(client&&sessionId&&client.state.phase==="connected")try{await Effect.runPromise(client.request("Pty",{ClosePty:{session_id:sessionId}}));}catch{}
  client?.stop();child.kill("SIGTERM");await child.exited;agent.stop(true);rmSync(directory,{recursive:true,force:true});
}
