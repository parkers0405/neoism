import assert from "node:assert/strict";
import {connectTest,liveRendererAsTest,type LiveAutomationRenderer} from "@gpuix/solid/automation";
import type {JsonUiControl} from "../src/agent/JsonRenderControls";

// Explicit manual smoke test in an isolated native window. Inputs go through
// GPUI's native event dispatch; JSON widgets have no agent submission path.
process.argv.push("--interactive");
const fixture=await import("./preview-json-render");
const app=await connectTest(liveRendererAsTest(fixture.renderer as unknown as LiveAutomationRenderer));
let stage="first paint";
const wait=async(predicate:()=>boolean)=>{const until=performance.now()+6000;while(!predicate()){if(performance.now()>until)throw new Error(`Timed out: ${stage}`);await Bun.sleep(30);}};
const controls=()=>fixture.observedFrame?.jsonUi?.controls||[];
const control=(label:string,kind?:JsonUiControl["kind"])=>{const result=controls().find(c=>c.label===label&&(!kind||c.kind===kind));assert.ok(result,`${stage}: missing ${label}`);return result;};
const widget=(label:string,kind?:JsonUiControl["kind"])=>app.getByTestId(`json-ui-control:${control(label,kind).id}`);
const body=()=>fixture.observedFrame?.commands.filter(c=>c.kind==="text").map(c=>c.text).join(" ")||"";
try{
  await wait(()=>controls().some(c=>c.kind==="chart"));
  stage="chart hover";
  const chart=controls().find(c=>c.kind==="chart")!;
  await app.getByTestId(`json-ui-control:${chart.id}`).waitFor();await Bun.sleep(200);
  await app.getByTestId(`json-ui-control:${chart.id}`).hover();
  // Real compositor pointer-leave can follow synthetic movement immediately.
  // Assert the native hover frame was emitted, not that it stays pinned.
  await wait(()=>fixture.observedText.has("Tue: 18"));
  stage="chart keyboard selection";
  await app.getByTestId(`json-ui-control:${chart.id}`).press("right");
  await wait(()=>controls().find(c=>c.id===chart.id)?.value===1);
  stage="collapsible";
  await widget("Search team notes").click();await wait(()=>!controls().some(c=>c.label==="Search notes locally"));
  await widget("Search team notes").click();await wait(()=>controls().some(c=>c.label==="Search notes locally"));
  stage="native filter typing";
  await widget("Search notes locally").fill("sales");await wait(()=>control("Search notes locally").value==="sales"&&!body().includes("Support:"));
  await widget("Search notes locally").fill("");await wait(()=>control("Search notes locally").value==="");
  stage="tabs";
  await widget("Options").click();await wait(()=>controls().some(c=>c.label==="Topic"));
  stage="native textarea editing";
  await widget("Topic").fill("Check churn risk");await wait(()=>control("Topic").value==="Check churn risk");
  stage="select menu";
  await widget("Team").click();await app.getByTestId("json-ui-options").waitFor();await app.getByTestId("json-ui-option:1").click();await wait(()=>control("Team").value==="Support");
  stage="checkbox";
  await widget("Include team notes").click();await wait(()=>control("Include team notes").value===true);
  assert.equal(Object.hasOwn(fixture.observedFrame?.jsonUi||{},"action"),false);
  console.log("Actual native input, chart, tabs, filter and local fields verified; no request UI.");
  await app.close();process.exit(0);
}catch(error){const chart=controls().find(c=>c.kind==="chart");console.error(JSON.stringify({stage,pointer:fixture.lastPointer,chartBounds:chart?await app.getByTestId(`json-ui-control:${chart.id}`).bounds():undefined,body:body(),controls:controls().map(c=>({id:c.id,kind:c.kind,label:c.label,value:c.value,rect:c.rect}))}));console.error(error);await app.close();process.exit(1);}