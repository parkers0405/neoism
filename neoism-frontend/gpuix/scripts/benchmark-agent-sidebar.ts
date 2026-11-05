// Exercise the real sidebar renderer/FFI and JS canvas translation, no window.
import {callNativeCore} from "../../native-bridge";
import {rasterKey,type CanvasImages,type CanvasRaster} from "../src/ui/canvas";
import {paintOps,type PaintCommand} from "../src/ui/PaintLayer";
import {loadConfig} from "../src/config";
const count=Number(process.argv[2]||250),family="Cascadia Code NF";
const config={...loadConfig([],{}),fontFamily:family,fontWeight:400};
const images={use:(source:number,raster:{id:number})=>rasterKey(source,raster.id),commit(){}} as unknown as CanvasImages;
const {id}=callNativeCore<{id:number}>("agentsidepanel.new",{directory:"/profile/sidebar",theme:"pastel_dark",scale:1,fontFamily:family,fontWeight:400});
try {
  callNativeCore("agentsidepanel.frame",{id,width:300,height:640});
  const threads=Array.from({length:50},(_,i)=>({session_id:`chat-${i}`,title:`Implement workspace feature ${i} with regression coverage`,directory:"/profile/sidebar",updated_at:Date.now()-i*1000}));
  callNativeCore("agentsidepanel.event",{id,message:{ThreadList:{threads,requested_cursor:null,next_cursor:null}}});
  for(let i=0;i<4;i++)callNativeCore("agentsidepanel.event",{id,message:{StreamingState:{session_id:`chat-${i}`,state:"Generating",label:null}}});
  for(const canvasPaint of [false,true]){
    const times:number[]=[],native:number[]=[],decode:number[]=[];let bytes=0,commands=0,usedCanvas=false;
    const profile={start:()=>performance.now(),end:(name:string,start:number,volume=0)=>{
      if(name.startsWith("native.agentsidepanel."))native.push(performance.now()-start);
      if(name==="core.decode")decode.push(performance.now()-start);
      if(name==="core.readResponse")bytes+=volume;
    }};
    for(let i=0;i<count+20;i++){
      const start=performance.now();
      // An unchanged focus state uses the retained legacy Value-tree path,
      // painting the same catalog. Compare both transports in the same build.
      const frame=callNativeCore<{commands:PaintCommand[];canvas?:{commands:string;images:Array<{source:number;raster:CanvasRaster}>}}>(canvasPaint?"agentsidepanel.frame":"agentsidepanel.focus",{id,focused:false,canvasPaint},i<20?undefined:profile);
      commands=frame.commands.length;usedCanvas=!!frame.canvas;
      const encoded=frame.canvas?.commands||JSON.stringify(paintOps(frame.commands,config,images));
      if(!encoded.length)throw Error("Empty sidebar paint");
      if(i>=20)times.push(performance.now()-start);
    }
    const summary=(values:number[])=>{values.sort((a,b)=>a-b);return{meanMs:values.reduce((a,b)=>a+b,0)/values.length,p50Ms:values[Math.floor(values.length*.5)],p95Ms:values[Math.floor(values.length*.95)]};};
    console.log(JSON.stringify({canvasPaint,usedCanvas,frames:count,rawCommands:commands,bytesPerFrame:bytes/count,total:summary(times),native:summary(native),decode:summary(decode)}));
  }
} finally {callNativeCore("agentsidepanel.dispose",{id});}