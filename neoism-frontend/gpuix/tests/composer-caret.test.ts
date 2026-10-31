import {expect,test} from "bun:test";
import {mkdtempSync,rmSync} from "node:fs";
import {join} from "node:path";
import {tmpdir} from "node:os";
import {fileURLToPath} from "node:url";
import {composerCaretGeometry} from "../src/agent/ComposerCaret";
import {coreCall} from "../src/native-core";
import {verifiedDisplayBinding} from "../src/display-binding";
const cwd=fileURLToPath(new URL("..",import.meta.url));
test("actual source geometry module preserves native wrapped/scrolled insertion points, Unicode byte offsets and selection visibility",async()=>{
 const temporary=mkdtempSync(join(tmpdir(),"neoism-native-caret-"));
 try{
  const output=join(temporary,"caret-tests"),source=join(cwd,"artifacts/gpuix-pacing-source/packages/native/src/custom_elements/input_caret_geometry.rs");
  const build=Bun.spawn(["rustc","--edition=2021","--test",source,"-o",output],{stdout:"pipe",stderr:"pipe"});
  expect({status:await build.exited,stderr:await new Response(build.stderr).text()}).toEqual({status:0,stderr:""});
  const run=Bun.spawn([output],{stdout:"pipe",stderr:"pipe"});const [status,stdout]=await Promise.all([run.exited,new Response(run.stdout).text()]);
  expect(status).toBe(0);expect(stdout).toContain("5 passed; 0 failed");
 }finally{rmSync(temporary,{recursive:true,force:true});}
});
test("composer uses original Rust block dimensions, true shaped coordinates and safe event parsing",()=>{
 const event={x:120.25,y:240.5,value:JSON.stringify({fontSize:16,remainingWidth:100,visible:true,offset:11,selectionStart:11,selectionEnd:11})};
 const decoded=composerCaretGeometry(event)!;expect(decoded).toMatchObject({x:120.25,y:240.5,offset:11,visible:true});
 expect(coreCall<number[]>("agentcomposer.caret-rect",{...decoded,scale:1})).toEqual([120.25,240.5,expect.closeTo(9.6,4),20]);
 expect(coreCall<number[]>("agentcomposer.caret-rect",{...decoded,remainingWidth:1,scale:1.5})).toEqual([120.25,240.5,1,30]);
 expect(composerCaretGeometry({x:0,y:0,value:"not-json"})).toBeUndefined();expect(composerCaretGeometry({value:event.value})).toBeUndefined();
});
test("real Solid native textarea custom event drives the original GPU block trail and cross-zone handoff without windows",async()=>{
 const child=Bun.spawn([process.execPath,"run","tests/support/composer-caret-check.tsx"],{cwd,stdout:"pipe",stderr:"pipe"});
 const [status,stdout,stderr]=await Promise.all([child.exited,new Response(child.stdout).text(),new Response(child.stderr).text()]);
 expect({status,stderr,stdout:status===0?undefined:stdout}).toEqual({status:0,stderr:"",stdout:undefined});
 expect(JSON.parse(stdout)).toMatchObject({nativeEventRegistered:true,sourceBlockWidth:9.6,sourceBlockHeight:20,burstFrames:1,visibleSpring:true,unicodeOffset:11,windowHandoff:true,selectionAndBlurHidden:true,cursorAlpha:255,idleBlinkRestored:true,nativeWindowsCreated:0});
},15_000);


test("compiled native input factories retain caretGeometry through the real custom-element event whitelist",async()=>{
 const script=`const native=require(process.argv[1]); if(!native.inputCaretGeometrySupported()||native.testSupportEnabled())process.exit(1);`;
 const child=Bun.spawn([process.execPath,"-e",script,verifiedDisplayBinding().binding],{stdout:"pipe",stderr:"pipe"});
 const [status,err]=await Promise.all([child.exited,new Response(child.stderr).text()]);
 expect({status,err}).toEqual({status:0,err:""});
});
