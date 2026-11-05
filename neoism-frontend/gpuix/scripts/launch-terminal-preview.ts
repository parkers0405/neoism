import {launchNative} from "../tests/support/native-launch";
import {mkdir} from "node:fs/promises";
import {resolve} from "node:path";
import {fileURLToPath} from "node:url";
import {execFileSync} from "node:child_process";

const cwd=fileURLToPath(new URL("..",import.meta.url));
const output=resolve(process.env.NEOISM_TERMINAL_PREVIEW_OUTPUT||"/tmp/neoism-terminal-preview-clean");
await mkdir(output,{recursive:true});
const token=crypto.randomUUID();
let ipcEndpoint="", stderr="";
const app=await launchNative({command:process.execPath,args:["scripts/preview-block-terminal.tsx"],cwd,env:{...process.env,
  NEOISM_GPUIX_CORE:process.env.NEOISM_GPUIX_CORE||resolve(cwd,"native/target/debug/libneoism_gpuix_core.so"),NEOISM_TERMINAL_FIXTURE_TOKEN:token,GPUIX_BACKGROUND:"1"},onStderr(chunk){
    process.stderr.write(chunk);stderr+=chunk;
    const match=stderr.match(/\[terminal-fixture-ipc\](http:\/\/127\.0\.0\.1:\d+\/)/);if(match)ipcEndpoint=match[1];
  }});
