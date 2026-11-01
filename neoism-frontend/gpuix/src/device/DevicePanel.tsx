import {createEffect,createSignal,For,mergeProps,onCleanup,Show} from "solid-js";
import {mkdir,writeFile} from "node:fs/promises";
import {homedir} from "node:os";
import {join} from "node:path";
import {useGpuixRequired,type EventPayload,type HostElement,type KeyEvent,type StyleDesc} from "@gpuix/solid";
import {focusNativeElement} from "../native-focus";
import {paintedChromeBounds} from "../ui/chrome-target-bounds";
import {DeviceHost,type DeviceFrame,type DeviceInfo,type DeviceSession,type DeviceState} from "./host";
import {deviceHinge,faceDevice,initialDevicePose,selectDeviceModel,type DevicePose} from "./interaction";
import {deviceModels,deviceModel,deviceFinishes,resolveDeviceModel,type DeviceModel,type DeviceDisplay} from "./models";
import {DeviceViewport} from "./DeviceViewport";
import {DeviceIcon} from "./DeviceIcon";
import {DEVICE_UI_FONT,DeviceIconButton,DeviceRail,DeviceRailDivider,DeviceMenuItem,DeviceTooltipProvider,devicePanelConfig} from "./DeviceChrome";
import type {DevicePanelProps} from "./types";

/** An explicitly opened host connection. Closing this panel releases streams
 * and decoders, but never powers down a simulator or installs host tools. */
