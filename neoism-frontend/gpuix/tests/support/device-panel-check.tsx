import assert from "node:assert/strict";
import {createRoot} from "@gpuix/solid";
import {App} from "../../src/ui/App";
import {AppVisualState} from "../../src/ui/AppVisualState";
import {WorkspaceModel} from "../../src/model";
import {DaemonConnection} from "../../src/daemon/client";
import {loadConfig} from "../../src/config";
import {usePresentationScheduler} from "../../src/animation/frame-scheduler";
import {createRecordingAppRenderer} from "./full-app-recording-renderer";
import {createAgentDaemon} from "./agent-daemon";
import {spawnSync} from "node:child_process";
import {deviceModels} from "../../src/device/models";

const daemon=createAgentDaemon({deferCreations:false});
const config={...loadConfig([],{}),endpoint:daemon.endpoint,authToken:undefined,pairingToken:undefined,reducedMotion:true};
const client=new DaemonConnection(config),model=new WorkspaceModel(client,config),visuals=new AppVisualState();
const recording=createRecordingAppRenderer(),live=new Set<number>();
let nextId=1,created=0,dropped=0,updated=0,uploaded=0;
const options:Array<Record<string,unknown>>=[];
let hub:Bun.Server<undefined>|undefined;
Object.assign(recording.renderer,{
  createDeviceViewport(json:string){options.push(JSON.parse(json));const id=nextId++;live.add(id);created++;return id;},
  updateDeviceViewport(id:number,json:string){assert.ok(live.has(id));options.push(JSON.parse(json));updated++;},
  putDeviceFrame(id:number,width:number,height:number,pixels:Buffer){assert.ok(live.has(id));assert.equal(width,16);assert.equal(height,16);assert.equal(pixels.length,1024);assert.ok(pixels[2]>240);uploaded++;},
  clearDeviceFrame(id:number){assert.ok(live.has(id));},
  dropDeviceViewport(id:number){assert.ok(live.delete(id));dropped++;},
  hitDeviceViewport(id:number,x:number,y:number,width:number,height:number){assert.ok(live.has(id));assert.ok(width>0&&height>0);return uploaded?JSON.stringify({x:x/width,y:y/height,display:"inner"}):"null";},
  facingDeviceViewport(){return options.at(-1)?.display||"inner";},
});
const root=createRoot(recording.renderer),callbacks=new Set<()=>void>();
const restore=usePresentationScheduler(callback=>{callbacks.add(callback);return()=>callbacks.delete(callback);});
const pulse=async()=>{const frame=[...callbacks];callbacks.clear();for(const callback of frame)callback();await recording.pump(root);await Bun.sleep(5);};
const until=async(check:()=>boolean)=>{const end=Date.now()+5000;while(!check()){await pulse();if(Date.now()>end)throw new Error("Device panel condition timed out");}await recording.pump(root);};
const event=async(testId:string,eventType:string,args:Record<string,unknown>={})=>{const node=recording.find(testId);assert.ok(node,`Missing ${testId}`);root.dispatch({elementId:node.id,eventType,...args});await recording.pump(root);};
try {
  model.start();client.start();await until(()=>!!model.state.activeWorkspace&&!model.state.loading);
  root.render(()=><App model={model} config={config} visuals={visuals}/>);await recording.pump(root);
  assert.equal(!!recording.find("device-preview-toggle"),process.env.NEOISM_GPUIX_DEVICE_PREVIEW==="1");
  assert.equal(recording.find("device-preview-panel"),undefined);assert.equal(created,0);
  if(process.env.NEOISM_GPUIX_DEVICE_PREVIEW==="1"){
    await event("app-content","keyDown",{key:"d",modifiers:{alt:true,shift:true}});await until(()=>!!recording.find("device-preview-panel")&&created===1);
    assert.equal(live.size,1);assert.ok(updated>0);assert.equal(daemon.ptyRequests.length,0);
    assert.equal(uploaded,0,"Opening the panel never starts device tools or a stream");
    assert.equal(recording.find("device-viewport")!==undefined,true);
    assert.ok(recording.find("device-controls-rail"));
    assert.equal(recording.find("device-hub-url"),undefined,"Connection settings must not consume the default viewport");
    assert.equal(recording.find("device-back"),undefined,"Rear-view action belongs in More, not a large permanent Back button");
    await event("device-viewport","mouseDown",{button:0,x:450,y:350});
    await event("device-viewport","mouseMove",{button:0,x:480,y:350});const firstYaw=Number(options.at(-1)!.yaw);
    await event("device-viewport","mouseMove",{button:0,x:510,y:350});const secondYaw=Number(options.at(-1)!.yaw);
    await event("device-viewport","mouseMove",{button:0,x:540,y:350});const thirdYaw=Number(options.at(-1)!.yaw);
    assert.ok(firstYaw>0&&secondYaw>firstYaw&&thirdYaw>secondYaw,"Orbit must survive consecutive reactive pose updates without cancelling its own gesture");
    await event("device-viewport","mouseUp",{button:0,x:540,y:350});
    for(const model of deviceModels){
      await event("device-model-picker","click");
      for(const other of deviceModels)assert.ok(recording.find(`device-model:${other.id}`),`Full lineup includes ${other.label}`);
      await event(`device-model:${model.id}`,"click");
      assert.equal(options.at(-1)!.model,model.id);
      assert.equal(created,1,"Changing models reuses the native viewport");
    }
    await event("device-keyboard-toggle","click");assert.equal(options.at(-1)!.keyboard,true);
    await event("device-more","click");await event("device-back","click");assert.equal(options.at(-1)!.yaw,Math.PI);
    await event("device-more","click");await event("device-front","click");assert.equal(options.at(-1)!.yaw,0);
    await event("device-model-picker","click");await event("device-model:iphone-duo","click");
    assert.equal(options.at(-1)!.keyboard,false);
    await event("device-hinge-book","click");assert.equal(options.at(-1)!.hinge,Math.PI/2);
    await event("device-more","click");await event("device-display-cover","click");assert.equal(options.at(-1)!.display,"cover");assert.equal(options.at(-1)!.yaw,Math.PI/2);assert.equal(options.at(-1)!.pitch,0);
    await event("device-model-picker","click");await event("device-finish:gold","click");assert.deepEqual(options.at(-1)!.color,[.765,.686,.569]);await event("device-model-picker","click");
    await event("device-viewport","scroll",{deltaY:2,precise:false});assert.ok(Number(options.at(-1)!.zoom)>1);
    await event("device-reset","click");assert.equal(options.at(-1)!.zoom,1);
    await event("device-preview-close","click");await until(()=>!recording.find("device-preview-panel"));
    assert.equal(live.size,0);assert.equal(dropped,1);
    await event("device-preview-toggle","click");await until(()=>created===2);
    assert.equal(live.size,1);
    const video=spawnSync("ffmpeg",["-hide_banner","-loglevel","error","-f","lavfi","-i","color=c=blue:s=16x16:r=10","-frames:v","4","-c:v","libx264","-preset","ultrafast","-tune","zerolatency","-f","h264","pipe:1"]);
    assert.equal(video.status,0,"Codec integration test requires ffmpeg with libx264");
    const inputs:Array<{type:string;action?:string;x?:number;y?:number}>=[];
    const controlRequests:string[]=[];
    let socketClosed=false;
    hub=Bun.serve<undefined>({hostname:"127.0.0.1",port:0,fetch(request,server){
      const path=new URL(request.url).pathname;
      if(path==="/api/devices")return Response.json({simulators:[],emulators:[{id:"test-emulator",name:"Test Android",platform:"android",version:"35",booted:true,physical:false}]});
      if(path==="/vendor/serve-emu/api")return Response.json({stream:{transport:"h264"}});
      if(path==="/vendor/serve-emu/api/uimode"||path==="/vendor/serve-emu/api/screenshot")controlRequests.push(path);
      if(path==="/vendor/serve-emu/ws"&&server.upgrade(request))return;
      return new Response("Not found",{status:404});
    },websocket:{message(ws,raw){const packet=JSON.parse(String(raw));inputs.push(packet);if(packet.type==="reset-video")ws.send(video.stdout);},close(){socketClosed=true;}}});
    await event("device-tools","click");await event("device-hub-url","change",{value:`http://127.0.0.1:${hub.port}`});
    await event("device-hub-connect","click");await until(()=>!!recording.find("device-watch:test-emulator"));
    assert.equal(uploaded,0,"Discovery does not start video");
    await event("device-watch:test-emulator","click");await until(()=>uploaded>0);
    assert.equal(recording.find("device-rotate")?.props.tabIndex,-1,"Unsupported rotation must not pretend to change the device");
    await event("device-more","click");await event("device-more","keyDown",{key:"b",keyChar:"b"});
    assert.equal(inputs.some(input=>input.type==="text"),false,"Typing on controls must not leak into the remote device");
    await event("device-action-menu","keyDown",{key:"escape"});
    await event("device-viewport","keyDown",{key:"a",keyChar:"a"});await until(()=>inputs.some(input=>input.type==="text"));
    await event("device-appearance","click");await until(()=>controlRequests.includes("/vendor/serve-emu/api/uimode")&&recording.find("device-screenshot")?.props.tabIndex===0);
    assert.equal(recording.find("device-appearance")?.props.tabIndex,-1);
    await event("device-screenshot","click");await until(()=>controlRequests.includes("/vendor/serve-emu/api/screenshot")&&recording.find("device-screenshot")?.props["aria-label"]==="Save screenshot"&&recording.find("device-screenshot")?.props.tabIndex===-1);
    assert.equal(socketClosed,false,"Unsupported optional controls must not interrupt video");
    const viewport=recording.find("device-viewport")!,bounds=recording.renderer.getElementBounds!(viewport.id)!;
    await event("device-viewport","mouseDown",{button:0,x:bounds.x+bounds.width/2,y:bounds.y+bounds.height/2});
    await event("device-viewport","mouseUp",{button:0,x:bounds.x+bounds.width/2,y:bounds.y+bounds.height/2});
    await until(()=>inputs.some(input=>input.type==="touch"&&input.action==="up"));
    assert.ok(inputs.some(input=>input.action==="down"&&input.x!>=0&&input.x!<=1&&input.y!>=0&&input.y!<=1));
    await event("device-preview-close","click");await until(()=>socketClosed&&!recording.find("device-preview-panel"));
    assert.equal(live.size,0);assert.equal(dropped,2);
  }
  console.log(JSON.stringify({defaultOff:process.env.NEOISM_GPUIX_DEVICE_PREVIEW!=="1",created,dropped,uploaded,noPty:true}));
} finally {root.unmount();hub?.stop(true);restore();visuals.dispose();model.dispose();client.stop();daemon.stop();assert.equal(live.size,0);}