import assert from "node:assert/strict";
import {createRoot} from "@gpuix/solid";
import {WorkspaceTabs} from "../../src/ui/WorkspaceTabs";
import {PaneLayoutView} from "../../src/layout/PaneLayoutView";
import {WorkspaceLayout} from "../../src/layout/WorkspaceLayout";
import {loadConfig} from "../../src/config";
import {usePresentationScheduler} from "../../src/animation/frame-scheduler";
import {HoverTabTitle,hoverTitleFrame} from "../../src/layout/HoverTabTitle";
import {createRecordingAppRenderer} from "./full-app-recording-renderer";
const config={...loadConfig(["--config","/nonexistent/tab-scroll-title"],{}),reducedMotion:false},host=createRecordingAppRenderer(),root=createRoot(host.renderer),layout=new WorkspaceLayout();
const title="Very long complete title that must move under its own fixed clip without moving any icon or close button";
const workspaces=Array.from({length:5},(_,i)=>({id:String(i),host_id:"host",title:i?`Workspace ${i}`:title,root_dir:i?`/project/${i}`:undefined}));
for(let i=0;i<5;i++)layout.open({id:`file:${i}`,kind:"file",title:i?`file-${i}.ts`:title});layout.layout(300,220);
let now=0;const originalNow=performance.now.bind(performance);performance.now=()=>now;
let next=0;const scheduled=new Map<number,{due:number;callback:()=>void}>(),release=usePresentationScheduler((callback,delay)=>{const id=++next;scheduled.set(id,{due:now+delay,callback});return()=>{scheduled.delete(id);};});
const offsets=new Map<number,number>(),scrolls:Array<{id:number;x:number}>=[];
(host.renderer as any).scrollTo=(id:number,x:number)=>{offsets.set(id,x);scrolls.push({id,x});root.dispatch({elementId:id,eventType:"scrollChange",scrollX:x,scrollY:0});};
(host.renderer as any).getScrollOffset=(id:number)=>[offsets.get(id)||0,0];
const flush=async()=>{await Promise.resolve();root.flush();};
const find=(id:string)=>{const node=host.find(id);assert.ok(node,`Missing ${id}`);return node;};
const dispatch=async(id:string,eventType:string,payload={})=>{root.dispatch({elementId:find(id).id,eventType,...payload});await flush();};
const descendants=(id:number):any[]=>{const node=host.nodes.get(id)!;return[node,...node.children.flatMap(descendants)];};
const advance=async(ms:number)=>{now+=ms;for(let pass=0;pass<10;pass++){const ready=[...scheduled].filter(([,job])=>job.due<=now);if(!ready.length)break;for(const[id,job]of ready){scheduled.delete(id);job.callback();}await flush();}};
try{
 root.render(()=><div><WorkspaceTabs config={config} workspaces={workspaces} activeId="0" onActivate={()=>{}} onNew={()=>{}} onClose={()=>{}} registerHost={()=>{}} registerNewHost={()=>{}} onBounds={()=>{}} onScroll={()=>{}}/><PaneLayoutView controller={layout} config={config} renderSurface={()=> <div testId="unchanged-editor"/>} onClose={()=>{}} onNewWelcome={()=>{}}/></div>);await flush();
 await dispatch("workspace-tab-scroll","layout",{x:0,y:0,width:300,height:28});for(const workspace of workspaces)await dispatch(`workspace:${workspace.id}`,"layout",{width:workspace.id==="0"?300:120,height:28});
 const pane=layout.frame.panes[0],workspaceIds=workspaces.map(workspace=>find(`workspace:${workspace.id}`).id),bufferIds=pane.tabs.map(tab=>find(`pane-tab:${tab.leaf}`).id),body=find("unchanged-editor").id;
 for(const id of["workspace-tab-scroll",`pane-tab-scroll:${pane.leaf}`]){
  const node=find(id);await dispatch(id,"scroll",{deltaX:0,deltaY:-1,precise:false});assert.equal(offsets.get(node.id),-60,"Vertical wheel uses canonical60px notch in delivered direction");
  const calls=scrolls.length;await dispatch(id,"scroll",{deltaX:-20,deltaY:0,precise:true});assert.equal(scrolls.length,calls,"Horizontal gestures retain native scrolling instead of being applied twice");offsets.set(node.id,-80);await dispatch(id,"scrollChange",{scrollX:-80});
  await dispatch(id,"scroll",{deltaX:0,deltaY:-13.5,precise:true});assert.equal(offsets.get(node.id),-93.5);await dispatch(id,"scroll",{deltaX:0,deltaY:10,precise:true});assert.equal(offsets.get(node.id),-83.5,"Natural scrolling sign is never inverted");
  await dispatch(id,"scroll",{deltaX:0,deltaY:-10000,precise:true});const end=offsets.get(node.id)!;assert.ok(end<0&&end>-10000,"Overflow clamps at measured content extent");const clampedCalls=scrolls.length;await dispatch(id,"scroll",{deltaX:0,deltaY:-10000,precise:true});assert.equal(scrolls.length,clampedCalls,"Clamped input requests no redundant redraw");await dispatch(id,"scroll",{deltaX:0,deltaY:10000,precise:true});assert.equal(offsets.get(node.id),0);
 }
 const cases=[{tab:"workspace:0",text:"workspace-title:0",close:"workspace-close:0"},{tab:`pane-tab:${pane.tabs[0].leaf}`,text:undefined,close:`pane-close:${pane.tabs[0].leaf}`}];
 for(const entry of cases){
  const tab=find(entry.tab),close=find(entry.close),closeBefore=JSON.stringify(close.style);const text=entry.text?find(entry.text):descendants(tab.id).find(node=>node.type==="text"&&node.style.textOverflow==="ellipsis"&&node.children.some((id:number)=>host.nodes.get(id)?.text===title));assert.ok(text,"Actual title must use fixed clipped text viewport");
  const parent=[...host.nodes.values()].find(node=>node.children.includes(text.id))!,distance=Number(text.style.width);assert.ok(distance>0);assert.equal(Number(text.style.left),0);
  await dispatch(entry.tab,"mouseEnter");assert.equal(Number(text.style.left),0);assert.ok(scheduled.size>0);await advance(599);assert.equal(Number(text.style.left),0,"Original600ms lead-in remains");await advance(1);const leg=Math.max((Number(text.style.width)-distance)/(35*config.scale),2);await advance(leg*500);assert.ok(Number(text.style.left)<0,"Overflowed hovered title must move through full text");assert.equal(Number(parent.style.width),distance,"Title clipping box stays fixed");assert.equal(JSON.stringify(close.style),closeBefore,"Close icon geometry stays fixed");await dispatch(entry.tab,"mouseLeave");assert.equal(Number(text.style.left),0,"Pointer leave restores truncation immediately");assert.equal(scheduled.size,0,"No idle marquee frames after leave");
 }
 assert.deepEqual(workspaces.map(workspace=>find(`workspace:${workspace.id}`).id),workspaceIds);assert.deepEqual(pane.tabs.map(tab=>find(`pane-tab:${tab.leaf}`).id),bufferIds);assert.equal(find("unchanged-editor").id,body);
 assert.equal(hoverTitleFrame(.599,70,1).offset,0);assert.ok(Math.abs(hoverTitleFrame(.9,70,1).offset)<1e-8);assert.ok(Math.abs(hoverTitleFrame(2.3,70,1).offset-70)<1e-8);assert.ok(hoverTitleFrame(3.6,70,1).offset<70);
 root.render(()=><HoverTabTitle label={title} width={60} naturalWidth={900} height={24} scale={1} hovered={true} reducedMotion={true} style={{fontSize:12}}/>);await flush();assert.equal(scheduled.size,0,"Reduced motion never starts marquee work");root.render(()=><HoverTabTitle label="Fits" width={60} naturalWidth={30} height={24} scale={1} hovered={true} reducedMotion={false} style={{fontSize:12}}/>);await flush();assert.equal(scheduled.size,0,"Unclipped titles never start marquee work");
 console.log(JSON.stringify({noWindow:true,actualBothTabRows:true,verticalWheelAndPrecise:true,horizontalNotDoubled:true,naturalDirection:true,overflowClamped:true,stableTabAndBodyIds:true,fullTitleMarquee:true,canonicalTiming:true,fixedCloseGeometry:true,idleStops:true}));
}finally{root.unmount();layout.dispose();release();performance.now=originalNow;}
