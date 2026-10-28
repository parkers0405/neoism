// Full production App against an in-memory daemon, real display clock. Prints
// native draws/s and draw-time percentiles per scenario: code, markdown, agent.
// The window must stay visible: compositors throttle hidden windows' frames.
import { installDisplayClock } from "../../src/animation/display-clock";
import { createRenderer,render } from "@gpuix/solid";
import { startFrameLoop } from "@gpuix/native/runtime";
import { App as AutomationApp,InProcessBackend,liveRendererAsTest } from "@gpuix/native/automation";
import { readFileSync } from "node:fs";
import { createAgentDaemon } from "./agent-daemon";
import { DaemonConnection } from "../../src/daemon/client";
import { WorkspaceModel } from "../../src/model";
import { App } from "../../src/ui/App";
import { loadConfig } from "../../src/config";
import { installFrameMeter,notifyFrameActivity } from "../../src/chrome/frame-meter";
const scenarios=process.argv.slice(2).length?process.argv.slice(2):["code","markdown","agent"];
const server=createAgentDaemon({deferCreations:false});
const md=Array.from({length:120},(_,i)=>`## Section ${i}\n\nSome **bold** text and \`inline code\` with a [link](https://example.com) in paragraph ${i}. Lorem ipsum dolor sit amet, consectetur adipiscing elit, sed do eiusmod tempor.\n\n- item one ${i}\n- item two\n\n\`\`\`rust\nfn main() { println!("{}", ${i}); }\n\`\`\`\n`).join("\n");
server.files.set("notes.md",md);
server.files.set("code.rs",Array.from({length:3000},(_,i)=>`pub fn function_${i}(value: usize) -> String { format!("sample item ${i}: {}", value) }`).join("\n"));
const config={...loadConfig(["--config","/nonexistent/neoism-scenario-fps"],{}),endpoint:server.endpoint,authToken:undefined,vim:true,cursorBlink:false};
const client=new DaemonConnection(config),model=new WorkspaceModel(client,config);
model.start();client.start();
const renderer=createRenderer(event=>notifyFrameActivity(renderer,event));
renderer.init({title:"Neoism scenario FPS",appId:"ai.neoism.gpuix.scenario-fps",width:1440,height:900,titlebarTransparent:true});
for(const name of ["CascadiaCode/CascadiaCodeNF-Regular.otf","PressStart2P/PressStart2P-Regular.ttf"])renderer.registerFont(readFileSync(new URL(`../../../../sugarloaf/src/font/resources/${name}`,import.meta.url)));
installFrameMeter(renderer);
const displayClock=installDisplayClock(renderer,console.error);
const loop=startFrameLoop(renderer);
const root=render(()=><App model={model} config={config}/>,{renderer,keyboardFocusDim:false,onUncaughtError:error=>console.error("RENDER ERROR",error)});
const app=new AutomationApp(new InProcessBackend(liveRendererAsTest(renderer)));
const frames=()=>(renderer as any).getRenderedFrameCount();
async function measure(label:string,drive:()=>void,ms=3000,period=16){
 if(process.env.SHOT){console.log("READY",label);await Bun.sleep(2500);}
 const timer=setInterval(drive,period);await Bun.sleep(400);
 renderer.resetDebugFrameOverlayStats();await Bun.sleep(50);
 const f0=frames(),d0=renderer.getDebugFrameOverlayStats().frames,t0=performance.now();await Bun.sleep(ms);
 const stats=renderer.getDebugFrameOverlayStats(),fps=(frames()-f0)*1000/(performance.now()-t0);clearInterval(timer);
 const tree=JSON.parse(renderer.getAutomationTree());let nodes=0;const walk=(x:any)=>{if(!x||typeof x!=="object")return;if(Array.isArray(x)){x.forEach(walk);return;}if(x.type)nodes++;for(const v of Object.values(x))if(typeof v==="object")walk(v);};walk(tree);
console.log(JSON.stringify({label,fps:+fps.toFixed(1),drawFps:+((stats.frames-d0)*1000/(performance.now()-t0)).toFixed(1),drawP90:+stats.p90Ms!.toFixed(2),drawP99:+stats.p99Ms!.toFixed(2),drawMax:+stats.maxMs!.toFixed(2),nodes}));
 await Bun.sleep(300);
}
const until=async(test:()=>boolean|Promise<boolean>,what:string)=>{const limit=Date.now()+15000;while(!await test()){if(Date.now()>limit)throw new Error("timeout "+what);await Bun.sleep(20);}};
try{
 await until(()=>!!model.state.activeWorkspace,"workspace");
 for(const scenario of scenarios){
  if(scenario==="markdown"){
   await model.openFile("notes.md");const host=app.getByTestId("rich-markdown-editor");await host.waitFor({timeoutMs:8000});await host.click();
   const b=await host.bounds();let dir=-1,n=0;
   const surface=model.state.surfaces.find(s=>s.id===model.state.activeSurfaceId)!;const mdv=model.getMarkdown(surface,model.getLayout().frame.focusedLeaf||undefined);
   for(const pass of [1,2]){
    n=0;await measure(`markdown-60hz-pass${pass}`,()=>{n++;renderer.simulateScrollWheel(b.x+b.width/2,b.y+b.height/2,0,(Math.floor(n/60)%2?1:-1)*40);});
    n=0;await measure(`markdown-notch-8hz-pass${pass}`,()=>{if(++n%20===0)dir=-dir;mdv.call("scroll",{delta:dir*config.fontSize*3,deltaX:0,x:b.width/2,y:b.height/2});},3000,125);
   }
  }
  if(scenario==="tree"){
   for(let i=0;i<400;i++)server.files.set(`module_${String(i).padStart(3,"0")}.rs`,"// file\n");
   const host=app.getByTestId("file-tree");await host.waitFor({timeoutMs:8000});
   await model.listDirectory("");await Bun.sleep(1200);
   const b=await host.bounds();let dir=-1,n=0;
   if(process.env.PROBE){
    await host.click();await Bun.sleep(300);
    const probe=async(label:string,act:()=>Promise<void>|void)=>{const row=app.getByTestId("file:module_020.rs");const ys:number[]=[];await act();const t0=performance.now();while(performance.now()-t0<450){try{ys.push(Math.round((await row.bounds()).y*10)/10);}catch{ys.push(NaN);}await Bun.sleep(7);}console.log(label,JSON.stringify(ys));};
    await probe("ctrl-d",()=>host.press("ctrl-d"));
    await probe("ctrl-u",()=>host.press("ctrl-u"));
    await probe("notch",()=>renderer.simulateScrollWheel(b.x+b.width/2,b.y+b.height/2,0,-78));
   }
   await measure("tree-scroll",()=>{if(++n%60===0)dir=-dir;renderer.simulateScrollWheel(b.x+b.width/2,b.y+b.height/2,0,dir*40);});
  }
  if(scenario==="code"){
   await model.openFile("code.rs");const host=app.getByTestId("code-editor");await host.waitFor({timeoutMs:8000});await host.click();
   const b=await host.bounds();let dir=-1,n=0;
   await measure("code-scroll",()=>{if(++n%150===0)dir=-dir;renderer.simulateScrollWheel(b.x+b.width/2,b.y+b.height/2,0,dir*40);});
  }
  if(scenario==="agent"){
   model.openThread(server.threads[0] as any);await Bun.sleep(800);
   const prose=(i:number)=>`Here is message ${i}. It explains **something** with \`code\` and a list:\n\n- first point ${i}\n- second point\n\n\`\`\`ts\nconst value${i} = compute(${i});\nconsole.log(value${i});\n\`\`\`\n\nAnd a closing paragraph with more words to wrap across lines in the conversation column.`;
   server.push({HistoryChunk:{session_id:"one",messages:Array.from({length:120},(_,i)=>i%4===0?{id:`user-${i}`,role:"User",kind:"User",author:"Parker",text:`Question ${i}: can you check this?`}:i%4===3?{id:`tool-${i}`,role:"Assistant",kind:"Tool",tool:"bash",status:"completed",title:`Run tests ${i}`,detail:"ok"}:{id:`a-${i}`,role:"Assistant",kind:"Assistant",text:prose(i)})}});
   await until(async()=>await app.getByTestId("conversation").count()>0,"conversation");await Bun.sleep(1500);
   if(process.env.SIDEBAR){await app.getByTestId("conversation").press("alt-c");await Bun.sleep(800);}
   const host=app.getByTestId("conversation");const b=await host.bounds();let dir=1,n=0;
   await measure("agent-scroll",()=>{if(++n%120===0)dir=-dir;renderer.simulateScrollWheel(b.x+b.width/2,b.y+b.height/2,0,dir*40);},3000);
   await host.press("end");await Bun.sleep(500);let text="Streaming answer:";let k=0;
   await measure("agent-stream",()=>{text+=` token${k++}`;if(k%25===0)text+="\n\n";server.push({MessageUpdated:{session_id:"one",message:{id:"stream",role:"Assistant",kind:"Assistant",text,status:"running"}}});},3000,16);
  }
 }
}catch(error){console.error("SCENARIO ERROR",error instanceof Error?error.stack:error);}finally{root.unmount();model.dispose();client.stop();server.stop();loop.stop();displayClock.dispose();}
process.exit(0);
