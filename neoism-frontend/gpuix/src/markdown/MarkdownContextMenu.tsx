import { Show } from "solid-js";
import type { AppConfig } from "../config";
import type { NativeMarkdown,MarkdownFrame } from "./NativeMarkdown";
import { PaintLayer } from "../ui/PaintLayer";
/** Original ContextMenu rows, geometry, spring and glyphs. The document retains
 * native keyboard focus while the late overlay owns its actual pointer box. */
export function MarkdownContextMenu(props:{markdown:NativeMarkdown;config:AppConfig;menu:MarkdownFrame["menu"];origin:{x:number;y:number;width:number;height:number}}){
 const pointer=(kind:string,x=0,y=0)=>props.markdown.call("menu.pointer",{kind,x:x-props.origin.x,y:y-props.origin.y});
 return <Show when={props.menu}>{menu=><anchored fill="window" deferred={true} priority={2} occlude={false} style={{pointerEvents:"none"}}>
  <div style={{position:"absolute",left:props.origin.x,top:props.origin.y,width:props.origin.width,height:props.origin.height,pointerEvents:"none"}}>
   <PaintLayer commands={menu().commands} config={props.config}/>
   <div testId="markdown-context-menu" role="menu" aria-label="Markdown actions" onMouseDownOutside={()=>props.markdown.call("menu.close")}
    onMouseDown={event=>{if(event.button===0)pointer("down",event.x,event.y);}}
    onMouseMove={event=>{pointer("hover",event.x,event.y);}}
    onScroll={event=>props.markdown.call("menu.scroll",{delta:-(event.deltaY||0)*(event.precise?1:props.config.fontSize*3)})}
    style={{position:"absolute",left:menu().rect[0],top:menu().rect[1],width:menu().rect[2],height:menu().rect[3],pointerEvents:"auto",cursor:"pointer"}}/>
  </div>
 </anchored>}</Show>;
}
