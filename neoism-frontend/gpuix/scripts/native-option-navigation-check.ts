import {mkdtempSync,rmSync,writeFileSync} from "node:fs";
import {join} from "node:path";
import {tmpdir} from "node:os";
import {launchNative} from "../tests/support/native-launch";
import {createAgentDaemon} from "../tests/support/agent-daemon";
import assert from "node:assert/strict";
const directory=mkdtempSync(join(tmpdir(),"neoism-option-nav-")),config=join(directory,"config.json");
writeFileSync(config,JSON.stringify({appearance:{fonts:{family:"Cascadia Code NF",size:14}},terminal:{cursor:{blinking:false}}}));
const server=createAgentDaemon({deferCreations:false});
for(let i=0;i<60;i++)server.files.set(`cursor-${String(i).padStart(2,"0")}.rs`,"");
const app=await launchNative({openWorkspaceIds:["a"],command:process.execPath,args:["src/main.tsx","--daemon",server.endpoint,"--workspace","a","--config",config],cwd:process.cwd(),env:{...process.env,NEOISM_GPUIX_STATE_PATH:join(directory,"state.json")}});
const press=(keys:string)=>app.call("keystrokes",{keys});
try{
 await app.getByTestId("activity-rail").waitFor({timeoutMs:15000});
  await press("alt-a");const composer=app.getByTestId("composer");await composer.waitFor();await composer.fill("preserve this draft");
  // Use real native hit testing, not JS event dispatch: focus prevention is
  // evaluated by GPUI on each icon's hit region before JS receives the press.
  for(const icon of ["toggle-files","toggle-notes","toggle-conversations"]){
    await app.getByTestId(icon).click();await Bun.sleep(100);
    assert.equal(await app.getByTestId("chrome-focus-cursor:sidebar").count(),0,`${icon} must not park the cursor in the sidebar`);
  }
  await press("x");
  await press("alt-left");await app.getByTestId("chrome-focus-cursor:sidebar").waitFor();
  await press("alt-e");await app.getByTestId("file-tree").waitFor();
  await press("down");
  await Bun.sleep(250);
  const treeCursor=app.getByTestId("chrome-focus-cursor:sidebar");await treeCursor.waitFor();const before=(await treeCursor.bounds()).y;
  for(let i=0;i<6;i++)await press("down");
  await Bun.sleep(300);
  assert.ok((await treeCursor.bounds()).y>before+26,"Agent to Files cursor must follow repeated Down, not remain at the handoff position");
  await press("alt-right");
 // GPUI uses alt for the macOS Option modifier. These must escape native
 // textarea word/paragraph movement and reach the app's focus ladder.
 await press("alt-up");await app.getByTestId("chrome-focus-cursor:buffers").waitFor();
 await press("alt-down");await composer.click();
 await press("alt-left");await app.getByTestId("chrome-focus-cursor:sidebar").waitFor();
 await press("alt-right");await composer.click();
 await press("alt-up");await app.getByTestId("chrome-focus-cursor:buffers").waitFor();
 await press("alt-down");await composer.press("enter");
 const end=Date.now()+10000;
 while(!server.requests.some(row=>row.tag==="SubmitPrompt")){if(Date.now()>end)throw new Error("Draft did not submit after Option navigation");await Bun.sleep(20);}
 const prompt=server.requests.find(row=>row.tag==="SubmitPrompt");
  if(prompt?.body.text!=="preserve this draftx")throw new Error(`Navigation modified the draft or stole composer focus: ${JSON.stringify(prompt)}`);
 console.log(JSON.stringify({ok:true,checks:["Option/Alt arrows escape native textarea navigation","focus moves to buffers/sidebar and returns","draft preserved"]}));
}finally{await app.close();server.stop();rmSync(directory,{recursive:true,force:true});}
