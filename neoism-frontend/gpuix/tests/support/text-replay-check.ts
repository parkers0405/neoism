import {createHash} from 'node:crypto';
import {coreCall} from '../../src/native-core';
const {id}=coreCall<{id:number}>('agenttimeline.new',{sessionId:'replay',scale:1.357,fontFamily:'Cascadia Code NF',fontWeight:700,canvasPaint:true});
const call=(op:string,args:Record<string,unknown>={})=>coreCall<any>(`agenttimeline.${op}`,{id,...args});
try{
 const messages=Array.from({length:60},(_,i)=>({id:`row-${i}`,role:'Assistant',kind:'Assistant',text:`Message ${i}: **bold glyphs** and *italic words* and [a link](https://example.com/${i}).\n\n- repeated text to wrap across the column with original shaped metrics\n- second list entry\n\n\`inline code\` and more prose to scroll.`}));
 call('event',{message:{HistoryChunk:{session_id:'replay',messages,next_cursor:null}}});call('layout',{width:1100,height:900,contentBottom:590});
 await Bun.sleep(500);for(let i=0;i<30;i++)call('frame');
 const times:number[]=[];for(let i=0;i<600;i++){const start=performance.now();call('frame');times.push(performance.now()-start);}
 const frame=call('frame',{includeCommands:true,compactPaint:false});
 const geometry=frame.commands.map((command:any)=>{const {raster,...rest}=command;return {...rest,...(raster?{raster:{width:raster.width,height:raster.height,scale:raster.scale,dx:raster.dx,dy:raster.dy}}:{})};});
 times.sort((a,b)=>a-b);
 console.log(JSON.stringify({hash:createHash('sha256').update(JSON.stringify(geometry)).digest('hex'),commands:geometry.length,p50:times[300],p95:times[570],totalMs:times.reduce((a,b)=>a+b,0)}));
}finally{call('dispose');}
