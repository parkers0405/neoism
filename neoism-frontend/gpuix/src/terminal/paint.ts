import type {TerminalFrame,CellRun} from "./buffer";
import {cssColor,type CanvasOp} from "../ui/canvas";

/** One display list replaces the per-run layout tree. Reserve one row above
 * the viewport for fractional scrollback; the host translates the list. */
export function terminalPaintOps(frame:Pick<TerminalFrame,"lines"|"extra">,font:{fontFamily:string;fontSize:number;fontWeight:number},cell:number,rowHeight:number):CanvasOp[]{
 const ops:CanvasOp[]=[];
 const row=(runs:readonly CellRun[],index:number)=>{
  const y=(index+1)*rowHeight;
  for(const run of runs){
   const x=run.column*cell,width=run.width*cell;
   if(run.bg)ops.push({k:"r",b:[x,y,width,rowHeight],c:cssColor(run.bg)});
   if(run.text.trim())ops.push({k:"t",x,y,t:run.text,s:font.fontSize,f:font.fontFamily,w:run.bold?700:font.fontWeight,i:run.italic,c:cssColor(run.fg),lh:rowHeight});
   if(run.underline)ops.push({k:"r",b:[x,y+rowHeight-2,width,1],c:cssColor(run.fg)});
  }
 };
 frame.lines.forEach(row);
 if(frame.extra)row(frame.extra.line,frame.extra.above?-1:frame.lines.length);
 return ops;
}
