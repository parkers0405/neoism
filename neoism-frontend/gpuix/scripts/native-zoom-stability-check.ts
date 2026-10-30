import {copyFileSync,mkdtempSync,rmSync,writeFileSync} from "node:fs";
import {tmpdir} from "node:os";
import {join} from "node:path";
import type {TreeNode} from "@gpuix/solid/automation";
import {launchNative} from "../tests/support/native-launch";
import {createAgentDaemon} from "../tests/support/agent-daemon";

const directory=mkdtempSync(join(tmpdir(),"neoism-zoom-stability-")),config=join(directory,"config.json"),recovery=join(directory,"state.json"),wallpaper=join(directory,"wallpaper.png");
copyFileSync(join(process.cwd(),"../shared/assets/icons/neoism.png"),wallpaper);
const configuration={appearance:{fonts:{family:"Cascadia Code NF",size:14}},editor:{"vim-mode":true},ui:{window:{"background-image":{path:wallpaper,opacity:.45},"background-image-disabled":false}}};
writeFileSync(config,JSON.stringify(configuration));
const server=createAgentDaemon({configuration,directoryEntries:{"":[{name:"README.md",is_dir:false},{name:"code.rs",is_dir:false},{name:"assets",is_dir:true}]}});
const packaged=process.argv.includes("--packaged");
const app=await launchNative({command:packaged?join(process.cwd(),"dist/neoism"):process.execPath,args:[...(packaged?[]:["src/main.tsx"]),"--daemon",server.endpoint,"--config",config],cwd:process.cwd(),env:{...process.env,NEOISM_GPUIX_STATE_PATH:recovery}});
const flatten=(nodes:TreeNode[]):TreeNode[]=>nodes.flatMap(node=>[node,...flatten(node.children||[])]);
const snapshot=async()=>{const{tree}=await app.call("getTree",{});return flatten(tree?[tree]:[]);};
const until=async(predicate:()=>boolean|Promise<boolean>)=>{const deadline=Date.now()+15000;while(!await predicate()){if(Date.now()>deadline)throw new Error("Zoom appearance did not converge");await Bun.sleep(5);}};
const find=(nodes:TreeNode[],id:string)=>{const value=nodes.find(node=>node.testId===id);if(!value)throw new Error(`Missing ${id} during font zoom`);return value;};
const prop=(node:TreeNode,key:string)=>node.customProps?.[key];
let stage="initial real wallpaper, Welcome and revealed file tree";
try{
 await app.getByTestId("splash-wordmark").waitFor({timeoutMs:20000});await app.getByTestId("file:code.rs").waitFor();await app.getByTestId("app-wallpaper").waitFor();await Bun.sleep(2000);
 const baseline=await snapshot(),shell=find(baseline,"app-shell"),background=find(baseline,"app-wallpaper"),content=find(baseline,"app-content"),tree=find(baseline,"file-tree");
 const starts=new Map(baseline.filter(node=>node.testId?.startsWith("file:")).map(node=>[node.testId,prop(node,"data-reveal-start")]));
 if(starts.size!==3||[...starts.values()].some(value=>typeof value!=="number"))throw new Error("Missing original tree reveal timestamps");
 const treeId=prop(tree,"data-tree-id");if(treeId===undefined)throw new Error("File tree does not expose its retained native host identity");
 const observed=new Map<number,number>(),errors:string[]=[],evidence:Array<{generation:number;fontSize:number}>=[];let running=true;
 const inspect=(nodes:TreeNode[])=>{
  if(find(nodes,"app-shell").id!==shell.id||find(nodes,"app-wallpaper").id!==background.id)throw new Error("Font zoom replaced the window shell or wallpaper image");
  const current=find(nodes,"app-content"),fontSize=Number(prop(current,"data-font-size"));
  if(prop(find(nodes,"file-tree"),"data-tree-id")!==treeId)throw new Error("Font zoom reset the native file-tree host");
  for(const row of nodes.filter(node=>node.testId?.startsWith("file:"))){
   if(prop(row,"data-reveal-start")!==starts.get(row.testId))throw new Error(`Font zoom restarted ${row.testId}'s reveal clock`);
   if(current.id!==content.id&&Number(prop(row,"data-reveal-opacity"))!==1)throw new Error(`New generation ${fontSize}px remounted ${row.testId} with a reveal animation`);
   if(current.id!==content.id&&typeof row.style?.opacity==="number"&&row.style.opacity<.999)throw new Error(`New generation ${fontSize}px hid ${row.testId}`);
  }
  const logo=find(nodes,"splash-wordmark"),paint=flatten(logo.children||[]).find(node=>node.type==="canvas");
  if(!paint?.bounds||paint.bounds.width<=0||paint.bounds.height<=0)throw new Error(`Font zoom produced an empty wordmark canvas at ${fontSize}px`);
  if(!observed.has(current.id)){observed.set(current.id,fontSize);evidence.push({generation:current.id,fontSize});}
 };
 inspect(baseline);
 stage="six rapid Ctrl +/- cycles retain wallpaper, native tree and completed reveal clocks";
 const poll=(async()=>{while(running){try{inspect(await snapshot());}catch(error){errors.push(String(error));}await Bun.sleep(3);}})();
 try{
  for(let cycle=0;cycle<6;cycle++){await app.call("keystrokes",{keys:"ctrl-="});await app.call("keystrokes",{keys:"ctrl--"});}
  await until(()=>server.configRequests.filter(request=>request.tag==="SetSetting"&&request.body.key==="appearance.fonts.size").length===12);
  await until(async()=>{const nodes=await snapshot();inspect(nodes);return Number(prop(find(nodes,"app-content"),"data-font-size"))===14&&observed.size>=3;});await Bun.sleep(100);
 }finally{running=false;await poll;}
 if(errors.length)throw new Error(errors.join("\n"));
 if(server.ptyRequests.some(request=>request.tag==="CreatePty"))throw new Error("Font zoom created a terminal");
 console.log(JSON.stringify({packaged,rapidZoomCycles:6,settingsWrites:12,persistentShell:shell.id,persistentWallpaper:background.id,persistentNativeTree:treeId,revealClocksPreserved:true,allObservedNewRowsAlreadyRevealed:true,positiveWordmarkPaintGeometry:true,evidence}));
}catch(error){console.error(`Native zoom stability failed at ${stage}`);console.error(JSON.stringify({configRequests:server.configRequests,nodes:(await snapshot()).filter(node=>["app-shell","app-wallpaper","app-content","file-tree","splash-wordmark"].includes(node.testId||"")||node.testId?.startsWith("file:")).map(node=>({id:node.id,testId:node.testId,style:node.style,customProps:node.customProps,bounds:node.bounds}))}));throw error;}
finally{await app.close();server.stop();rmSync(directory,{recursive:true,force:true});}
