import {NativeEditor} from "../src/native-core";
const editor=new NativeEditor("cursor-benchmark.rs",Array.from({length:2000},(_,i)=>`let value_${i}: usize = ${i};`).join("\n"),true);
try{
 editor.call("layout",{width:1000,height:800,cellWidth:9,rowHeight:22});
 editor.call("raster.configure",{fontFamily:"Cascadia Code NF",fontSize:14,fontWeight:400,colors:{Plain:"#eeeeee",Keyword:"#ffaaaa"}});
 const timings:Record<string,unknown>={};
 // Both paths run against the same editor, native library and viewport.
 for(const op of ["frame","cursor-frame"]){
  const samples:number[]=[];editor.call("key",{key:"l",char:"l"});
  const rows=editor.frame.lines;
  for(let i=0;i<240;i++){const start=performance.now();editor.call(op);samples.push(performance.now()-start);}
  const sorted=samples.toSorted((a,b)=>a-b);
  timings[op]={count:samples.length,totalMs:samples.reduce((a,b)=>a+b,0),p95Ms:sorted[Math.floor(sorted.length*.95)],rowsRetained:editor.frame.lines===rows};
 }
 console.log(JSON.stringify({noWindow:true,timings},null,2));
}finally{editor.dispose();}
