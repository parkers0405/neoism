import { createEffect,createSignal,onCleanup,Show } from "solid-js";
import type { HostElement } from "@gpuix/solid";
import type { NativeMarkdown,MarkdownFrame } from "./NativeMarkdown";
/** The original renderer reserves the complete banner band. Native aspect-fill
 * plus the document viewport clip reproduces the desktop source crop at scroll. */
export function MarkdownCover(props:{markdown:NativeMarkdown;cover:MarkdownFrame["cover"]}){
 const [loaded,setLoaded]=createSignal(false);
 let image:HostElement|undefined,version=0,disposed=false,lastValue:string|undefined;
 createEffect(()=>{
  const cover=props.cover;if(!cover){lastValue=undefined;version++;setLoaded(false);return;}
  if(lastValue===cover.value)return;setLoaded(false);lastValue=cover.value;const current=++version;
  void props.markdown.coverBytes(cover.value).then(bytes=>{if(!bytes||disposed||current!==version)return;queueMicrotask(()=>{if(!image?.mounted||disposed||current!==version)return;image.root?.mutations.flushMutations();image.setImage(bytes);setLoaded(true);});});
 });
 onCleanup(()=>{disposed=true;version++;});
 return <Show when={props.cover}>{cover=><img ref={image} testId="markdown-cover" alt="Document cover" objectFit="cover" style={{opacity:loaded()?1:0,position:"absolute",left:cover().rect[0],top:cover().rect[1],width:cover().rect[2],height:cover().rect[3],pointerEvents:"none"}}/>}</Show>;
}
