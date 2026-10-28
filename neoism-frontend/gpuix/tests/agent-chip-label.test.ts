import {expect,test} from "bun:test";
import {fileURLToPath} from "node:url";
import {coreCall} from "../src/native-core";
import type {PaintCommand} from "../src/ui/PaintLayer";
interface Frame {commands:PaintCommand[];width:number;height:number;animating:boolean;phase:string}
const texts=(frame:Frame)=>frame.commands.flatMap(command=>command.kind==="text"?[command]:[]);
test("original Agent chip loading clock settles without restarting, mode changes scramble and Plan has its original accent",async()=>{
 const id=coreCall<{id:number}>("agentcomposer.new",{}).id;
 const frame=(label:string,loading=false,reducedMotion=false)=>coreCall<Frame>("agentcomposer.chip-frame",{id,kind:"agent",label,loading,reducedMotion,fontSize:14,theme:"pastel_dark"});
 try{
  const loading=frame("Build",true);expect(loading.phase).toBe("loading");expect(loading.animating).toBe(true);
  expect(texts(loading).map(command=>command.text).join("")).not.toBe("Build");
  expect(new Set(texts(loading).map(command=>JSON.stringify(command.color))).size).toBeGreaterThan(1);
  const ready=frame("Build");expect(ready.phase).toBe("settling");expect(ready.width).toBe(loading.width);
  await Bun.sleep(350);const settled=frame("Build");expect(settled.phase).toBe("idle");expect(texts(settled).map(command=>command.text).join("")).toBe("Build");
  expect(frame("Build").animating).toBe(false);
  const changed=frame("Plan");expect(changed.phase).toBe("changing");expect(changed.animating).toBe(true);
  const reduced=frame("Plan",false,true);expect(reduced.animating).toBe(false);expect(texts(reduced).map(command=>command.text).join("")).toBe("Plan");
  expect(texts(reduced)[0].color).not.toEqual(texts(settled)[0].color);
  await Bun.sleep(330);expect(frame("Plan").phase).toBe("idle");
 }finally{coreCall("agentcomposer.dispose",{id});}
});
test("actual Solid Agent chips paint the canonical per-letter color animation and retain the settled host on remount",async()=>{
 const child=Bun.spawn([process.execPath,"run","tests/support/agent-chip-label-check.tsx"],{cwd:fileURLToPath(new URL("..",import.meta.url)),stdout:"pipe",stderr:"pipe"});
 const [status,stdout,stderr]=await Promise.all([child.exited,new Response(child.stdout).text(),new Response(child.stderr).text()]);
 expect({status,stderr}).toEqual({status:0,stderr:""});expect(JSON.parse(stdout)).toEqual({noWindow:true,originalLoadingColors:true,originalModeScramble:true,settledRemountNoReplay:true,noTextureChurn:true,reducedMotion:true});
},10000);
