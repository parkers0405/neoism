import assert from "node:assert/strict";
import {connectTest,liveRendererAsTest,type LiveAutomationRenderer} from "@gpuix/solid/automation";
import type {JsonUiDiagramTransform} from "../src/agent/JsonRenderControls";

// Real native input against an isolated graph; no provider or tool requests.
process.argv.push("--diagram");
const fixture=await import("./preview-json-render");
const app=await connectTest(liveRendererAsTest(fixture.renderer as unknown as LiveAutomationRenderer));
let stage="first diagram paint";
const wait=async(predicate:()=>boolean)=>{const until=performance.now()+6000;while(!predicate()){if(performance.now()>until)throw new Error(`Timed out: ${stage}`);await Bun.sleep(30);}};
const control=()=>fixture.observedFrame?.jsonUi?.controls.find(control=>control.kind==="diagram");
const state=()=>control()?.value as JsonUiDiagramTransform|undefined;
try{
  await wait(()=>!!control());
  const id=control()!.id,canvas=app.getByTestId(`json-ui-control:${id}`);
  await canvas.waitFor();await Bun.sleep(200);
  assert.deepEqual(state(),{zoom:1,panX:0,panY:0});
  stage="zoom toolbar";
  await app.getByTestId(`json-ui-diagram:${id}:zoom-in`).click();await wait(()=>state()!.zoom>1);
  const zoomed=state()!.zoom;
  await app.getByTestId(`json-ui-diagram:${id}:zoom-out`).click();await wait(()=>state()!.zoom<zoomed);
  stage="native canvas drag";
  const bounds=await canvas.bounds();assert.ok(bounds);
  await canvas.dragTo({x:bounds.x+bounds.width*0.5+40,y:bounds.y+bounds.height*0.5+25},{steps:8});
  await wait(()=>Math.abs(state()!.panX)>1||Math.abs(state()!.panY)>1);
  stage="fit keyboard shortcut";
  await canvas.press("0");await wait(()=>state()!.zoom===1&&state()!.panX===0&&state()!.panY===0);
  stage="keyboard navigation";
  await canvas.press("right");await wait(()=>state()!.panX!==0);
  await canvas.press("0");await wait(()=>state()!.panX===0&&state()!.panY===0);
  assert.equal(control()!.id,id,"Navigation must preserve the native canvas host");
  assert.equal(Object.hasOwn(fixture.observedFrame?.jsonUi||{},"action"),false);
  console.log("Actual native diagram zoom, drag-to-pan, fit and keyboard navigation verified; no agent request sent.");
  await app.close();process.exit(0);
}catch(error){console.error({stage,control:control()});console.error(error);await app.close();process.exit(1);}