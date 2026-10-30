import assert from "node:assert/strict";
import {createRoot} from "@gpuix/solid";
import {createSignal,createEffect,createRoot as solidRoot} from "solid-js";
import {runFrameBatch} from "../../src/animation/display-clock";
import {usePresentationScheduler} from "../../src/animation/frame-scheduler";
import {CommandController} from "../../src/commands/CommandController";
import {CommandView} from "../../src/commands/CommandView";
import {createRecordingAppRenderer} from "./full-app-recording-renderer";
import {loadConfig} from "../../src/config";
const host=createRecordingAppRenderer(),root=createRoot(host.renderer),config=loadConfig(["--config","/nonexistent/command-budget"],{});
const pending=new Set<()=>void>(),restore=usePresentationScheduler(callback=>{pending.add(callback);return()=>pending.delete(callback);});
const controller=new CommandController({surface:"editor"});
let textWrites=0,frames=0;
const apply=host.renderer.applyBatch.bind(host.renderer);
host.renderer.applyBatch=json=>{for(const[op,id]of JSON.parse(json))if((op==="setText"||op==="setStyle")&&host.nodes.get(id)?.type==="text")textWrites++;return apply(json);};
async function settle(){const deadline=performance.now()+3000;while(pending.size){await Bun.sleep(8);const jobs=[...pending];pending.clear();runFrameBatch(jobs,error=>{throw error;});root.flush();frames++;if(performance.now()>deadline)throw Error("palette did not settle");}}
try{
 root.render(()=><CommandView controller={controller} config={config} onAccept={()=>{}} onClose={()=>{}}/>);await host.pump(root);await settle();
 controller.move(2);root.flush();textWrites=frames=0;const rows=controller.frame.rows;await settle();
 assert.ok(frames>5);assert.equal(controller.frame.rows,rows);assert.equal(textWrites,0,"Animation must not restyle/rewrite result text");
 const animationTextWrites=textWrites;
 controller.query("Write File");await host.pump(root);assert.ok(host.find("command:SaveDocument"));assert.equal(controller.target()?.action,"SaveDocument");
 let computations=0,dispose=()=>{},setA!:(v:number)=>void,setB!:(v:number)=>void;
 solidRoot(d=>{dispose=d;const[a,sa]=createSignal(0),[b,sb]=createSignal(0);setA=sa;setB=sb;createEffect(()=>{a();b();computations++;});});
 computations=0;runFrameBatch([()=>setA(1),()=>setB(1)],error=>{throw error;});assert.equal(computations,1);dispose();
 console.log(JSON.stringify({noWindow:true,frames,textWrites:animationTextWrites,sharedComputations:computations,queryPreserved:true}));
}finally{root.unmount();controller.dispose();restore();}
