import { handleApplicationShortcut } from "../commands/application-shortcuts";
import { createEffect, createMemo, createSignal, For, onCleanup, onMount, Show } from "solid-js";
import { createWindowSize, useGpuixRequired, type HostElement, type KeyEvent } from "@gpuix/solid";
import type { AppConfig } from "../config";
import { FinderController, type FinderFrame, type FinderTarget } from "./FinderController";
import { PaintLayer } from "../ui/PaintLayer";
import { cssColor, type CanvasOp } from "../ui/canvas";
import { dialogOverlayOffset, useDialogViewport } from "../ui/overlay-viewport";
import { coreCall } from "../native-core";
import {CursorBlinkClock} from "../ui/cursor-blink";

/** Original Rust Finder pixels with transparent native input and hit targets. */
export function FinderView(props: { controller: FinderController; config: AppConfig;
  onAccept: (target: FinderTarget) => void; onClose: () => void; onPreview?: (target: FinderTarget) => void; inputTestId?: string }) {
  const renderer = useGpuixRequired(), viewport = useDialogViewport(renderer, createWindowSize()), config = props.config;
  const [frame, setFrame] = createSignal<FinderFrame>(props.controller.frame);
  const [blink,setBlink]=createSignal(false),[rainbow,setRainbow]=createSignal<number[]>([]);
  const blinkClock=new CursorBlinkClock(setBlink,{scope:renderer});
  createEffect(()=>{const current=frame(),row=current.rows[current.selected];blinkClock.update({active:current.enabled&&!!current.cursorQuad?.length,enabled:config.cursorBlink,intervalMs:config.cursorBlinkInterval,animating:current.animating,activityToken:`${current.mode}:${current.selected}:${row?.path}:${current.selectedY}:${current.top}:${current.offset}`});});
  const colorTimer=config.cursorStyle==="rainbow"?setInterval(()=>setRainbow(coreCall<{rainbow:number[]}>("cursor",{seconds:performance.now()/1000}).rainbow),1000/60):undefined;
  onCleanup(()=>{blinkClock.dispose();if(colorTimer!==undefined)clearInterval(colorTimer);});
  createEffect(() => { const controller=props.controller;setFrame(controller.frame);onCleanup(controller.subscribe(setFrame)); });
  createEffect(() => { props.controller.appearance(config); });
  createEffect(() => { const bounds=viewport();props.controller.layout(bounds.width,bounds.height); });
  let input: HostElement | undefined;
  onMount(() => { if(input){input.root?.mutations.flushMutations();renderer.focusElement?.(input.id);} });
  const accept=(index?:number)=>{if(index!==undefined)props.controller.select(index);const target=props.controller.selectedTarget();if(target)props.onAccept(target);};
  let lastPreview="";
  createEffect(()=>{const current=frame(),row=current.rows[current.selected],key=`${current.mode}:${current.query}:${row?.path}:${row?.line}:${row?.column}`;
    if(lastPreview!==key){lastPreview=key;const target=props.controller.selectedTarget();if(target)props.onPreview?.(target);}
  });
  const keyDown=(event:KeyEvent)=>{
    if(handleApplicationShortcut(renderer,event))return;
    event.stopPropagation();const key=event.key?.toLowerCase();
    if(key==="escape"){event.preventDefault();props.onClose();}
    else if(key==="tab"&&event.modifiers?.ctrl&&!event.modifiers.alt&&!event.modifiers.cmd){event.preventDefault();props.controller.cycle();}
    else if(key==="up"||key==="down"){event.preventDefault();props.controller.move(key==="up"?-1:1);}
  };
  const rows=()=>frame().rows.slice(Math.max(0,frame().top-3),frame().top+frame().visibleRows+4);
  const offset=createMemo(()=>dialogOverlayOffset(viewport(),frame().rect));
  const cursor=createMemo(():CanvasOp[]=>{
    const points=frame().cursorQuad||[];if(!points.length)return [];
    if(!blink())return [];
    const color=config.cursorStyle==="rainbow"&&rainbow().length?`#${rainbow().slice(0,3).map(value=>value.toString(16).padStart(2,"0")).join("")}`:config.cursorColor||config.theme.accent;
    return [{k:"p",c:cssColor(color),p:[points.flatMap(point=>[point[0]!,point[1]!])]}];
  });
  return <anchored fill="window" deferred priority={2} occlude={false} style={{backgroundColor:"transparent",pointerEvents:"none"}}>
    <div testId="finder-viewport" style={{position:"absolute",left:viewport().x,top:viewport().y,width:viewport().width,height:viewport().height,overflow:"hidden",pointerEvents:"none"}}>
      <div testId="finder-paint-origin" style={{position:"absolute",left:offset().x,top:offset().y,width:viewport().width,height:viewport().height,pointerEvents:"none"}}>
      <PaintLayer config={config} commands={frame().commands||[]} extra={cursor()}/>
      <div role="dialog" aria-label="Finder" testId="finder" onKeyDown={keyDown} onMouseDownOutside={props.onClose}
        style={{position:"absolute",left:frame().rect[0],top:frame().rect[1],width:frame().rect[2],height:frame().rect[3],pointerEvents:"auto",overflow:"hidden",borderTopLeftRadius:frame().radius,borderTopRightRadius:frame().radius}}>
        <input ref={input} testId={props.inputTestId||"command-palette"} aria-label={frame().placeholder} autoFocus value={frame().query} placeholder={frame().placeholder}
          onChange={event=>props.controller.query(event.value||"")} onSubmit={()=>accept()}
          style={{position:"absolute",left:frame().inputRect[0],top:frame().inputRect[1],width:frame().inputRect[2],height:frame().inputRect[3],opacity:0,fontSize:frame().inputFontSize,fontFamily:config.fontFamily,pointerEvents:"auto"}}/>
        <div role="button" aria-label="Cycle Finder search mode" testId="finder-mode" onClick={()=>props.controller.cycle()}
          style={{position:"absolute",left:frame().inputRect[0]+frame().inputRect[2]-80*config.scale,top:frame().inputRect[1],width:80*config.scale,height:frame().inputRect[3],cursor:"pointer"}}/>
        <div testId="finder-results" onScroll={event=>props.controller.scroll(event.precise?event.deltaY||0:(event.deltaY||0)*22*3)}
          style={{position:"absolute",left:frame().listRect[0],top:frame().listRect[1],width:frame().listRect[2],height:frame().listRect[3],overflow:"hidden"}}>
          <For each={rows().map(row=>row.index)}>{index=>{
            const initial=frame().rows[index]!,row=()=>frame().rows[index]||initial;
            return <div testId={`finder-result:${index}`} role="option" aria-selected={frame().selected===index} aria-label={row().path||row().text}
              onMouseDown={()=>accept(index)} onMouseMove={()=>{if(frame().selected!==index)props.controller.select(index);}}
              style={{position:"absolute",left:0,top:(index-frame().top)*frame().rowHeight+frame().offset,width:"100%",height:frame().rowHeight,cursor:"pointer"}}/>;
          }}</For>
          <Show when={frame().error&&!frame().rows.length}><div role="alert" style={{position:"absolute",left:0,top:0,width:"100%",height:frame().rowHeight,backgroundColor:config.theme.bg,pointerEvents:"none"}}>
            <text style={{position:"absolute",left:14*config.scale,top:(frame().rowHeight-frame().fontSize)/2,fontSize:frame().fontSize,fontFamily:config.fontFamily,color:config.theme.red}}>{frame().error}</text>
          </div></Show>
        </div>
        <Show when={frame().preview}><div testId="finder-preview" aria-label={frame().previewPath||"Preview"} style={{position:"absolute",left:frame().previewRect[0],top:frame().previewRect[1],width:frame().previewRect[2],height:frame().previewRect[3],pointerEvents:"none"}}/></Show>
      </div>
      </div>
    </div>
  </anchored>;
}
