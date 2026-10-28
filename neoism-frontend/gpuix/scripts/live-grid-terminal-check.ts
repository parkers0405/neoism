import {Effect} from "effect";
import {readFileSync,writeFileSync,chmodSync,mkdtempSync,mkdirSync,rmSync,existsSync} from "node:fs";
import {join} from "node:path";
import {tmpdir} from "node:os";
import {DaemonConnection} from "../src/daemon/client";
import {AgentSdk} from "../src/daemon/sdk";
import {expectVariant,isRecord,variant} from "../src/daemon/protocol";
// After an explicitly approved cutover, read restored metadata and create only
// this check's own Grid shell. Existing shell IDs never receive input/close.
const folder=process.argv[2]!;const env=JSON.parse(readFileSync(join(folder,"new-daemon-env.json"),"utf8"));
const config={endpoint:"ws://127.0.0.1:7878/session",authToken:env.NEOISM_DAEMON_TOKEN};
const client=new DaemonConnection(config),sdk=new AgentSdk(config),before=JSON.parse(readFileSync(join(folder,"live-tree.json"),"utf8"));
const temp=mkdtempSync(join(tmpdir(),"neoism-live-grid-proof-")),target=join(temp,"a 'quoted' ; $(touch INJECTED)");mkdirSync(target);
let id:string|undefined,output="",cwd:unknown,changed:unknown,stage="connection";
const until=async(test:()=>boolean,name:string)=>{const end=Date.now()+15000;while(!test()){if(Date.now()>end)throw new Error(`Timed out at ${name}`);await Bun.sleep(10);}};
const request=(family:Parameters<DaemonConnection["request"]>[0],message:unknown)=>Effect.runPromise(client.request(family,message as never));
const tree=async()=>{const value=expectVariant(await request("Workspace","RequestHostWorkspaceTree"),"HostWorkspaceTree");if(!isRecord(value))throw new Error("Invalid tree");return value as any;};
client.subscribe(event=>{const entry=variant(event.message);if(event.family!=="Pty"||!entry||!isRecord(entry[1])||entry[1].session_id!==id)return;const [tag,body]=entry;if(tag==="PtyOutput"&&Array.isArray(body.bytes))output+=new TextDecoder().decode(Uint8Array.from(body.bytes));if(tag==="SessionCwd")cwd=body.cwd;if((tag==="TerminalDirectoryChanged"||tag==="TerminalDirectoryChangeFailed")&&body.request_id==="live-grid-proof")changed=body;});
client.start();
try{
 await until(()=>client.state.phase==="connected","connected");const restored=await tree();
 if(restored.workspaces.length!==before.workspaces.length||before.workspaces.some((old:any)=>!restored.workspaces.some((now:any)=>now.id===old.id&&now.root_dir===old.root_dir)))throw new Error("Workspace identities/roots changed in cutover");
 const workspace=restored.workspaces.find((value:any)=>value.root_dir&&existsSync(value.root_dir));if(!workspace)throw new Error("No existing restored root");
 stage="SDK catalog";const agent=sdk.forWorkspace(workspace.id);const [providers,agents]=await Promise.all([agent.catalog.providers.configured(workspace.root_dir),agent.catalog.agents.list(workspace.root_dir)]);
 if(!providers.providers.length||!agents.length)throw new Error("Agent catalogs empty");
 await request("Workspace",{SubscribeWorkspace:{workspace_id:workspace.id}});stage="create default shell";
 const created=expectVariant(await request("Pty",{CreatePty:{cwd:workspace.root_dir,cols:120,rows:28,presentation:"grid"}}),"PtyCreated");if(!isRecord(created)||created.presentation!=="grid"||typeof created.session_id!=="string")throw new Error("Grid not acknowledged");id=created.session_id;
 const prompt=()=>{const marker=/\x1b\]777;neoism-control;[^\x07]*;B\x07/g;let end=0;for(const match of output.matchAll(marker))end=match.index!+match[0].length;return end>0&&output.slice(end).replace(/\x1b\][^\x07]*(?:\x07|\x1b\\)/g,"").replace(/\x1b\[[0-?]*[ -/]*[@-~]/g,"").replace(/[\r\n]/g,"").trim().length>0;};
 await until(prompt,"visible configured shell prompt");
 await request("Workspace",{BindEditorSurface:{surface_id:`grid-proof:${id}`,session_id:id,path:null}});
 if((await tree()).workspaces.find((value:any)=>value.id===workspace.id)?.main_session_id!==id)throw new Error("First Grid shell did not become durable main");
 const input=(command:string)=>request("Pty",{PtyInput:{session_id:id,bytes:[...new TextEncoder().encode(command+"\r")]}});
 stage="actual Zsh";output="";await input("printf 'GRID_SHELL_%s\\n' \"$ZSH_VERSION\"; printf 'GRID_DONE_%s\\n' SHELL");await until(()=>output.includes("GRID_DONE_SHELL"),"Zsh version");if(!/GRID_SHELL_5\./.test(output))throw new Error("Default shell is not configured Zsh");
 stage="safe cwd";changed=undefined;await request("Pty",{ChangeTerminalDirectory:{session_id:id,directory:target,request_id:"live-grid-proof"}});await until(()=>changed!==undefined,"cwd confirmation");if(!isRecord(changed)||changed.directory!==target)throw new Error("Directory confirmation failed");await until(()=>cwd===target,"observed cwd");
 if((await tree()).workspaces.find((value:any)=>value.id===workspace.id)?.root_dir!==workspace.root_dir)throw new Error("PTY cd changed shared host root");if(existsSync(join(workspace.root_dir,"INJECTED")))throw new Error("Quoted directory injection occurred");
 const result={daemon:"updated-persistent",workspaces:restored.workspaces.length,tabs:restored.tabs.length,providers:providers.providers.length,agents:agents.length,configuredShell:"zsh",gridPromptVisible:true,durableMain:true,safeQuotedCwd:true,hostRootUnchanged:true};writeFileSync(join(folder,"live-verification.json"),JSON.stringify(result,null,2));console.log(JSON.stringify(result));
}catch(error){writeFileSync(join(folder,"failed-pty-output.txt"),output);chmodSync(join(folder,"failed-pty-output.txt"),0o600);console.error(`Live Grid check failed at ${stage}`);throw error;}
finally{
 if(id&&client.state.phase==="connected"){
  try{await request("Workspace",{CloseSession:{session_id:id}});}catch{}
  try{await request("Pty",{ClosePty:{session_id:id}});}catch{}
 }
 sdk.dispose();client.stop();rmSync(temp,{recursive:true,force:true});
}