export function DevicePanel(input:DevicePanelProps) {
  const props=mergeProps(input,{config:devicePanelConfig(input.config)});
  const renderer=useGpuixRequired(),c=props.config.theme,s=props.config.scale;
  const [endpoint,setEndpoint]=createSignal(process.env.NEOISM_DEVICE_HUB_URL||"");
  const [devices,setDevices]=createSignal<DeviceInfo[]>([]),[selected,setSelected]=createSignal<DeviceInfo>();
  const [busy,setBusy]=createSignal(false),[notice,setNotice]=createSignal("");
  const [state,setState]=createSignal<DeviceState>({status:"closed",inputConnected:false});
  const [pose,setPose]=createSignal<DevicePose>(initialDevicePose());
  const [menu,setMenu]=createSignal<"models"|"connection"|"more">(),[manualModel,setManualModel]=createSignal(false);
  const [panelBounds,setPanelBounds]=createSignal({x:0,y:0,width:props.width,height:600});
  const [menuAnchor,setMenuAnchor]=createSignal({x:0,y:0});
  const profile=()=>deviceModel(pose().model);
  const [dimensions,setDimensions]=createSignal({width:0,height:0});
  const [nativeError,setNativeError]=createSignal("");
  const [capturePending,setCapturePending]=createSignal(false),[appearancePending,setAppearancePending]=createSignal(false);
  const [sourceRevision,setSourceRevision]=createSignal(0);
  let host:HostElement|undefined,screenHost:HostElement|undefined,moreHost:HostElement|undefined,connection:DeviceHost|undefined,session:DeviceSession|undefined,serial=0,disposed=false;
  let frameSink:((frame:DeviceFrame)=>void)|undefined,latest:DeviceFrame|undefined;
  const text=(size=12,color=c.fg):StyleDesc=>({fontFamily:DEVICE_UI_FONT,fontSize:size*s,color});
  const row:StyleDesc={display:"flex",flexDirection:"row",alignItems:"center",minWidth:0,gap:6*s};
  const stop=()=>{serial++;session?.close();session=undefined;connection?.close();connection=undefined;latest=undefined;setSourceRevision(value=>value+1);setSelected(undefined);setState({status:"closed",inputConnected:false});setBusy(false);setCapturePending(false);setAppearancePending(false);setDimensions({width:0,height:0});};
  createEffect(()=>{props.workspaceId;stop();setDevices([]);setNotice("");});
  onCleanup(()=>{disposed=true;stop();frameSink=undefined;props.onHost?.(undefined);});
  createEffect(()=>{props.focusToken;if(props.active&&host)queueMicrotask(()=>{const target=screenHost||host;if(!disposed&&target?.mounted){target.root?.mutations.flushMutations();focusNativeElement(renderer,target.id);}});});
  const error=(cause:unknown)=>setNotice(cause instanceof Error?cause.message:String(cause));
  const discover=async()=>{
    stop();setDevices([]);setNotice("");setBusy(true);
    const generation=serial;
    try {
      const next=new DeviceHost({url:endpoint().trim(),ffmpegPath:process.env.NEOISM_DEVICE_FFMPEG||undefined});connection=next;
      const listing=await next.listDevices();
      if(disposed||generation!==serial)return;
      setDevices(listing.devices);setNotice(listing.warnings.join("; ")||(!listing.devices.length?"No devices found on this host.":""));
    }catch(cause){if(!disposed&&generation===serial)error(cause);}
    finally{if(!disposed&&generation===serial)setBusy(false);}
  };
  const watch=async(device:DeviceInfo,boot=false)=>{
    const current=connection;if(!current||busy())return;
    session?.close();session=undefined;latest=undefined;setSourceRevision(value=>value+1);setNotice("");setBusy(true);setCapturePending(false);setAppearancePending(false);
    const generation=++serial;
    try {
      const target=boot?await current.boot(device):device;
      if(disposed||generation!==serial)return;
      setDevices(items=>items.map(item=>item.id===device.id?target:item));setSelected(target);
      const matched=resolveDeviceModel(target.platform,target.name);
      const fallback=/duo|fold/i.test(target.name)?"iphone-duo":/ipad|tablet/i.test(target.name)?"ipad-pro-13-m5":"iphone-18-pro";
      setPose(before=>selectDeviceModel(before,matched||fallback));setManualModel(!matched);setMenu(undefined);
      session=current.startSession(target,{
        onFrame(frame){if(disposed||generation!==serial)return;setDimensions(before=>before.width===frame.width&&before.height===frame.height?before:{width:frame.width,height:frame.height});if(frameSink)frameSink(frame);else latest=frame;},
        onState(next){
          if(disposed||generation!==serial)return;
          const previous=state();
          if((next.displaySwitch&&!previous.displaySwitch)||next.screen?.activeDisplay!==previous.screen?.activeDisplay){latest=undefined;setSourceRevision(value=>value+1);}
          setState(next);
          if(next.screen?.hingeAngle!==undefined||next.screen?.activeDisplay)setPose(before=>{
            if(before.model!=="iphone-duo")return before;
            const changed={...before,...(next.screen?.hingeAngle!==undefined?{hinge:deviceHinge(next.screen.hingeAngle)}:{}),...(next.screen?.activeDisplay?{display:next.screen.activeDisplay}:{})};
            return changed.hinge!==before.hinge||changed.display!==before.display?faceDevice(changed):before;
          });
        },
      });
      await session.ready;
    }catch(cause){if(!disposed&&generation===serial)error(cause);}
    finally{if(!disposed&&generation===serial)setBusy(false);}
  };
  const disconnect=()=>{stop();setDevices([]);setNotice("");};
  const changeAppearance=async()=>{
    const current=session;if(!current||appearancePending()||capturePending())return;
    setAppearancePending(true);setNotice("");
    try{const appearance=state().appearance??await current.getAppearance();await current.setAppearance(appearance==="dark"?"light":"dark");}
    catch(cause){if(!disposed&&current===session)error(cause);}
    finally{if(!disposed&&current===session)setAppearancePending(false);}
  };
  const saveScreenshot=async()=>{
    const current=session,device=selected();if(!current||!device||capturePending()||appearancePending())return;
    setCapturePending(true);setNotice("");
    try{
      const png=await current.screenshot();if(disposed||current!==session)return;
      const directory=join(homedir(),"Downloads");await mkdir(directory,{recursive:true});if(disposed||current!==session)return;
      const name=device.name.replace(/[^a-zA-Z0-9_-]/g,"-").replace(/-+/g,"-").slice(0,70)||"device";
      const path=join(directory,`Neoism-${name}-${Date.now()}.png`);await writeFile(path,png,{flag:"wx"});
      if(!disposed&&current===session)setNotice(`Screenshot saved to ${path}`);
    }catch(cause){if(!disposed&&current===session)error(cause);}
    finally{if(!disposed&&current===session)setCapturePending(false);}
  };
  const button=(label:string,action:()=>void,options:{id?:string;selected?:boolean;disabled?:boolean;wide?:boolean}={})=><div role="button" tabIndex={options.disabled?-1:0} aria-label={label} aria-disabled={!!options.disabled} testId={options.id}
    onClick={()=>{if(!options.disabled)action();}} onKeyDown={event=>{if(event.key==="enter"||event.key==="space"){event.preventDefault();event.stopPropagation();if(!options.disabled)action();}}}
    style={{...row,justifyContent:"center",paddingLeft:9*s,paddingRight:9*s,height:27*s,borderRadius:5*s,borderWidth:1,borderColor:c.border,flexShrink:0,flexGrow:options.wide?1:0,cursor:options.disabled?"default":"pointer",opacity:options.disabled ? .45 : 1,backgroundColor:c.surface,hover:options.disabled?undefined:{backgroundColor:c.border}}}>
    <text style={{...text(11,options.selected?c.accent:c.dim),pointerEvents:"none",whiteSpace:"nowrap"}}>{label}</text>
  </div>;
  const chooseModel=(model:DeviceModel)=>{
    setPose(before=>{
      const next=selectDeviceModel(before,model),screen=state().screen;
      if(model!=="iphone-duo"||!screen)return next;
      return faceDevice({...next,...(screen.activeDisplay?{display:screen.activeDisplay}:{}),...(screen.hingeAngle!==undefined?{hinge:deviceHinge(screen.hingeAngle)}:{})});
    });
    setMenu(undefined);setManualModel(true);
  };
  const setDisplay=(display:DeviceDisplay,frameModel=true)=>{
    if(session){setNotice("");if(!session.setDisplay(display))setNotice(session.state.detail||"Display control is unavailable or a request is already pending.");return;}
    setPose(before=>frameModel?faceDevice({...before,display,hinge:display==="cover"?0:180}):{...before,display});
  };
  const followDisplay=(display:DeviceDisplay)=>{
    if(display===pose().display||state().displaySwitch)return;
    if(!session)setDisplay(display,false);
    else {
      const screen=state().screen;
      // Like the reference viewer, alternate resting faces are eligible only
      // between closed and fully open. Do not issue guesses while orbiting.
      if(state().inputConnected&&screen?.supportsDisplaySelection&&screen.hingeAngle!==undefined&&screen.hingeAngle>20&&screen.hingeAngle<180)setDisplay(display,false);
    }
  };
  const setHinge=(angle:number)=>{
    // A connected device's screen metadata is authoritative. Do not animate
    // into a posture the host rejected or has not applied yet.
    if(session){if(!session.setHingeAngle(angle))setNotice("Posture control is unavailable or a request is already pending.");return;}
    setPose(before=>faceDevice({...before,hinge:deviceHinge(angle),display:angle<=20?"cover":"inner"}));
  };
  const keyDown=(event:KeyEvent,screen=false)=>{
    if(props.onCustomKey?.(event))return;
    event.stopPropagation();
    const key=event.key?.toLowerCase();
    if(key==="escape"){event.preventDefault();if(menu())setMenu(undefined);else props.onClose();return;}
    if(screen&&!menu()&&!event.modifiers?.ctrl&&!event.modifiers?.cmd&&!event.modifiers?.alt&&event.keyChar&&session){event.preventDefault();session.sendText(event.keyChar);}
  };
  const toggleMenu=(next:"models"|"connection"|"more",event?:EventPayload)=>{
    if(next==="more"){
      const bounds=moreHost&&paintedChromeBounds(renderer,moreHost.id),panel=panelBounds();
      setMenuAnchor({x:event?.x!==undefined?event.x-14*s:bounds?.x??panel.x+props.width-54*s,y:event?.y!==undefined?event.y-14*s:bounds?.y??panel.y+panel.height/2});
    }
    setMenu(before=>before===next?undefined:next);
  };
  const menuWidth=()=>Math.min((menu()==="connection"?360:menu()==="more"?240:280)*s,props.width-20*s);
  const moreHeight=()=>(((profile().foldable?7:4)+(selected()?1:0))*34+(profile().foldable?36:22))*s;
  const resetView=()=>setPose(before=>({...before,yaw:faceDevice(before).yaw,pitch:0,zoom:1,flat:false}));
  return <DeviceTooltipProvider delayDuration={400}><div ref={value=>{host=value;props.onHost?.(value);}} testId="device-preview-panel" role="region" aria-label="Device preview" tabIndex={0}
    onFocus={()=>props.onFocus?.()} onKeyDown={keyDown}
    onLayout={event=>setPanelBounds({x:event.x||0,y:event.y||0,width:event.width||props.width,height:event.height||600})}
    style={{display:"flex",flexDirection:"column",width:props.width,height:"100%",minWidth:0,minHeight:0,flexShrink:0,borderLeftWidth:s,borderColor:c.border,backgroundColor:c.bg,overflow:"hidden",userSelect:"none"}}>
    <div testId="device-panel-header" style={{...row,height:30*s,minHeight:30*s,paddingLeft:8*s,paddingRight:5*s,gap:3*s,borderBottomWidth:1,borderColor:c.border}}>
      <div role="button" tabIndex={0} testId="device-model-picker" aria-label="Choose preview model" aria-expanded={menu()==="models"} onClick={()=>toggleMenu("models")}
        onKeyDown={event=>{if(event.key==="enter"||event.key==="space"){event.preventDefault();event.stopPropagation();toggleMenu("models");}}}
        style={{...row,gap:5*s,paddingLeft:4*s,paddingRight:6*s,height:27*s,borderRadius:4*s,minWidth:0,cursor:"pointer",hover:{backgroundColor:c.surface}}}>
        <DeviceIcon name="phone" size={12*s} color={c.dim}/>
        <text style={{...text(12,c.dim),fontWeight:500,whiteSpace:"nowrap",overflow:"hidden"}}>{selected()?.name||profile().label}</text>
        <DeviceIcon name="chevron" size={12*s} color={c.muted}/>
      </div>
      <div style={{flexGrow:1}}/>
      <DeviceIconButton config={props.config} icon="close" label="Close device panel" id="device-preview-close" size={22} onClick={props.onClose}/>
    </div>
    <div testId="device-preview-stage" style={{position:"relative",flexGrow:1,minHeight:0,minWidth:0,overflow:"hidden"}}>
      <div style={{position:"absolute",left:10*s,right:60*s,top:12*s,bottom:12*s,display:"flex",flexDirection:"column",minHeight:0}}>
      <DeviceViewport pose={pose()} preview={!selected()} orientation={state().screen?.orientation} onPose={setPose} config={props.config} onError={setNativeError} inputConnected={state().inputConnected}
        onHost={value=>screenHost=value} onKeyDown={event=>keyDown(event,true)}
       sourceKey={String(sourceRevision())} onFrameSink={sink=>{frameSink=sink;if(sink&&latest){sink(latest);latest=undefined;}}}
        onTouch={(phase,point)=>{if(pose().model!=="iphone-duo"||!state().screen?.activeDisplay||point.display===state().screen?.activeDisplay)session?.sendTouch(phase,point.x,point.y);}}
        onDisplayRequest={display=>setDisplay(display,false)} onFacingDisplay={followDisplay} onFocus={()=>props.onFocus?.()}/>
      </div>
      <div testId="device-controls-rail" style={{position:"absolute",right:8*s,top:12*s,bottom:12*s,width:46*s,display:"flex",flexDirection:"column",alignItems:"center",justifyContent:"center",gap:8*s,pointerEvents:"none"}}>
        <DeviceRail config={props.config}>
          <DeviceIconButton config={props.config} icon="home" label="Home" id="device-home" disabled={!state().inputConnected} onClick={()=>session?.pressButton("home")}/>
          <Show when={selected()?.platform==="android"}>
            <DeviceIconButton config={props.config} icon="back" label="Back" id="device-navigation-back" disabled={!state().inputConnected} onClick={()=>session?.pressButton("back")}/>
            <DeviceIconButton config={props.config} icon="square" label="Recents" disabled={!state().inputConnected} onClick={()=>session?.pressButton("recents")}/>
          </Show>
          <DeviceIconButton config={props.config} icon="rotate" label="Rotate device" id="device-rotate" disabled={!state().inputConnected||!state().capabilities?.rotate||pose().keyboard||profile().foldable||!!state().orientationChange} onClick={()=>session?.rotate()}/>
          <DeviceRailDivider config={props.config}/>
          <DeviceIconButton config={props.config} icon={state().appearance==="dark"?"sun":"moon"} label={state().appearance==="dark"?"Switch device to light mode":"Switch device to dark mode"} id="device-appearance" disabled={!state().inputConnected||!state().capabilities?.appearance||appearancePending()||capturePending()} onClick={()=>void changeAppearance()}/>
          <DeviceIconButton config={props.config} icon="settings" label="Device tools" id="device-tools" pressed={menu()==="connection"} onClick={()=>toggleMenu("connection")}/>
          <DeviceIconButton config={props.config} icon="camera" label={capturePending()?"Capturing screenshot":"Save screenshot"} id="device-screenshot" disabled={state().status!=="streaming"||!state().capabilities?.screenshot||capturePending()||appearancePending()||!!state().displaySwitch||!!state().orientationChange} onClick={()=>void saveScreenshot()}/>
          <DeviceIconButton config={props.config} icon="more" label="More device actions" id="device-more" onHost={value=>moreHost=value} onClick={event=>toggleMenu("more",event)}/>
          <DeviceRailDivider config={props.config}/>
          <DeviceIconButton config={props.config} icon="box" label="3D view" id="device-3d-toggle" pressed={!pose().flat} onClick={()=>setPose(before=>({...before,flat:false}))}/>
          <DeviceIconButton config={props.config} icon="phone" label="Flat view" id="device-flat-toggle" pressed={pose().flat} disabled={!dimensions().width} onClick={()=>setPose(before=>({...before,flat:true}))}/>
          <Show when={profile().keyboard&&!pose().flat}><DeviceIconButton config={props.config} icon="keyboard" label={pose().keyboard?"Detach keyboard":"Attach keyboard"} id="device-keyboard-toggle" pressed={pose().keyboard} onClick={()=>setPose(before=>({...before,keyboard:!before.keyboard}))}/></Show>
          <DeviceIconButton config={props.config} icon="reset" label="Restore 3D view" id="device-reset" onClick={resetView}/>
        </DeviceRail>
        <Show when={profile().foldable&&!pose().flat}><DeviceRail config={props.config} label="Fold shape">
          <DeviceIconButton config={props.config} icon="closed" label="Closed" id="device-hinge-closed" pressed={pose().hinge===0} disabled={!!selected()&&!state().screen?.supportsHingeAngle} onClick={()=>setHinge(0)}/>
          <DeviceIconButton config={props.config} icon="book" label="Book" id="device-hinge-book" pressed={pose().hinge>0&&pose().hinge<180} disabled={!!selected()&&!state().screen?.supportsHingeAngle} onClick={()=>setHinge(90)}/>
          <DeviceIconButton config={props.config} icon="open" label="Open" id="device-hinge-open" pressed={pose().hinge===180} disabled={!!selected()&&!state().screen?.supportsHingeAngle} onClick={()=>setHinge(180)}/>
        </DeviceRail></Show>
      </div>
      <Show when={selected()&&!nativeError()&&!notice()&&!state().detail&&(busy()||state().status==="connecting"||state().displaySwitch||state().orientationChange)}>
        <div role="status" testId="device-stream-status" style={{position:"absolute",left:14*s,right:66*s,bottom:12*s,pointerEvents:"none"}}><text style={{...text(11,c.muted),textAlign:"center"}}>{state().displaySwitch?`Switching to ${state().displaySwitch!.requested} display...`:state().orientationChange?"Rotating device...":"Opening device..."}</text></div>
      </Show>
      <Show when={nativeError()||notice()||state().detail}><div style={{position:"absolute",left:14*s,right:66*s,bottom:12*s,padding:10*s,borderRadius:6*s,backgroundColor:c.surface,borderWidth:1,borderColor:c.border}}><text testId="device-notice" style={text(11,c.dim)}>{nativeError()||notice()||state().detail}</text></div></Show>
    </div>
    <Show when={menu()}><anchored fill="window" deferred priority={6} occlude style={{backgroundColor:"transparent"}}>
      <div onMouseDown={()=>setMenu(undefined)} style={{position:"absolute",left:0,top:0,width:"100%",height:"100%",pointerEvents:"auto"}}/>
      <div testId="device-action-menu" role="menu" onKeyDown={event=>{if(event.key==="escape"){event.preventDefault();event.stopPropagation();setMenu(undefined);}}}
        style={{position:"absolute",left:menu()==="more"?Math.max(panelBounds().x+10*s,menuAnchor().x-menuWidth()-8*s):panelBounds().x+10*s,top:menu()==="more"?Math.max(panelBounds().y+34*s,Math.min(menuAnchor().y+28*s-moreHeight(),panelBounds().y+panelBounds().height-moreHeight()-10*s)):panelBounds().y+34*s,width:menuWidth(),maxHeight:Math.max(120*s,panelBounds().height-52*s),display:"flex",flexDirection:"column",padding:5*s,gap:2*s,overflowY:"scroll",backgroundColor:c.surface,borderRadius:8*s,borderWidth:1,borderColor:c.border,pointerEvents:"auto"}}>
        <Show when={menu()==="models"}>
          <text style={{...text(10,c.muted),padding:8*s}}>Preview model</text>
          <Show when={selected()}><text style={{...text(10,c.muted),paddingLeft:8*s,paddingRight:8*s,paddingBottom:4*s}}>{manualModel()?"Preview override":"Matched to simulator"}</text></Show>
          <div testId="device-model-list" style={{display:"flex",flexDirection:"column",gap:2*s}}><For each={deviceModels}>{model=><DeviceMenuItem config={props.config} label={model.label} icon={model.keyboard?"open":"phone"} id={`device-model:${model.id}`} selected={pose().model===model.id} onClick={()=>chooseModel(model.id)}/>}</For></div>
          <div style={{height:1,marginTop:6*s,marginBottom:6*s,backgroundColor:c.border}}/>
          <div style={{...row,padding:8*s,gap:8*s}}><text style={{...text(11,c.muted),flexGrow:1}}>Finish</text>
            <For each={profile().finishes}>{finish=><div role="button" tabIndex={0} aria-label={`${deviceFinishes[finish].label} finish`} aria-pressed={pose().finish===finish} testId={`device-finish:${finish}`}
              onClick={()=>setPose(before=>({...before,finish}))} onKeyDown={event=>{if(event.key==="enter"||event.key==="space"){event.preventDefault();event.stopPropagation();setPose(before=>({...before,finish}));}}}
              style={{display:"flex",alignItems:"center",justifyContent:"center",width:22*s,height:22*s,borderRadius:12*s,borderWidth:1,borderColor:pose().finish===finish?c.fg:c.border,cursor:"pointer"}}><div style={{width:14*s,height:14*s,borderRadius:8*s,backgroundColor:deviceFinishes[finish].color,pointerEvents:"none"}}/></div>}</For>
          </div>
        </Show>
        <Show when={menu()==="more"}>
          <DeviceMenuItem config={props.config} icon="front" label="View front" id="device-front" onClick={()=>{setPose(before=>faceDevice(before));setMenu(undefined);}}/>
          <DeviceMenuItem config={props.config} icon="rear" label="View back" id="device-back" onClick={()=>{setPose(before=>faceDevice(before,true));setMenu(undefined);}}/>
          <Show when={profile().foldable}>
            <div style={{height:1,marginTop:5*s,marginBottom:5*s,backgroundColor:c.border}}/>
            <DeviceMenuItem config={props.config} icon={pose().duoBook?"phone":"book"} label={pose().duoBook?"Portrait hold":"Book hold"} id="device-duo-hold" onClick={()=>{setPose(before=>faceDevice({...before,duoBook:!before.duoBook}));setMenu(undefined);}}/>
            <DeviceMenuItem config={props.config} icon="open" label="Inner display" id="device-display-inner" selected={pose().display==="inner"} disabled={!!selected()&&(!state().screen?.supportsDisplaySelection||!state().inputConnected)} onClick={()=>{setDisplay("inner");setMenu(undefined);}}/>
            <DeviceMenuItem config={props.config} icon="closed" label="Cover display" id="device-display-cover" selected={pose().display==="cover"} disabled={!!selected()&&(!state().screen?.supportsDisplaySelection||!state().inputConnected)} onClick={()=>{setDisplay("cover");setMenu(undefined);}}/>
          </Show>
          <div style={{height:1,marginTop:5*s,marginBottom:5*s,backgroundColor:c.border}}/>
          <DeviceMenuItem config={props.config} icon="connect" label="Connect a device" id="device-connect-menu" onClick={()=>setMenu("connection")}/>
          <Show when={selected()}><DeviceMenuItem config={props.config} icon="disconnect" label="Disconnect" id="device-disconnect" onClick={()=>{disconnect();setMenu(undefined);}}/></Show>
          <DeviceMenuItem config={props.config} icon="close" label="Close device panel" onClick={props.onClose}/>
        </Show>
        <Show when={menu()==="connection"}>
          <text style={{...text(12),fontWeight:500,padding:8*s}}>Devices</text>
          <div style={{...row,padding:6*s,gap:6*s}}>
            <input testId="device-hub-url" aria-label="Device Hub URL" value={endpoint()} placeholder="Device Hub URL" onChange={event=>setEndpoint(event.value||"")}
              onKeyDown={event=>{if(event.key==="enter"){event.preventDefault();event.stopPropagation();void discover();}else if(event.key!=="escape"&&!event.modifiers?.alt)event.stopPropagation();}}
              style={{...text(12),height:28*s,flexGrow:1,minWidth:0,paddingLeft:8*s,paddingRight:8*s,borderRadius:5*s,borderWidth:1,borderColor:c.border,backgroundColor:c.bg}}/>
            {button(busy()?"Connecting...":"Connect",()=>void discover(),{id:"device-hub-connect",disabled:busy()||!endpoint().trim()})}
          </div>
          <Show when={!devices().length}><text style={{...text(11,c.muted),padding:8*s}}>{busy()?"Finding devices...":"Connect a device host to find its simulators and emulators."}</text></Show>
          <For each={devices()}>{device=><div style={{...row,gap:9*s,padding:8*s}}>
            <DeviceIcon name="phone" color={c.dim} size={17*s}/>
            <div style={{display:"flex",flexDirection:"column",gap:3*s,flexGrow:1,minWidth:0}}><text style={{...text(12),whiteSpace:"nowrap",overflow:"hidden"}}>{device.name}</text><text style={text(10,c.muted)}>{device.version} / {device.booted?"Running":"Stopped"}</text></div>
            {button(device.booted?"Open":"Start",()=>void watch(device,!device.booted),{id:`device-watch:${device.id}`,disabled:busy()||device.physical||!!nativeError()})}
          </div>}</For>
        </Show>
      </div>
    </anchored></Show>
  </div></DeviceTooltipProvider>;
}