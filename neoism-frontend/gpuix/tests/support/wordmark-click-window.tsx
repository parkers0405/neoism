// Real display-thread regression: math/recording tests cannot detect a paint deadlock.
import assert from "node:assert/strict";
import {createRenderer,render} from "@gpuix/solid";
import {startFrameLoop} from "@gpuix/native/runtime";
import {App,InProcessBackend,liveRendererAsTest} from "@gpuix/native/automation";
import {mock} from "bun:test";
import * as native from "../../src/native-core";
import {installDisplayClock} from "../../src/animation/display-clock";
import {loadConfig} from "../../src/config";

const config={...loadConfig(["--config","/nonexistent/wordmark-click"],{}),reducedMotion:false,splashAutoAnimate:false};
const actual=native.coreCall;let calls=0,maxCall=0,maxCommands=0;
mock.module("../../src/native-core",()=>({...native,coreCall:(op:string,args?:Record<string,unknown>)=>{
 const started=performance.now(),value=actual<any>(op,args);
 if(op==="wordmark.frame"){calls++;maxCall=Math.max(maxCall,performance.now()-started);maxCommands=Math.max(maxCommands,value.commands.length);}
 return value;
}}));
const {AnimatedWordmark,WordmarkHosts}=await import("../../src/ui/AnimatedWordmark");
const renderer=createRenderer();
renderer.init({title:"Neoism splash click regression",appId:"ai.neoism.gpuix.wordmark-check",width:900,height:500,focus:false});
const hosts=new WordmarkHosts(),clock=installDisplayClock(renderer),loop=startFrameLoop(renderer);
let sentinels=0;
const root=render(()=><div style={{width:"100%",height:"100%",backgroundColor:"#121218"}}>
 <AnimatedWordmark config={config} hosts={hosts} startupKey="home" kind="splash" testId="home-logo" style={{position:"absolute",left:100,top:70,width:600,height:100}}/>
 <AnimatedWordmark config={config} hosts={hosts} startupKey="agent" kind="agent" testId="agent-logo" style={{position:"absolute",left:100,top:240,width:600,height:100}}/>
 <div testId="sentinel" onClick={()=>sentinels++} style={{position:"absolute",left:20,top:400,width:80,height:50,backgroundColor:"#447799"}}/>
</div>,{renderer});
const app=new App(new InProcessBackend(liveRendererAsTest(renderer)));
try{
 await Bun.sleep(300);
 for(const target of ["home-logo","agent-logo"]){
  const bounds=await app.getByTestId(target).bounds();
  for(let click=0;click<16;click++){
   const started=performance.now();calls=0;maxCall=0;maxCommands=0;console.log(`click ${target} ${click}`);
   const before=renderer.getDebugFrameOverlayStats().frames;
   renderer.simulateClick(bounds.x+bounds.width/2,bounds.y+bounds.height/2,0);
   await Bun.sleep(130);
   assert.ok(renderer.getDebugFrameOverlayStats().frames>before,"display must keep presenting after replay");
   console.log(`completed ${target} ${click}: ${Math.round(performance.now()-started)}ms ${JSON.stringify({calls,maxCall,maxCommands})}`);
   assert.ok(performance.now()-started<500,"a replay must not starve the 130ms input/timer probe for the whole effect");
  }
  const before=sentinels;
  await app.getByTestId("sentinel").click();await Bun.sleep(80);
  assert.ok(sentinels>before,"other UI controls must still receive input");
 }
 console.log("PASS: Home and Agent splash clicks keep painting and accepting input");
}catch(error){console.error(error);process.exitCode=1;}
finally{root.unmount();hosts.dispose();loop.stop();clock.dispose();}
process.exit(process.exitCode||0);