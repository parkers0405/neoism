import {mkdtempSync,rmSync,writeFileSync} from "node:fs";
import {join} from "node:path";
import {tmpdir} from "node:os";
import {launchNative} from "../tests/support/native-launch";
import {createAgentDaemon} from "../tests/support/agent-daemon";
const directory=mkdtempSync(join(tmpdir(),"neoism-option-nav-")),config=join(directory,"config.json");
writeFileSync(config,JSON.stringify({appearance:{fonts:{family:"Cascadia Code NF",size:14}}}));
const server=createAgentDaemon({deferCreations:false});
const app=await launchNative({openWorkspaceIds:["a"],command:process.execPath,args:["src/main.tsx","--daemon",server.endpoint,"--workspace","a","--config",config],cwd:process.cwd(),env:{...process.env,NEOISM_GPUIX_STATE_PATH:join(directory,"state.json")}});
const press=(keys:string)=>app.call("keystrokes",{keys});
try{
 await app.getByTestId("original-topbar").waitFor({timeoutMs:15000});
 await press("alt-a");const composer=app.getByTestId("composer");await composer.waitFor();await composer.fill("preserve this draft");
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
 if(prompt?.body.text!=="preserve this draft")throw new Error(`Navigation modified the draft: ${JSON.stringify(prompt)}`);
 console.log(JSON.stringify({ok:true,checks:["Option/Alt arrows escape native textarea navigation","focus moves to buffers/sidebar and returns","draft preserved"]}));
}finally{await app.close();server.stop();rmSync(directory,{recursive:true,force:true});}
