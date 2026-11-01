import {createEffect,createSignal,onCleanup,onMount,Show} from "solid-js";
import {createWindowSize,useGpuixRequired,type HostElement,type StyleDesc} from "@gpuix/solid";
import type {AppConfig} from "../config";
export interface ImagePreview {name:string;bytes:Uint8Array}
/** Upload once per image change; native image caching handles paints and resizes. */
export function AttachmentImage(props:{image:ImagePreview;testId?:string;style?:StyleDesc}){
 const [failed,setFailed]=createSignal(false);
 let host:HostElement|undefined,disposed=false;
 createEffect(()=>{const bytes=props.image.bytes;setFailed(false);queueMicrotask(()=>{if(disposed||!host?.mounted)return;try{host.root?.mutations.flushMutations();host.setImage(bytes);}catch{setFailed(true);}});});
 onCleanup(()=>{disposed=true;});
 return <Show when={!failed()} fallback={<text style={props.style}>Image unavailable</text>}><img ref={host} testId={props.testId} alt={props.image.name} objectFit="contain" style={{...props.style,pointerEvents:"none"}}/></Show>;
}
export function ImagePreviewModal(props:{image:ImagePreview;config:AppConfig;onClose:()=>void}){
 const renderer=useGpuixRequired(),size=createWindowSize();let host:HostElement|undefined;
 const previous=renderer.getFocusedElementId?.();
 onMount(()=>queueMicrotask(()=>{if(host?.mounted){host.root?.mutations.flushMutations();renderer.focusElement?.(host.id);}}));
 onCleanup(()=>{if(previous!=null)renderer.focusElement?.(previous);});
 return <anchored fill="window" priority={10} deferred occlude><div ref={host} testId="image-preview-modal" role="dialog" aria-label={`Image preview: ${props.image.name}`} tabIndex={0}
  onKeyDown={event=>{event.stopPropagation();if(event.key?.toLowerCase()==="escape"){event.preventDefault();props.onClose();}}}
  style={{position:"absolute",left:0,top:0,width:size().width,height:size().height,backgroundColor:"#000000dd"}}>
  <AttachmentImage image={props.image} testId="image-preview-full" style={{position:"absolute",left:32,top:64,width:Math.max(1,size().width-64),height:Math.max(1,size().height-96)}}/>
  <text style={{position:"absolute",left:32,top:20,right:80,color:props.config.theme.fg,fontSize:14,fontFamily:props.config.fontFamily}}>{props.image.name}</text>
  <div role="button" testId="image-preview-close" aria-label="Close image preview" onClick={props.onClose} style={{position:"absolute",right:16,top:12,width:40,height:40,cursor:"pointer",backgroundColor:props.config.theme.surface,borderRadius:6,alignItems:"center",justifyContent:"center"}}><text style={{color:props.config.theme.fg,fontSize:24}}>×</text></div>
 </div></anchored>;
}
