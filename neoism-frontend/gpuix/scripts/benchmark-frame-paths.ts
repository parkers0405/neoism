import {coreCall,NativeEditor} from "../src/native-core";
const timings:Record<string,unknown>={};
function measure(name:string,run:()=>unknown){const samples:number[]=[];let bytes=0;for(let i=0;i<160;i++){const start=performance.now();const result=run();samples.push(performance.now()-start);if(!i)bytes=JSON.stringify(result).length;}samples.sort((a,b)=>a-b);timings[name]={samples:samples.length,totalMs:samples.reduce((a,b)=>a+b,0),p95Ms:samples[Math.floor(samples.length*.95)],wireBytes:bytes};}
const {id}=coreCall<{id:number}>("commands.new",{surface:"editor"});
try{measure("paletteFull",()=>coreCall("commands.frame",{id}));measure("paletteMotion",()=>coreCall("commands.frame",{id,retainRows:true}));}finally{coreCall("commands.dispose",{id});}
const editor=new NativeEditor("budget.rs",Array.from({length:5000},(_,i)=>`let value_${i}: usize = ${i};`).join("\n"),true);
try{
 editor.call("layout",{width:1000,height:800,cellWidth:9,rowHeight:22});
 editor.call("raster.configure",{fontFamily:"Cascadia Code NF",fontSize:14,fontWeight:400,colors:{Plain:"#eeeeee",Keyword:"#ffaaaa"}});
 const args={id:editor.id,x:100,y:80};
 measure("lspPointerFull",()=>coreCall("editor.lsp.pointer",args));
 measure("lspPointerOverlay",()=>coreCall("editor.lsp.pointer",{...args,overlayOnly:true}));
}finally{editor.dispose();}
console.log(JSON.stringify({scope:"native bridge CPU and wire size; no window or daemon",timings},null,2));
