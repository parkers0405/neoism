import {createEffect,createMemo,createSignal,onCleanup,onMount,untrack} from "solid-js";
import {useGpuixRequired,type EventPayload,type HostElement,type KeyEvent} from "@gpuix/solid";
import type {AppConfig} from "../config";
import {AnimationFrameScheduler,ANIMATION_FRAME_MS} from "../animation/frame-scheduler";
import {paintedChromeBounds} from "../ui/chrome-target-bounds";
import type {DeviceFrame} from "./host";
import type {DeviceOrientation} from "./protocol";
import {parseDeviceHit,orbitDevice,zoomDevice,type DevicePose,type DeviceScreenHit} from "./interaction";
import {deviceFinishes,type DeviceDisplay} from "./models";
import {focusNativeElement} from "../native-focus";

export interface NativeDeviceRenderer {
  createDeviceViewport?: (options?:string)=>number;
  updateDeviceViewport?: (id:number,options:string)=>void;
  putDeviceFrame?: (id:number,width:number,height:number,pixels:Buffer)=>void;
  clearDeviceFrame?: (id:number)=>void;
  dropDeviceViewport?: (id:number)=>void;
  hitDeviceViewport?: (id:number,x:number,y:number,width:number,height:number)=>string;
  facingDeviceViewport?: (id:number)=>string;
}
export function DeviceViewport(props:{config:AppConfig;pose:DevicePose;preview?:boolean;orientation?:DeviceOrientation;sourceKey?:string;inputConnected:boolean;onPose:(pose:DevicePose)=>void;
  onFrameSink:(sink:((frame:DeviceFrame)=>void)|undefined)=>void;onError:(message:string)=>void;onTouch:(phase:"begin"|"move"|"end",point:DeviceScreenHit)=>void;onFocus:()=>void;
  onDisplayRequest?:(display:DeviceDisplay)=>void;onFacingDisplay?:(display:DeviceDisplay)=>void;onHost?:(host:HostElement|undefined)=>void;onKeyDown?:(event:KeyEvent)=>void}) {
  const renderer=useGpuixRequired(),native=renderer as unknown as NativeDeviceRenderer;
  const [id,setId]=createSignal(0),[revision,setRevision]=createSignal(0);
  const scheduler=new AnimationFrameScheduler(ANIMATION_FRAME_MS);
  let host:HostElement|undefined,disposed=false,latest:DeviceFrame|undefined;
  let size={width:0,height:0},drag:{mode:"touch"|"orbit";x:number;y:number;point?:DeviceScreenHit}|undefined;
  const [dragging,setDragging]=createSignal(false);
  let previousCamera:{yaw:number;pitch:number}|undefined;
  const options=createMemo(()=>JSON.stringify({model:props.pose.model,color:deviceFinishes[props.pose.finish].rgb,keyboard:props.pose.keyboard,duoBook:props.pose.duoBook,display:props.pose.display,yaw:props.pose.yaw,pitch:props.pose.pitch,hinge:(180-props.pose.hinge)*Math.PI/180,zoom:props.pose.zoom,flat:props.pose.flat,orientation:props.orientation||"portrait",preview:props.preview===true}));
  const interactionGeometry=createMemo(()=>[props.pose.model,props.pose.keyboard,props.pose.duoBook,props.pose.display,props.pose.flat,props.pose.hinge,props.pose.zoom,props.orientation].join(":"));
  const inputReady=createMemo(()=>props.inputConnected);
  const publish=()=>setRevision(value=>value+1);
  const reportFacing=()=>{
    if(disposed||!id()||props.pose.model!=="iphone-duo"||props.pose.flat)return;
    const facing=native.facingDeviceViewport?.(id());
    if(facing==="inner"||facing==="cover")untrack(()=>props.onFacingDisplay?.(facing));
  };
  const cancel=()=>{if(drag?.mode==="touch"&&drag.point)props.onTouch("end",drag.point);drag=undefined;setDragging(false);};
  const flush=()=>{
    if(disposed||!id()||!latest)return;
    const frame=latest;latest=undefined;
    try{native.putDeviceFrame!(id(),frame.width,frame.height,Buffer.from(frame.pixels.buffer,frame.pixels.byteOffset,frame.pixels.byteLength));publish();}
    catch(cause){props.onError(cause instanceof Error?cause.message:String(cause));}
  };
  onMount(()=>{
    if(!native.createDeviceViewport||!native.updateDeviceViewport||!native.putDeviceFrame||!native.clearDeviceFrame||!native.dropDeviceViewport||!native.hitDeviceViewport||!native.facingDeviceViewport){props.onError("This display binding lacks the complete device-preview API. Rebuild with NEOISM_GPUIX_DEVICE_PREVIEW=1.");return;}
    try{setId(native.createDeviceViewport(options()));props.onError("");props.onFrameSink(frame=>{latest=frame;scheduler.request(flush);});}
    catch(cause){props.onError(cause instanceof Error?cause.message:String(cause));}
  });
  createEffect(()=>{
    const key=id(),value=options(),camera={yaw:props.pose.yaw,pitch:props.pose.pitch};
    const moved=previousCamera&&(previousCamera.yaw!==camera.yaw||previousCamera.pitch!==camera.pitch);previousCamera=camera;
    if(key){try{native.updateDeviceViewport!(key,value);publish();if(moved&&!drag)reportFacing();}catch(cause){props.onError(String(cause));}}
  });
  createEffect(()=>{inputReady();cancel();});
  createEffect(()=>{interactionGeometry();cancel();});
  createEffect(()=>{props.sourceKey;const key=id();if(key){cancel();scheduler.cancel();latest=undefined;native.clearDeviceFrame?.(key);publish();}});
  onCleanup(()=>{disposed=true;cancel();props.onHost?.(undefined);props.onFrameSink(undefined);scheduler.dispose();latest=undefined;if(id())native.dropDeviceViewport?.(id());});
  const point=(event:EventPayload)=>{const bounds=host&&paintedChromeBounds(renderer,host.id);return{x:(event.x||0)-(bounds?.x||0),y:(event.y||0)-(bounds?.y||0)};};
  const hit=(p:{x:number;y:number})=>{
    if(!id()||!native.hitDeviceViewport)return null;
    try{return parseDeviceHit(native.hitDeviceViewport(id(),p.x,p.y,size.width,size.height));}catch(cause){props.onError(String(cause));return null;}
  };
  const move=(event:EventPayload)=>{
    const gesture=drag;if(!gesture)return;const next=point(event);
    if(gesture.mode==="orbit")props.onPose(orbitDevice(props.pose,next.x-gesture.x,next.y-gesture.y));
    else{const screen=hit(next);if(screen&&screen.display===gesture.point?.display){gesture.point=screen;props.onTouch("move",screen);}}
    gesture.x=next.x;gesture.y=next.y;
  };
  return <div ref={value=>{host=value;props.onHost?.(value);}} testId="device-viewport" tabIndex={0} role="application" aria-label={props.preview?"3D model preview":"Device screen"} onKeyDown={props.onKeyDown} onFocus={props.onFocus} onLayout={event=>{size={width:event.width||0,height:event.height||0};}}
    onMouseDown={event=>{if(event.button!==undefined&&event.button!==0)return;if(host)focusNativeElement(renderer,host.id);props.onFocus();cancel();const p=point(event),screen=!event.modifiers?.alt&&props.inputConnected?hit(p):null;if(screen&&screen.display!==props.pose.display){props.onDisplayRequest?.(screen.display);return;}drag={mode:screen?"touch":"orbit",...p,...(screen?{point:screen}:{})};setDragging(drag.mode==="orbit");if(screen)props.onTouch("begin",screen);}}
    onMouseMove={move} onMouseUp={event=>{const orbited=drag?.mode==="orbit";move(event);cancel();if(orbited)queueMicrotask(()=>{try{reportFacing();}catch(cause){props.onError(String(cause));}});}} onBlur={cancel} onScroll={event=>{
      if(props.pose.flat)return;
      const controls=event as EventPayload&{preventDefault?:()=>void;stopPropagation?:()=>void};controls.preventDefault?.();controls.stopPropagation?.();
      props.onPose(zoomDevice(props.pose,(event.deltaY||0)*(event.precise?1:60)));
    }}
    style={{position:"relative",flexGrow:1,minWidth:0,minHeight:0,overflow:"hidden",cursor:dragging()?"grabbing":"grab",userSelect:"none"}}>
    <device_viewport viewportId={id()} frameRevision={revision()} style={{position:"absolute",left:0,top:0,right:0,bottom:0,pointerEvents:"none"}}/>
  </div>;
}