import {createRenderer, render} from "@gpuix/solid";
import {enableAutomation, startFrameLoop} from "@gpuix/native/runtime";
import {createSignal, Show} from "solid-js";
import {Effect} from "effect";
import {loadConfig} from "../src/config";
import {prepareBundledFonts} from "../src/bundled-fonts";
import {TerminalView} from "../src/terminal/TerminalView";
import {terminalSession, disposeTerminalSessions} from "../src/terminal/sessions";
import type {DaemonConnection} from "../src/daemon/client";

// Isolated transport fixture: no shell, real daemon, session store, or user
// configuration. All VT parsing/capture and UI come from production modules.
const config = {...loadConfig([], {}, {terminal:{"block-ui":true}}), reducedMotion:false};
const listeners = new Set<(event:any)=>void>();
let offset=0, requestId=0, pendingInput="", running=false;
const osc=(value:string)=>`\x1b]${value}\x07`;
const prompt=osc("133;A")+"$ "+osc("133;B");
async function output(text:string){
  const bytes=[...new TextEncoder().encode(text)], start=offset; offset+=bytes.length;
  for(const listener of listeners)listener({family:"Pty",message:{PtyOutput:{session_id:"terminal-showcase",bytes,offset:start}}});
  await Bun.sleep(65);
}
const samples=[
  {command:"git status --short --branch", text:"\x1b[1;36m## feature/terminal-ui\x1b[0m\r\n \x1b[33mM\x1b[0m src/terminal/TerminalView.tsx\r\n\x1b[32mA \x1b[0m tests/terminal-layout.test.ts\r\n\x1b[31m??\x1b[0m docs/terminal-notes.md\r\n", status:0},
  {command:"cargo check -p showcase", text:"\x1b[1;32m    Checking\x1b[0m showcase v0.1.0 (/workspace/showcase)\r\n\x1b[1;32m    Checking\x1b[0m terminal-core v0.4.2\r\n\x1b[1;32m    Finished\x1b[0m dev profile [unoptimized + debuginfo] target(s) in 1.24s\r\n", status:0},
  {command:"grep -n TODO docs/terminal-notes.md", text:"", status:1},
];
async function simulate(command:string){
  if(running)return; running=true;
  try{
    await output(command+"\r\n"+osc("133;C"));
    const sample=samples.find(item=>item.command===command);
    if(command==="cargo check --workspace"){
      for(let i=1;i<=12;i++){await output(`\x1b[32m    Checking\x1b[0m workspace-module-${i} v0.1.0\r\n`);await Bun.sleep(350);}
      await output("\x1b[32m    Finished\x1b[0m dev profile in 5.1s\r\n");
    }else if(sample){if(sample.text)await output(sample.text);}
    else await output("Command not available.\r\n");
    await output(osc(`133;D;${sample?.status||0}`)+prompt);
  }finally{running=false;}
}
const client={
  request(_family:string,message:any){
    if(message==="ListBrowserLocations")return Effect.succeed({BrowserLocations:{locations:[{kind:"home",path:"/workspace"}]}});
    if(message?.BrowserListDir)return Effect.succeed({DirListing:{entries:[{name:"src",is_dir:true},{name:"tests",is_dir:true},{name:"README.md",is_dir:false}]}});
    return Effect.succeed({ShellHistory:{entries:samples.map(item=>item.command)}});
  },
  send(family:string,message:any){
    if(family==="Pty"&&message.PtyInput){
      const text=new TextDecoder().decode(Uint8Array.from(message.PtyInput.bytes));
      // Only recognize submitted text. Never hand it to a process or shell.
      pendingInput+=text.replace(/\x1b\[20[01]~/g,"");
      if(pendingInput.includes("\r")){const command=pendingInput.split("\r")[0];pendingInput="";void simulate(command);}
    }
    return ++requestId;
  },
  subscribe(listener:(event:any)=>void){listeners.add(listener);return ()=>listeners.delete(listener);},
  subscribeStatus(){return ()=>{};},
} as unknown as DaemonConnection;
const report=(error:unknown)=>console.error("[terminal-preview]",error);
const renderer=createRenderer();
prepareBundledFonts(renderer);
await (renderer as any).initAsync({title:"Terminal",appId:"ai.neoism.terminal-showcase",width:1120,height:760,focus:true});
if(process.argv.includes("--maximized"))renderer.zoomWindow();
const session=terminalSession(client,"terminal-showcase",config.theme,report,true);
const initial=process.env.NEOISM_TERMINAL_INPUT_POSITION||"bottom";
if(!["bottom","top","flow"].includes(initial))throw new Error("Invalid input position");
const [position,setPosition]=createSignal(initial as "bottom"|"top"|"flow");
const attachedBlocks:Array<{command:string;directory:string;output:string}>=[];
const [savedCommands,setSavedCommands]=createSignal<string[]>([]);
const savedDirectories:Record<string,string>={};
const attachBlock=(block:{command:string;directory:string;output:string})=>{attachedBlocks.push({...block});};
const saveCommand=(command:string,directory:string)=>{
  savedDirectories[command]=directory;
  setSavedCommands(current=>current.includes(command)?current:[...current,command]);
};
render(()=><div style={{width:"100%",height:"100%",display:"flex",flexDirection:"column"}}><Show when={position()} keyed>{mode=><TerminalView config={{...config,terminalInputPosition:mode}} sessionId={session.id} client={client} active onSearch={()=>{}} report={report} onAttachBlock={attachBlock} onSaveCommand={saveCommand} savedCommands={savedCommands()}/>}</Show></div>,{renderer});
(renderer as any).setStartupRenderingReady(true);
enableAutomation(renderer);
startFrameLoop(renderer,{onError:report,onTerminated:()=>{disposeTerminalSessions(client);process.exit(0);}});
await Bun.sleep(350);
await output(osc("7;file:///workspace/showcase")+prompt);
for(const sample of samples){session.submitBlock(sample.command);while(running)await Bun.sleep(50);await Bun.sleep(80);}
// Private control plane only: nothing from this server is laid out in the UI.
const ipc=Bun.serve({hostname:"127.0.0.1",port:0,async fetch(request){
  if(!process.env.NEOISM_TERMINAL_FIXTURE_TOKEN||request.headers.get("authorization")!==process.env.NEOISM_TERMINAL_FIXTURE_TOKEN)return new Response("Forbidden",{status:403});
  const path=new URL(request.url).pathname;
  if(path==="/mode"&&request.method==="POST"){
    const body=await request.json() as {mode:string};
    if(!["top","bottom","flow"].includes(body.mode))return new Response("Invalid mode",{status:400});
    setPosition(body.mode as "bottom"|"top"|"flow");
  }else if(path==="/run"&&request.method==="POST"){
    if(running)return new Response("Busy",{status:409});
    session.submitBlock("cargo check --workspace");
  }else if(path!=="/status")return new Response("Not found",{status:404});
  return Response.json({mode:position(),running,attachedBlocks,savedCommands:savedCommands(),savedDirectories});
}});
console.error("[terminal-fixture-ipc]"+ipc.url.toString());