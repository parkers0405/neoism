import {createHash} from "node:crypto";
import {toArrayBuffer,type Pointer} from "bun:ffi";
import {coreCall,NativeEditor} from "../src/native-core";
import {NativeMarkdown} from "../src/markdown/NativeMarkdown";
import {loadConfig} from "../src/config";
// In-memory native workload: no window, daemon, files opened or user state changed.
// Run in separate processes with NEOISM_GPUIX_CORE to compare build artifacts.
const timings:Record<string,{count:number;totalMs:number;p95Ms:number}>={};
function measure(name:string,count:number,task:(i:number)=>void){const times:number[]=[];for(let i=0;i<count;i++){const start=performance.now();task(i);times.push(performance.now()-start);}const sorted=times.toSorted((a,b)=>a-b);timings[name]={count,totalMs:times.reduce((a,b)=>a+b,0),p95Ms:sorted[Math.min(count-1,Math.floor(count*.95))]};}
const config={...loadConfig(["--config","/nonexistent/native-render-benchmark"],{}),vim:true};
const hash=createHash("sha256");
const {id}=coreCall<{id:number}>("treeglyphs.new",{family:config.fontFamily,weight:400,fonts:["Symbols Nerd Font Mono"]});
try{measure("sidebarUncached",200,i=>{const row=coreCall<{raster:{ptr:number;len:number};label:string}>("treeglyphs.row",{id,name:`module_${i}.rs`,width:300,scale:1,chevron:"",icon:"\uf15b",labelColor:"#dddddd",iconColor:"#bbbbbb",chevronColor:"#aaaaaa"});hash.update(row.label);hash.update(new Uint8Array(toArrayBuffer(row.raster.ptr as unknown as Pointer,0,row.raster.len)));});}finally{coreCall("treeglyphs.dispose",{id});}
const text="# Render benchmark\n\n"+Array.from({length:80},(_,i)=>`Paragraph ${i} with **rich text** and a [local link](README.md).`).join("\n\n");
const editor=new NativeEditor("benchmark.md",text,true);
let markdown:NativeMarkdown|undefined;
try{measure("markdownOpen",1,()=>{markdown=new NativeMarkdown(editor,config,"/benchmark.md");markdown.call("layout",{width:900,height:700});});measure("markdownHover",120,i=>{markdown!.call("hover",{x:100+i,y:150+i%30});});hash.update(markdown!.text());}finally{markdown?.dispose();editor.dispose();}
console.log(JSON.stringify({noWindow:true,library:process.env.NEOISM_GPUIX_CORE||"default",outputHash:hash.digest("hex"),timings},null,2));
