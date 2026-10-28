// Read-only integration diagnostic. No window, recovered buffers, PTYs, prompts,
// sessions, or workspace records are created. Scope must name an existing workspace.
import{createRoot}from"@gpuix/solid";
import{App}from"../../src/ui/App";
import{AppVisualState}from"../../src/ui/AppVisualState";
import{loadConfig}from"../../src/config";
import{DaemonConnection}from"../../src/daemon/client";
import{WorkspaceModel}from"../../src/model";
import{createRecordingAppRenderer}from"./full-app-recording-renderer";
import{usePresentationScheduler}from"../../src/animation/frame-scheduler";
import{installProfiling}from"../../src/profiling";
import{variant,type WireEvent}from"../../src/daemon/protocol";
process.env.NODE_ENV="test";
const config={...loadConfig(),reducedMotion:true,cursorBlink:false,splashAutoAnimate:false};if(!config.workspaceId)throw Error("Pass --workspace with an existing workspace ID");
const client=new DaemonConnection(config),model=new WorkspaceModel(client,config),visuals=new AppVisualState(),host=createRecordingAppRenderer(),root=createRoot(host.renderer),frames=new Set<()=>void>(),restore=usePresentationScheduler(callback=>{frames.add(callback);return()=>frames.delete(callback);});
installProfiling(host.renderer as never);
const costs:Record<string,{count:number;ms:number;maxMs:number}>={};const measure=(key:string,start:number)=>{const ms=performance.now()-start,entry=costs[key]||(costs[key]={count:0,ms:0,maxMs:0});entry.count++;entry.ms+=ms;entry.maxMs=Math.max(entry.maxMs,ms);};
const internal=model as unknown as{receive:(event:WireEvent)=>void;transaction:<T>(work:()=>T)=>T};const receive=internal.receive;internal.receive=event=>{const started=performance.now();try{receive(event);}finally{measure(`receive.${event.family}.${variant(event.message)?.[0]||"unknown"}`,started);}};const transaction=internal.transaction.bind(model);internal.transaction=work=>{const started=performance.now();let workMs=0;try{return transaction(()=>{const before=performance.now();try{return work();}finally{workMs=performance.now()-before;}});}finally{const publish=performance.now()-started-workMs,entry=costs.publication||(costs.publication={count:0,ms:0,maxMs:0});entry.count++;entry.ms+=publish;entry.maxMs=Math.max(entry.maxMs,publish);}};
let publications=0,raw=0,processed=0,closed=0,firstFamilyAt:number|undefined,lastPublish=0,maxPublicationGap=0;const counts:Record<string,number>={};
model.setUI({sidebar:"conversations",openWorkspaceIds:[config.workspaceId]});const off=model.subscribe(()=>{publications++;const now=performance.now();if(lastPublish)maxPublicationGap=Math.max(maxPublicationGap,now-lastPublish);lastPublish=now;});const rawOff=client.subscribe(event=>{if(event.family==="Agent"){raw++;const tag=variant(event.message)?.[0]||"unknown";counts[tag]=(counts[tag]||0)+1;if(tag==="CatalogFamily"&&firstFamilyAt===undefined)firstFamilyAt=performance.now();}});const processedOff=model.subscribeAgentEvents(()=>processed++);let connected=false;const states=client.subscribeStatus(state=>{if(state.phase==="connected")connected=true;else if(connected&&state.phase==="waiting")closed++;});
const started=performance.now();
try{model.start();client.start();const deadline=started+7000;while(!model.state.activeWorkspace&&performance.now()<deadline)await Bun.sleep(5);if(!model.state.activeWorkspace)throw Error("Existing workspace did not resolve");root.render(()=><App model={model} config={config} visuals={visuals}/>);await host.pump(root);const activeFor=performance.now();while(performance.now()-activeFor<3500){const pending=[...frames];frames.clear();for(const frame of pending)frame();await host.pump(root);await Bun.sleep(5);}console.log(JSON.stringify({noWindow:true,actualApp:true,phase:client.state.phase,closed,raw,processed,counts,publications,maxPublicationGap:Math.round(maxPublicationGap),elapsedMs:Math.round(performance.now()-started),familyElapsedMs:firstFamilyAt===undefined?null:Math.round(performance.now()-firstFamilyAt),costs:Object.fromEntries(Object.entries(costs).map(([key,value])=>[key,{count:value.count,ms:Math.round(value.ms),maxMs:Math.round(value.maxMs)}])),nativeNodes:host.nodes.size,error:model.state.error?true:false}));}
finally{root.unmount();restore();rawOff();processedOff();states();off();visuals.dispose();model.dispose();client.stop();}
