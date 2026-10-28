import {mkdtempSync,readFileSync,rmSync,writeFileSync} from "node:fs";
import {tmpdir} from "node:os";
import {join} from "node:path";
import type {TreeNode,ElementBounds} from "@gpuix/solid/automation";
import {launchNative} from "../tests/support/native-launch";
import {createAgentDaemon} from "../tests/support/agent-daemon";
import {tabChrome} from "../src/layout/tab-chrome";
const directory=mkdtempSync(join(tmpdir(),"neoism-workspace-tabs-")),config=join(directory,"config.json"),recovery=join(directory,"state.json");
const configuration={appearance:{fonts:{family:"Cascadia Code NF",size:14}},editor:{"vim-mode":true}};
const chrome=tabChrome(1);
writeFileSync(config,JSON.stringify(configuration));
const server=createAgentDaemon({configuration,deferCreations:false});
const packaged=process.argv.includes("--packaged");
const launch=()=>launchNative({command:packaged?join(process.cwd(),"dist/neoism-gpuix"):process.execPath,args:[...(packaged?[]:["src/main.tsx"]),"--daemon",server.endpoint,"--config",config],cwd:process.cwd(),env:{...process.env,NEOISM_GPUIX_STATE_PATH:recovery}});
let app=await launch();
const flatten=(nodes:TreeNode[]):TreeNode[]=>nodes.flatMap(node=>[node,...flatten(node.children||[])]);
const nodes=async()=>{const {tree}=await app.call("getTree",{});return flatten(tree?[tree]:[]);};
const tabs=async()=>(await nodes()).filter(node=>/^workspace:/.test(node.testId||""));
const plus=()=>app.getByTestId("workspace-new"),strip=()=>app.getByTestId("workspace-tabs"),scroll=()=>app.getByTestId("workspace-tab-scroll");
const press=(keys:string)=>app.call("keystrokes",{keys});
const creates=()=>server.workspaceRequests.filter(request=>request.tag==="CreateHostWorkspace");
const until=async(predicate:()=>boolean|Promise<boolean>)=>{const end=Date.now()+15000;while(!await predicate()){if(Date.now()>end)throw new Error("Workspace tab state did not converge");await Bun.sleep(15);}};
const near=(a:number,b:number)=>Math.abs(a-b)<1.5;
const fixed=(a:ElementBounds,b:ElementBounds)=>near(a.x,b.x)&&near(a.y,b.y)&&near(a.width,b.width)&&near(a.height,b.height);
const evidence:Array<Record<string,unknown>>=[];
const saved=()=>JSON.parse(readFileSync(recovery,"utf8"));
const ids=async()=>(await tabs()).map(node=>node.testId!.slice("workspace:".length));
const assertNoPty=()=>{if(server.ptyRequests.some(request=>request.tag==="CreatePty"))throw new Error("Workspace + spawned a PTY");};
async function assertInline(short=false){
 const all=await tabs(),last=all.at(-1);if(!last?.testId)throw new Error("Missing last workspace tab");
 const [lastBox,button,bar]=await Promise.all([app.getByTestId(last.testId).bounds(),plus().bounds(),strip().bounds()]);
 if(!near(button.x,lastBox.x+lastBox.width)||!near(button.y,lastBox.y)||button.width<10)throw new Error(`Workspace + is not inline after last tab ${JSON.stringify({lastBox,button,bar})}`);
 if(button.x<bar.x-1||button.x+button.width>bar.x+bar.width+1)throw new Error(`Workspace + is outside the visible scroll area ${JSON.stringify({button,bar})}`);
 const id=last.testId.slice("workspace:".length),title=await app.getByTestId(`workspace-title:${id}`).bounds(),icon=await app.getByTestId(`workspace-icon:${id}`).bounds();
 const expectedPadding=2*chrome.workspace.tabPaddingX+18+chrome.buffer.closeHitSize;
 if(Math.abs(lastBox.width-title.width-icon.width-expectedPadding)>2)throw new Error(`Workspace tab does not fit its title, icon, close hitbox, and padding ${JSON.stringify({lastBox,title,icon,expectedPadding})}`);
 if(short&&button.x+button.width>=bar.x+bar.width-20)throw new Error("Short workspace tabs stretch across the row");
 evidence.push({stage:short?"title-sized-inline":"overflow-inline",last: last.testId,lastBox,button,bar});
}
async function dragWorkspace(sourceId:string,targetId:string,position:"before"|"after"){
 const before=await ids();await until(()=>{try{return JSON.stringify(saved().ui.openWorkspaceIds)===JSON.stringify(before);}catch{return false;}});
 const active=saved().activeWorkspaceId,requests=server.workspaceRequests.length,hostIds=new Map((await tabs()).map(node=>[node.testId,node.id]));
 const source=await app.getByTestId(`workspace:${sourceId}`).bounds(),target=await app.getByTestId(`workspace:${targetId}`).bounds();
 const start={x:source.x+source.width/2,y:source.y+source.height/2},end={x:target.x+target.width*(position==="before"?.25:.75),y:target.y+target.height/2};
 await app.mouse.down(start);await app.mouse.move({x:start.x+(end.x>=start.x?8:-8),y:start.y},{pressedButton:0});
 await app.getByTestId("workspace-drag-layer").waitFor();await app.mouse.move(end,{pressedButton:0});await app.mouse.up(end);
 const expected=before.filter(id=>id!==sourceId);expected.splice(expected.indexOf(targetId)+(position==="after"?1:0),0,sourceId);
 await until(async()=>JSON.stringify(await ids())===JSON.stringify(expected));await until(()=>JSON.stringify(saved().ui.openWorkspaceIds)===JSON.stringify(expected));
 if(saved().activeWorkspaceId!==active)throw new Error("Dragging workspace tabs activated a different workspace");
 if(server.workspaceRequests.length!==requests)throw new Error("Workspace drag sent a daemon request");
 for(const node of await tabs())if(hostIds.get(node.testId)!==node.id)throw new Error("Workspace reorder remounted a retained tab host");
 if(await app.getByTestId("workspace-drag-layer").count())throw new Error("Workspace drag layer survived release");
 evidence.push({stage:"reorder",sourceId,targetId,position,before,after:expected,active});
}
async function focusPlus(){
 // Use the production focus ladder. Its real HostElement.scrollIntoView
 // reveals the trailing + even when it begins outside the painted viewport.
 const welcome=await app.getByTestId("welcome-page").element();
 await app.call("focus",{elementId:welcome.id});await Bun.sleep(60);
 await press("alt-up");await press("alt-up");
 await app.getByTestId("chrome-focus-cursor:workspaces").waitFor();await press("right");
 await until(async()=>{try{const button=await plus().bounds(),bar=await strip().bounds();return button.x>=bar.x-1&&button.x+button.width<=bar.x+bar.width+1;}catch{return false;}});
}
async function expectCreated(before:number){
 await until(()=>creates().length===before+1);await app.getByTestId("welcome-page").waitFor();
 await until(async()=>(await tabs()).length===before+2);
 const created=server.workspaces.at(-1)!;await app.getByTestId(`workspace:${created.id}`).waitFor();
 await until(()=>server.workspaceRequests.filter(request=>request.tag==="SubscribeWorkspace").at(-1)?.body.workspace_id===created.id);assertNoPty();return created;
}
let stage="fresh title-sized A tab with adjacent workspace +";
try{
 await app.getByTestId("welcome-page").waitFor({timeoutMs:20000});await plus().waitFor();await until(async()=>(await tabs()).length===1);
 await assertInline(true);if(server.workspaces.length!==2)throw new Error("Fixture catalog A+B was not available independently of this window");
 stage="click + creates backend workspace and its independent Welcome";
 const before=creates().length;await plus().click();const first=await expectCreated(before);await assertInline(true);
 if(creates()[0]?.body.host_id!=="host")throw new Error("Workspace creation did not target the active host");
 stage="horizontal drag reorders inactive and active tabs without activating or recreating them";
 await dragWorkspace("a",first.id,"after");await dragWorkspace(first.id,"a","after");
 // Add enough title-sized tabs to overflow the actual native viewport.
 for(let i=1;i<40;i++){
  const current=await tabs(),bar=await strip().bounds(),button=await plus().bounds();
  if(i>=9&&current.reduce((sum,tab)=>sum+(tab.bounds?.width||0),0)+button.width>bar.width+120)break;
  stage=`overflow creation ${i+1}`;await focusPlus();await assertInline();const count=creates().length;await plus().click();await expectCreated(count);
 }
 stage="workspace + remains inline inside the same horizontal scrolling strip";
 await focusPlus();await assertInline();const barBefore=await strip().bounds(),scrollNode=await scroll().element();
 const contentOffset=await app.call("getScrollOffset",{elementId:scrollNode.id});if(!contentOffset.offset||Math.abs(contentOffset.offset[0])<20)throw new Error("Title-sized workspace tabs never overflowed horizontally");
 stage="precise horizontal wheel moves tabs without moving the bar";
 // Negative pixels pan right; first return left so there is room to scroll.
 await app.call("scrollTo",{elementId:scrollNode.id,x:0,y:0});
 const firstTab=app.getByTestId("workspace:a");await until(async()=>{try{return (await firstTab.bounds()).x>=barBefore.x-1;}catch{return false;}});
 const visible=(await tabs()).filter(node=>node.bounds&&node.bounds.x>=barBefore.x+100&&node.bounds.x+node.bounds.width<=barBefore.x+barBefore.width+1);
 const probe=visible[0];if(!probe?.testId)throw new Error("No visible middle workspace tab for horizontal wheel probe");
 const tracked=app.getByTestId(probe.testId),tabBefore=await tracked.bounds();await scroll().wheel(-100,0);
 await until(async()=>{try{return (await tracked.bounds()).x<tabBefore.x-15;}catch{return false;}});
 const tabAfter=await tracked.bounds(),afterOffset=await app.call("getScrollOffset",{elementId:scrollNode.id}),barAfter=await strip().bounds();
 if(!fixed(barBefore,barAfter))throw new Error(`Scrolling moved/resized the workspace bar ${JSON.stringify({barBefore,barAfter})}`);
 evidence.push({stage:"trackpad",probe:probe.testId,before:tabBefore,after:tabAfter,afterOffset:afterOffset.offset,barAfter});
 stage="drag uses painted tab bounds after precise horizontal scrolling";
 const scrolled=(await tabs()).filter(node=>node.bounds&&node.bounds.x>=barAfter.x+2&&node.bounds.x+node.bounds.width<=barAfter.x+barAfter.width-2);
 if(scrolled.length<2)throw new Error("Need two visible scrolled tabs for drag proof");
 await dragWorkspace(scrolled[0]!.testId!.slice(10),scrolled[1]!.testId!.slice(10),"after");
 stage="keyboard arrows reveal an offscreen workspace and the trailing +";
 await focusPlus();const beforeKeys=creates().length,total=(await tabs()).length;
 for(let i=0;i<total;i++)await press("left");
 await until(async()=>{try{const first=await app.getByTestId(`workspace:${(await ids())[0]}`).bounds(),bar=await strip().bounds();return first.x>=bar.x-1&&first.x+first.width<=bar.x+bar.width+1;}catch{return false;}});
 if(creates().length!==beforeKeys)throw new Error("Moving the workspace cursor created a workspace before Enter");
 await press("left");await until(async()=>{try{await assertInline();return true;}catch{return false;}});await press("enter");const keyboardCreated=await expectCreated(beforeKeys);
 await until(()=>{try{const saved=JSON.parse(readFileSync(recovery,"utf8"));return saved.activeWorkspaceId===keyboardCreated.id&&saved.surfaces.some((surface:any)=>surface.workspaceId===keyboardCreated.id&&surface.kind==="welcome");}catch{return false;}});
 stage="inactive workspace X removes membership without activation or daemon deletion";
 await app.call("scrollTo",{elementId:(await scroll().element()).id,x:0,y:0});const closedId=(await ids())[0]!;
 await until(async()=>(await app.getByTestId(`workspace:${closedId}`).bounds()).x>=barBefore.x-1);
 const catalogCount=server.workspaces.length,subscriptions=server.workspaceRequests.filter(request=>request.tag==="SubscribeWorkspace").length;
 await app.getByTestId(`workspace-close:${closedId}`).click();await until(async()=>!(await ids()).includes(closedId));
 if(saved().activeWorkspaceId!==keyboardCreated.id||server.workspaces.length!==catalogCount||server.workspaceRequests.filter(request=>request.tag==="SubscribeWorkspace").length!==subscriptions)throw new Error("Inactive workspace X activated or deleted a server workspace");
 stage="reordered membership survives a full frontend restart";
 const ordered=await ids();await until(()=>JSON.stringify(saved().ui.openWorkspaceIds)===JSON.stringify(ordered));await app.close();app=await launch();await app.getByTestId("welcome-page").waitFor({timeoutMs:20000});await until(async()=>JSON.stringify(await ids())===JSON.stringify(ordered));
 assertNoPty();
 console.log(JSON.stringify({packaged,titleSizedWorkspaceTabs:true,workspacePlusInline:true,backendWorkspaceCreates:creates().length,welcomeWithoutPty:true,preciseHorizontalTrackpad:true,fixedStripGeometry:true,keyboardRevealsHiddenTabsAndPlus:true,keyboardEnterCreatesWorkspace:true,horizontalReorder:true,reorderAfterScroll:true,reorderPersistsRestart:true,inactiveCloseDoesNotActivate:true,firstCreated:first.id,keyboardCreated:keyboardCreated.id,evidence}));
}catch(error){console.error(`Native workspace tabs failed at ${stage}`);console.error(JSON.stringify({workspace:server.workspaceRequests,pty:server.ptyRequests,evidence,nodes:(await nodes()).filter(node=>node.testId?.startsWith("workspace")||node.testId?.startsWith("chrome-focus-cursor")).map(node=>({id:node.id,testId:node.testId,bounds:node.bounds}))}));throw error;}
finally{await app.close();server.stop();rmSync(directory,{recursive:true,force:true});}
