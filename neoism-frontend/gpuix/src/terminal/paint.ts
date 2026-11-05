import type {TerminalFrame,CellRun} from "./buffer";
import {cssColor,type CanvasOp} from "../ui/canvas";

/** Lift dim block-output colors on dark themes without flattening ANSI hues. */
export function blockOutputForeground(background: string): (color: string) => string {
 const rgb=(color:string)=>[1,3,5].map(offset=>parseInt(color.slice(offset,offset+2),16));
 const luminance=(channels:number[])=>channels.reduce((sum,value,index)=>{
  const c=value/255; return sum+(c<=.04045?c/12.92:((c+.055)/1.055)**2.4)*[.2126,.7152,.0722][index];
 },0);
 if(!/^#[\da-f]{6}$/i.test(background))return color=>color;
 const bg=luminance(rgb(background)),cache=new Map<string,string>();
 if(bg>=.175)return color=>color;
 return color=>{
  const cached=cache.get(color);if(cached)return cached;
  if(!/^#[\da-f]{6}$/i.test(color))return color;
  const channels=rgb(color),contrast=(shade:number[])=>{const fg=luminance(shade);return(Math.max(fg,bg)+.05)/(Math.min(fg,bg)+.05);};
  let result=color;
  if(contrast(channels)<4.5){
   const blend=(amount:number)=>channels.map(c=>Math.round(c+(255-c)*amount));
   let low=0,high=1;
   for(let i=0;i<12;i++){const mid=(low+high)/2;if(contrast(blend(mid))>=4.5)high=mid;else low=mid;}
   result="#"+blend(high).map(c=>c.toString(16).padStart(2,"0")).join("");
  }
  if(cache.size>=512)cache.clear();cache.set(color,result);return result;
 };
}

/** One display list replaces the per-run layout tree. Reserve one row above
 * the viewport for fractional scrollback; the host translates the list. */
export function terminalPaintOps(frame:Pick<TerminalFrame,"lines"|"extra">,font:{fontFamily:string;fontSize:number;fontWeight:number},cell:number,rowHeight:number,foreground?:(color:string)=>string):CanvasOp[]{
 const ops:CanvasOp[]=[];
 const row=(runs:readonly CellRun[],index:number)=>{
  const y=(index+1)*rowHeight;
  for(const run of runs){
   const x=run.column*cell,width=run.width*cell,fg=foreground&&!run.bg?foreground(run.fg):run.fg;
   if(run.bg)ops.push({k:"r",b:[x,y,width,rowHeight],c:cssColor(run.bg)});
   if(run.text.trim())ops.push({k:"t",x,y,t:run.text,s:font.fontSize,f:font.fontFamily,w:run.bold?700:font.fontWeight,i:run.italic,c:cssColor(fg),lh:rowHeight});
   if(run.underline)ops.push({k:"r",b:[x,y+rowHeight-2,width,1],c:cssColor(fg)});
  }
 };
 frame.lines.forEach(row);
 if(frame.extra)row(frame.extra.line,frame.extra.above?-1:frame.lines.length);
 return ops;
}
