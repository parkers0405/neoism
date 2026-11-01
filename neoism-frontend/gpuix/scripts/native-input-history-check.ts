import assert from "node:assert/strict";
import {mkdtempSync,writeFileSync,rmSync} from "node:fs";
import {tmpdir} from "node:os";
import {join} from "node:path";
import type {TreeNode} from "@gpuix/solid/automation";
import {launchNative} from "../tests/support/native-launch";
import {createAgentDaemon} from "../tests/support/agent-daemon";
const directory=mkdtempSync(join(tmpdir(),"neoism-input-history-")),state=join(directory,"session.json"),config=join(directory,"config.json");
writeFileSync(config,JSON.stringify({appearance:{fonts:{family:"Cascadia Code NF",size:14}},terminal:{cursor:{blinking:false}}}));
writeFileSync(state,JSON.stringify({version:1,activeWorkspaceId:"a",activeSurfaceId:"agent:one",surfaces:[{id:"agent:one",kind:"agent",title:"One",workspaceId:"a",sessionId:"one"}],editors:{},providerStates:{},drafts:{},composerDrafts:{},ui:{openWorkspaceIds:["a"]}}));
const daemon=createAgentDaemon();daemon.history.set("one",[{id:"welcome",role:"Assistant",kind:"Assistant",text:"Input history test"}]);
const app=await launchNative({command:process.execPath,args:["src/main.tsx","--daemon",daemon.endpoint,"--workspace","a","--config",config],cwd:process.cwd(),env:{...process.env,NEOISM_GPUIX_STATE_PATH:state}});
const until=async(check:()=>boolean)=>{const end=Date.now()+8000;while(!check()){if(Date.now()>end)throw Error("Native input history timed out");await Bun.sleep(20);}};
try{
 await app.getByTestId("workspace-tabs").waitFor({timeoutMs:20000});
 const flatten=(nodes:TreeNode[]):TreeNode[]=>nodes.flatMap(node=>[node,...flatten(node.children||[])]);
 const {tree}=await app.call("getTree",{});const tab=flatten(tree?[tree]:[]).find(node=>/^pane-tab:\d+$/.test(node.testId||"")&&flatten([node]).some(child=>child.text==="One"));
 assert.ok(tab?.testId);await app.getByTestId(tab.testId).click();
 const composer=app.getByTestId("composer");await composer.waitFor({timeoutMs:20000});
 const sent=()=>daemon.requests.filter(request=>request.tag==="SubmitPrompt");
 const submit=async(count:number,text:string)=>{await composer.press("enter");await until(()=>sent().length===count);assert.equal(sent()[count-1].body.text,text);await Bun.sleep(80);};
 await composer.fill("first prompt");await submit(1,"first prompt");
 await composer.fill("second prompt");await submit(2,"second prompt");
 await composer.fill("unfinished draft");await composer.press("up");await composer.press("up");await submit(3,"first prompt");
 await composer.fill("restore my draft");await composer.press("up");await composer.press("down");await submit(4,"restore my draft");
 await composer.fill("line one");await composer.press("shift-enter l i n e space t w o");await composer.press("up");await submit(5,"line one\nline two");
 // Use real native key dispatch, with and without a native text selection.
 await composer.fill("must be cleared");await composer.press("ctrl-c");await composer.press("c l e a r e d");await submit(6,"cleared");
 await composer.fill("selected text must also clear");await composer.press("ctrl-a ctrl-c");await composer.press("s e l e c t e d");await submit(7,"selected");
 daemon.push({StreamingState:{session_id:"one",state:"Generating",label:null}});await Bun.sleep(150);
 await composer.fill("clear before cancelling");await composer.press("ctrl-c");await Bun.sleep(100);
 assert.equal(daemon.requests.filter(request=>request.tag==="CancelInflight").length,0);
 await composer.press("ctrl-c");await until(()=>daemon.requests.some(request=>request.tag==="CancelInflight"));
 assert.equal(daemon.requests.filter(request=>request.tag==="CancelInflight").length,1);
 const cancelled=()=>daemon.requests.filter(request=>request.tag==="CancelInflight").length;
 await composer.fill("Escape clears this draft first");await composer.press("escape");await Bun.sleep(100);assert.equal(cancelled(),1);
 await composer.press("escape");await until(()=>cancelled()===2);
 await composer.press("escape");await until(()=>cancelled()===3);
 daemon.push({SessionIdle:{session_id:"one"}});await Bun.sleep(100);
 await composer.fill("reset escape guard");await composer.fill("");await composer.press("escape");await Bun.sleep(100);assert.equal(cancelled(),3);
 await composer.press("escape");await until(()=>cancelled()===4);
 // A slash menu consumes Escape without interrupting the run behind it.
 daemon.push({StreamingState:{session_id:"one",state:"Generating",label:null}});await composer.fill("/mod");await app.getByTestId("agent-picker:slash").waitFor();
 await composer.press("escape");await Bun.sleep(100);assert.equal(cancelled(),4);assert.equal(await app.getByTestId("agent-picker:slash").count(),0);
 // Exercise actual shaping on both sides of a soft wrap, not a synthetic
 // caret event. Its width must remain the same full block at the right edge.
 const caret=app.getByTestId("agent-composer-cursor");
 await composer.fill("");await Bun.sleep(350);
 if(await caret.count()){
 const start=await caret.bounds();
 await composer.fill("m");await Bun.sleep(350);const one=await caret.bounds(),inputBox=await composer.bounds(),advance=one.x-start.x;
 assert.ok(advance>2&&start.width>4);
 const near=Math.floor((inputBox.x+inputBox.width-start.x)/advance);let wrapped=false;
 for(let count=Math.max(1,near-3);count<=near+3;count++){
  await composer.fill("m".repeat(count));await Bun.sleep(350);const block=await caret.bounds();
  assert.ok(Math.abs(block.width-start.width)<.15,`Full block at wrap (${count} chars): ${block.width} versus ${start.width}`);
  if(block.x<start.x+advance*5)wrapped=true;
 }
 assert.ok(wrapped,"Native fixture must cross a shaped soft-wrap boundary");
 console.log("Native soft-wrap caret passed: full block width before and after wrapping.");
 }else console.log("Native soft-wrap visual check skipped: fixture window has no visible caret; geometry unit tests cover the wrap gutter.");
 console.log("Native Escape passed: clear first, stop active runs, idle double-Escape, and picker dismissal.");
 console.log("Native Ctrl+C passed: clear draft, clear selected text, then cancel only when empty.");
 console.log("Native input history passed: Up/Down recall, unfinished draft restoration, and multiline caret navigation.");
}finally{await app.close();daemon.stop();rmSync(directory,{recursive:true,force:true});}
