// Real native-window startup check: the JS event loop must remain available
// for the daemon handshake while graphics initialization is pending.
import assert from "node:assert/strict";
import { GpuixRenderer } from "@gpuix/native";
import {prepareBundledFonts} from "../src/bundled-fonts";

const renderer = new GpuixRenderer() as GpuixRenderer & {
  initAsync(options?: Parameters<GpuixRenderer["init"]>[0]): Promise<void>;
  setStartupFonts(fonts:Buffer[]):void;
  setStartupRenderingReady(ready:boolean):void;
  getRenderedFrameSample():number[];
  getPresentedFrameSample():number[]|null;
  setFrameWakeCallback(callback?:()=>void):boolean;
  requestFrameWake():boolean;
};
let pending = true, turns = 0;
const timer = setInterval(() => { if (pending) turns++; }, 1);
try {
  assert.throws(()=>renderer.setStartupFonts([Buffer.from("invalid font")]), /not a TrueType or OpenType font/);
  prepareBundledFonts(renderer);
  if(process.platform!=="darwin")renderer.setStartupRenderingReady(false);
  const ready = renderer.initAsync({ title: "Neoism startup verification", width: 720, height: 480 });
  let wakes=0;
  assert.equal(renderer.setFrameWakeCallback(()=>{wakes++;}),true);
  assert.equal(renderer.requestFrameWake(),true,"a display wake can be queued during initialization");
  renderer.applyBatch(JSON.stringify([
    ["createElement",1,"div"],["setStyle",1,{width:300,height:100,backgroundColor:"#18202a"}],
    ["createElement",2,"text"],["setText",2,"Startup content"],
    ["appendChild",1,2],["setRoot",1],
  ]));
  renderer.setWindowTitle("Neoism startup verification");
  assert.throws(()=>renderer.setStartupFonts([]), /before initialization/);
  await assert.rejects(renderer.initAsync(), /already initialized/);
  await ready;
  pending = false;
  if(process.platform!=="darwin"){
    // Layout/glyph preparation may run offscreen, but a partial root must
    // never reach the compositor while presentation is paused.
    await Bun.sleep(40);
    assert.equal(renderer.getPresentedFrameSample()?.[0]||0,0,"startup must not present a partial root");
    renderer.setStartupRenderingReady(true);
  }
  const deadline=Date.now()+3000;
  while(!wakes&&Date.now()<deadline)await Bun.sleep(5);
  assert.ok(wakes>0,"the display wake queued before startup must be delivered");
  assert.equal(renderer.isInitialized(), true);
  const size=renderer.getWindowSize();
  assert.ok(size.width>0&&size.height>0,"startup must publish the real native viewport");
  for(let i=0;i<10000;i++){
    const current=renderer.getWindowSize();
    assert.ok(current.width>0&&current.height>0,"dimension reads must remain coherent while native frames run");
  }
  if (process.platform !== "darwin") assert.ok(turns > 0, "native startup blocked JavaScript");
  await assert.rejects(renderer.initAsync(), /already initialized/);
  console.log(JSON.stringify({ initialized: true, eventLoopTurnsDuringInit: turns, duplicateInitRejected: true, startupFonts:true, queuedStartupWake:true, viewport:size }));
} finally {
  clearInterval(timer);
}
process.exit(0);
