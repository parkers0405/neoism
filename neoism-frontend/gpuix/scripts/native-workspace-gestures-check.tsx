// One isolated native window exercises real mouse targeting and animated tabs.
import assert from "node:assert/strict";
import {createSignal} from "solid-js";
import {createRenderer,render} from "@gpuix/solid";
import {startFrameLoop} from "@gpuix/native/runtime";
import {App,InProcessBackend,liveRendererAsTest} from "@gpuix/native/automation";
import {WorkspaceTabs} from "../src/ui/WorkspaceTabs";
import {loadConfig} from "../src/config";
import {prepareBundledFonts} from "../src/bundled-fonts";
import {installDisplayClock} from "../src/animation/display-clock";
const config={...loadConfig(["--config","/nonexistent/workspace-gestures"],{}),reducedMotion:false};
const initial=["a","b","c"].map(id=>({id,host_id:"fixture",title:"Project",root_dir:"/project"}));
const [workspaces,setWorkspaces]=createSignal(initial),closed:string[]=[];
const renderer=createRenderer();prepareBundledFonts(renderer);
renderer.init({title:"Neoism workspace gesture check",appId:"ai.neoism.gpuix.workspace-gestures",width:900,height:360});
const clock=installDisplayClock(renderer,console.error),loop=startFrameLoop(renderer);
const root=render(()=><div style={{width:"100%",height:"100%",backgroundColor:config.theme.bg}}><WorkspaceTabs config={config} workspaces={workspaces()} activeId="a" onActivate={()=>{}} onNew={()=>{}} onClose={id=>{closed.push(id);setWorkspaces(items=>items.filter(item=>item.id!==id));}} onReorder={(id,target,position)=>setWorkspaces(items=>{const item=items.find(item=>item.id===id)!;const next=items.filter(item=>item.id!==id);next.splice(next.findIndex(item=>item.id===target)+(position==="after"?1:0),0,item);return next;})} registerHost={()=>{}} registerNewHost={()=>{}} onBounds={()=>{}} onScroll={()=>{}}/></div>,{renderer});
const app=new App(new InProcessBackend(liveRendererAsTest(renderer)));
const until=async(check:()=>boolean|Promise<boolean>)=>{const end=Date.now()+5000;while(!await check()){if(Date.now()>end)throw new Error("Native workspace gesture did not settle");await Bun.sleep(8);}};
let failed=false;
try{
 await app.getByTestId("workspace-close:b").waitFor();
 const close=await app.getByTestId("workspace-close:b").bounds(),point={x:close.x+close.width/2,y:close.y+close.height/2};
 await app.mouse.down(point);await until(()=>closed.length===1);await Bun.sleep(80);
 // C now occupies B's former slot before the same physical mouse release.
 const following=await app.getByTestId("workspace-close:c").bounds();assert.ok(Math.abs(following.x-close.x)<2);
 await app.mouse.up(point);await Bun.sleep(80);assert.deepEqual(closed,["b"]);assert.deepEqual(workspaces().map(item=>item.id),["a","c"]);
 setWorkspaces(initial);await app.getByTestId("workspace:b").waitFor();await Bun.sleep(100);
 const original=new Map(await Promise.all(initial.map(async item=>[item.id,(await app.getByTestId(`workspace:${item.id}`).element()).id] as const)));
 const source=await app.getByTestId("workspace:a").bounds(),target=await app.getByTestId("workspace:c").bounds();
 const start={x:source.x+source.width/2,y:source.y+source.height/2},end={x:target.x+target.width*.75,y:target.y+target.height/2};
 await app.mouse.down(start);await app.mouse.move({x:start.x+12,y:start.y},{pressedButton:0});await app.getByTestId("workspace-drag-layer").waitFor();await app.mouse.move(end,{pressedButton:0});
 await until(()=>workspaces().map(item=>item.id).join(",")==="b,c,a");
 const ghost=await app.getByTestId("workspace-drag-preview").bounds();
 assert.ok(Math.abs(ghost.x+source.width/2-end.x)<2);assert.ok(Math.abs(ghost.y+ghost.height/2-end.y)<2);
 for(const item of initial)assert.equal((await app.getByTestId(`workspace:${item.id}`).element()).id,original.get(item.id));
 await app.mouse.up(end);await until(async()=>await app.getByTestId("workspace-drag-preview").count()===0);assert.deepEqual(closed,["b"]);
 console.log("Native workspace gestures passed: one close across pointer retargeting; reorder while held; pointer-centered preview; retained tab hosts.");
}catch(error){failed=true;console.error(error);}
finally{root.unmount();loop.stop();clock.dispose();}
process.exit(failed?1:0);