const deadline=Date.now()+30000;
while(!ipcEndpoint){if(Date.now()>deadline){await app.close();throw new Error("Fixture IPC did not become ready");}await Bun.sleep(50);}
const fixture=async(path:string,body?:unknown)=>{
  const response=await fetch(ipcEndpoint+path,{method:body===undefined?"GET":"POST",headers:{authorization:token,"Content-Type":"application/json"},body:body===undefined?undefined:JSON.stringify(body)});
  if(!response.ok)throw new Error(`Fixture ${path}: ${response.status}`);return await response.json();
};
console.error("[terminal-preview] waiting for native composer");
await app.getByTestId("terminal-command").waitFor({timeoutMs:30000});
await Bun.sleep(2200);
let nativeCaptureError:string|undefined;
const capture=async(name:string)=>{
  const path=resolve(output,name+".png");await Bun.sleep(250);
  try{await app.screenshot({path});}
  catch(error){
    nativeCaptureError=String(error);
    // Some installed display addons omit test-support capture. Capture the
    // actual compositor window instead; never synthesize a renderer image.
    const windows=JSON.parse(execFileSync("hyprctl",["clients","-j"],{encoding:"utf8"}));
    const window=windows.find((item:any)=>item.class==="ai.neoism.terminal-showcase");
    if(!window)throw error;
    execFileSync("grim",["-g",`${window.at[0]},${window.at[1]} ${window.size[0]}x${window.size[1]}`,path]);
  }
  return path;
};
const screenshots:string[]=[];
if(process.argv.includes("--final-hover")){
  try{
    await fixture("mode",{mode:"bottom"});await Bun.sleep(300);
    screenshots.push(await capture("bottom"));
    const history=await app.getByTestId("terminal-block-history").bounds();
    // The final, empty-output grep block is nearest the composer.
    await app.mouse.move({x:history.x+Math.min(200,history.width/2),y:history.y+history.height*.94});
    await app.getByTestId("block-attach").waitFor({timeoutMs:3000});
    screenshots.push(await capture("hover"));
    console.log(JSON.stringify({screenshots,nativeCaptureError,closed:true}));
  }finally{await app.close();}
  process.exit(0);
}
for(const mode of ["bottom","top","flow"]){
  console.error(`[terminal-preview] capturing ${mode}`);
  await fixture("mode",{mode});screenshots.push(await capture(mode));
}
await fixture("mode",{mode:"bottom"});
await Bun.sleep(300);
const hoverBlock=async()=>{
  // Other development windows may resize this tile; always measure live.
  const history=await app.getByTestId("terminal-block-history").bounds();
  for(const fraction of [.7,.4,.2,.9]){
    await app.mouse.move({x:history.x+Math.min(200,history.width/2),y:history.y+history.height*fraction});
    await Bun.sleep(300);
    if(await app.getByTestId("block-attach").count()===1)return;
  }
  throw new Error("No stable unselected toolbar after native hover sweep");
};
const leaveToolbar=async()=>{const composer=await app.getByTestId("terminal-command").bounds();await app.mouse.move({x:composer.x+10,y:composer.y+5});await Bun.sleep(300);};
await hoverBlock();
await app.getByTestId("block-attach").waitFor({timeoutMs:3000});
screenshots.push(await capture("hover"));
const captureErrors:string[]=[];
const checks:Record<string,boolean>={};
try{
const action=await app.getByTestId("block-attach").bounds();
await app.mouse.move({x:action.x+action.width/2,y:action.y+action.height/2});
await Bun.sleep(550);
screenshots.push(await capture("tooltip"));
checks.unselectedToolbarEntry=await app.getByTestId("block-attach").count()===1;
await leaveToolbar();
checks.unselectedToolbarLeaves=await app.getByTestId("block-attach").count()===0;
screenshots.push(await capture("hover-left"));
await hoverBlock();
await app.getByTestId("block-filter").waitFor({timeoutMs:3000});
await app.getByTestId("block-filter").click();
checks.unselectedFilterOpens=await app.getByTestId("block-filter-query").count()===1;
await app.getByTestId("block-filter-query").fill("Checking");
screenshots.push(await capture("filter"));
await app.getByTestId("block-filter-query").press("escape");
await app.getByTestId("terminal-block-history").press("escape");
await hoverBlock();
await app.getByTestId("block-more").click();
checks.unselectedMoreOpens=await app.getByTestId("block-action-menu").count()===1;
checks.moreContainsCopyAndReuse=await app.getByTestId("block-copy-output").count()===1&&await app.getByTestId("block-reuse").count()===1;
screenshots.push(await capture("more"));
// A clamped menu can cover its trigger in a short tile; Escape closes the
// actual menu without accidentally clicking an overlapping menu action.
await app.getByTestId("terminal-block-history").press("escape");
await app.getByTestId("terminal-block-history").press("escape");
await hoverBlock();
const more=await app.getByTestId("block-more").bounds();
await app.mouse.move({x:more.x+more.width/2,y:more.y+more.height/2});await Bun.sleep(150);
await leaveToolbar();
checks.toolbarNotStuckAfterMenus=await app.getByTestId("block-more").count()===0;
await hoverBlock();
await app.getByTestId("block-attach").click();
let localState=await fixture("status") as {attachedBlocks:unknown[];savedCommands:string[]};
checks.attachRecordedLocally=localState.attachedBlocks.length===1;
await hoverBlock();
await app.getByTestId("block-save").click();
localState=await fixture("status") as {attachedBlocks:unknown[];savedCommands:string[]};
checks.saveRecordedLocally=localState.savedCommands.length===1;
screenshots.push(await capture("saved-command"));
await app.getByTestId("terminal-block-history").press("escape");
for(const [name,passed] of Object.entries(checks))if(!passed)captureErrors.push(`verification failed: ${name}`);
}catch(error){captureErrors.push(`toolbar scenarios: ${error}`);console.error(captureErrors.at(-1));}
try{
await app.getByTestId("terminal-command").click();
await app.getByTestId("terminal-command").fill("git st");
await app.getByTestId("terminal-ghost-text").waitFor({timeoutMs:3000});
screenshots.push(await capture("ghost"));
await app.getByTestId("terminal-command").fill("git ");
await app.getByTestId("terminal-command").press("tab");
await app.getByTestId("terminal-suggestions").waitFor({timeoutMs:3000});
screenshots.push(await capture("tab-menu"));
await app.getByTestId("terminal-command").press("escape");
await app.getByTestId("terminal-command").fill("");
}catch(error){captureErrors.push(`completion scenarios: ${error}`);console.error(captureErrors.at(-1));}
await fixture("run",{});
await Bun.sleep(1000);
screenshots.push(await capture("running"));
while((await fixture("status") as {running:boolean}).running)await Bun.sleep(100);
screenshots.push(await capture("running-completed"));
await app.getByTestId("terminal-command").fill("git st");
screenshots.push(await capture("input-after-run"));
await app.getByTestId("terminal-command").fill("");
// Local-only controller keeps this one preview alive and forwards the native
// automation API. It never attaches to an existing app or daemon.
const server=Bun.serve({hostname:"127.0.0.1",port:Number(process.env.NEOISM_TERMINAL_PREVIEW_PORT||0),async fetch(request){
  try{
    const url=new URL(request.url);
    if(request.method==="GET")return Response.json({preview:true,pid:process.pid,screenshots,checks,captureErrors,fixtureState:await fixture("status"),output,nativeCaptureError,help:"POST /call {method,params}; POST /mode {mode}; POST /capture {name}"});
    const body=await request.json() as any;
    if(url.pathname==="/mode"){
      if(!["top","bottom","flow"].includes(body.mode))return new Response("Invalid mode",{status:400});
      await fixture("mode",{mode:body.mode});return Response.json({mode:body.mode});
    }
    if(url.pathname==="/capture")return Response.json({path:await capture(String(body.name||"manual").replace(/[^a-z0-9_-]/gi,"_"))});
    if(url.pathname==="/run")return Response.json(await fixture("run",{}));
    if(url.pathname==="/call")return Response.json(await (app as any).call(body.method,body.params||{}));
    return new Response("Not found",{status:404});
  }catch(error){return Response.json({error:String(error)},{status:500});}
}});
console.log(JSON.stringify({endpoint:server.url.toString(),screenshots,output,nativeCaptureError,pid:process.pid}));
process.on("SIGTERM",()=>{server.stop();void app.close().finally(()=>process.exit(0));});