import assert from "node:assert/strict";
import {createRoot} from "@gpuix/solid";
import {FinderController} from "../../src/finder/FinderController";
import {FinderView} from "../../src/finder/FinderView";
import {loadConfig} from "../../src/config";
import {setOverlayViewport} from "../../src/ui/overlay-viewport";
import {variant} from "../../src/daemon/protocol";
import {createRecordingAppRenderer} from "./full-app-recording-renderer";
const host=createRecordingAppRenderer(),root=createRoot(host.renderer),config=loadConfig(["--config","/nonexistent/finder-paint"],{});
const finder=new FinderController({cwd:"/host/project",search:async message=>{
 const[tag,body]=variant(message)!;return tag==="CollectFiles"?{CollectFilesResult:{req_id:0,paths:["src/main.rs"]}}:{SearchFilesResult:{req_id:0,hits:[{path:"src/main.rs",score:100}]}};
}});
const until=async(predicate:()=>boolean)=>{const limit=Date.now()+3000;while(!predicate()){root.flush();if(Date.now()>limit)throw new Error("Finder original paint did not settle");await Bun.sleep(5);}root.flush();};
try{
 setOverlayViewport(host.renderer,{x:300,y:120,width:520,height:480});
 root.render(()=><FinderView controller={finder} config={config} onAccept={()=>{}} onClose={()=>{}}/>);
 await until(()=>finder.frame.rows.length===1&&!finder.frame.animating);
 const labels=finder.frame.commands.flatMap(command=>command.kind==="text"?[command.text]:[]);
 assert.ok(labels.includes("main.rs")&&labels.some(label=>label.includes("src")),"Original Rust file row must paint its actual filename and parent label");
 assert.ok(labels.some(label=>label.includes("fuzzy")),"Original mode badge must be painted by Rust");
 assert.ok(finder.frame.commands.some(command=>command.kind==="text"&&!!command.raster),"Source glyph raster must reach the GPU display list");
 assert.equal(host.find("finder-viewport")!.style.left,300);assert.equal(host.find("finder-viewport")!.style.top,120);
 assert.ok(finder.frame.rect[0]>=0&&finder.frame.rect[1]>=0&&finder.frame.rect[0]+finder.frame.rect[2]<=520+.1&&finder.frame.rect[1]+finder.frame.rect[3]<=480+.1);
 assert.equal(host.find("command-palette")!.style.opacity,0,"Native input must not repaint a different font/label over source pixels");
 const id=finder.id;setOverlayViewport(host.renderer,{x:800,y:180,width:310,height:300});
 await until(()=>!finder.frame.animating&&finder.frame.rect[2]<=294.1);
 assert.equal(finder.id,id);assert.equal(host.find("finder-viewport")!.style.left,800);assert.equal(host.find("finder-viewport")!.style.top,180);
 assert.ok(finder.frame.rect[0]+finder.frame.rect[2]<=310+.1&&finder.frame.rect[1]+finder.frame.rect[3]<=300+.1);
 assert.equal(finder.selectedTarget()?.path,"src/main.rs");
 assert.equal(host.find("finder-result:0")!.props["aria-label"],"src/main.rs");
 console.log(JSON.stringify({noWindow:true,originalRustFinderPaint:true,nativeGlyphRasters:true,transparentNativeInput:true,focusedPaneBodyGeometry:true,viewportResizeRetainsController:true}));
}finally{root.unmount();finder.dispose();setOverlayViewport(host.renderer,undefined);}
