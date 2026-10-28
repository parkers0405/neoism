import {mkdtempSync,rmSync,writeFileSync} from "node:fs";
import {join} from "node:path";
import {tmpdir} from "node:os";
import type {TreeNode} from "@gpuix/solid/automation";
import {launchNative} from "../tests/support/native-launch";
import {createAgentDaemon} from "../tests/support/agent-daemon";
import {coreCall} from "../src/native-core";
const directory=mkdtempSync(join(tmpdir(),"neoism-native-composer-")),config=join(directory,"config.json");
writeFileSync(config,JSON.stringify({appearance:{fonts:{family:"Cascadia Code NF",size:14}},editor:{"vim-mode":true}}));
const server=createAgentDaemon({deferCreations:false,slashCommands:[{name:"workspace-review",description:"A real host command"}]});
const app=await launchNative({command:process.execPath,args:["src/main.tsx","--daemon",server.endpoint,"--workspace","a","--config",config,"--reduced-motion"],cwd:process.cwd(),env:{...process.env,NEOISM_GPUIX_STATE_PATH:join(directory,"state.json")}});
const until=async(predicate:()=>Promise<boolean>|boolean)=>{const end=Date.now()+12000;while(!await predicate()){if(Date.now()>end)throw new Error("Agent Composer did not converge");await Bun.sleep(10);}};
const flatten=(nodes:TreeNode[]):TreeNode[]=>nodes.flatMap(node=>[node,...flatten(node.children||[])]);
const nodes=async()=>{const {tree}=await app.call("getTree",{});return flatten(tree?[tree]:[]);};
let stage="load workspace";
try{
 await app.getByTestId("workspace:a").waitFor({timeoutMs:20000});await until(async()=>await app.getByTestId("loading-skeleton:tree").count()===0);
 stage="Alt+A opens centered empty Agent session";await app.call("keystrokes",{keys:"alt-a"});
 await until(()=>server.requests.some(request=>request.tag==="CreateThread"));await app.getByTestId("composer").waitFor();
 await until(async()=>{const pane=await app.getByTestId("agent-pane").bounds(),card=await app.getByTestId("agent-input-card").bounds();return pane.width>200&&pane.height>200&&Math.abs(card.y-pane.y-pane.height*.46)<1;});
 if(await app.getByTestId("agent-details").count())throw new Error("Empty home incorrectly reserved a conversation details rail");
 const pane=await app.getByTestId("agent-pane").bounds(),card=await app.getByTestId("agent-input-card").bounds(),footer=await app.getByTestId("agent-composer-footer").bounds();
 if(Math.abs(card.height-102)>.7||Math.abs(footer.height-32)>.7||Math.abs(footer.y-card.y-card.height)>1||Math.abs(footer.x-card.x-24)>1||Math.abs(footer.width-card.width+48)>1)throw new Error(`Original card/tray geometry differs: ${JSON.stringify({pane,card,footer})}`);
 if(!(await app.getByTestId("agent-composer-shell").getByType("canvas").all()).some(node=>(node.bounds?.width||0)>1))throw new Error("Original footer glyph rasters did not mount");
 const composer=app.getByTestId("composer");stage="slash keeps composer caret and exposes full original plus actual SDK catalog";await composer.fill("/");await app.getByTestId("agent-picker:slash").waitFor();
 // Only the visible window of rows is mounted (desktop parity), so each
 // command is proven reachable through the picker's own source filter.
 const expected=coreCall<Array<{value:string}>>("agentcomposer.slash-options");
 for(const value of [...expected.map(option=>option.value),"/workspace-review"]){await composer.fill(value);try{await app.getByTestId(`picker-option:${value}`).waitFor({timeoutMs:2000});}catch{throw new Error(`Slash command omitted: ${value}`);}}
 await composer.fill("/");await app.getByTestId("agent-picker:slash").waitFor();
 if(await app.getByTestId("inline-picker-input").count())throw new Error("Slash popup stole typing into a separate search input");
 stage="slash query routes through source filter and executes actual thinking update";await composer.fill("/think high");await composer.press("enter");await until(()=>server.requests.some(request=>request.tag==="UpdateSession"&&(request.body.model as any)?.variant==="high"));
 if(server.requests.some(request=>request.tag==="SubmitPrompt"))throw new Error("A built-in slash command was submitted as ordinary prompt text");
 stage="slash model query opens";await composer.fill("/model");await app.getByTestId("agent-picker:slash").waitFor();stage="slash model Enter opens model chooser";await composer.press("enter");await app.getByTestId("agent-picker:model").waitFor();await app.getByTestId("inline-picker-input").press("escape");
 stage="successful text submission changes original home layout to chat bottom anchoring";await composer.fill("real chat body");await composer.press("enter");await until(()=>server.requests.some(request=>request.tag==="SubmitPrompt"&&request.body.text==="real chat body"));
 server.push({MessageUpdated:{session_id:"created-a",message:{id:"user-native-composer",kind:"User",role:"User",text:"real chat body"}}});
 await until(async()=>{const bounds=await app.getByTestId("agent-input-card").bounds(),tray=await app.getByTestId("agent-composer-footer").bounds(),viewport=await app.getByTestId("agent-pane").bounds();return Math.abs(bounds.height-94)<.7&&Math.abs(tray.y+tray.height-viewport.y-viewport.height)<1;});
 console.log(JSON.stringify({scope:"production Main/App with isolated daemon; semantic native bounds/input and source painter, not compositor pixel comparison",originalSlashCommands:expected.length,customSdkCommand:true,altACentered:true,originalFooterGeometry:true,realThinkingUpdate:true,chatBottomAnchor:true}));
}catch(error){console.error(`Native Composer check failed at ${stage}; requests ${JSON.stringify(server.requests)}`);console.error(JSON.stringify((await nodes()).filter(node=>node.testId?.includes("picker")).map(({children,...node})=>node)));console.error(JSON.stringify((await nodes()).filter(node=>["agent-pane","agent-input-card","agent-composer-footer","composer"].includes(node.testId||"")).map(({children,...node})=>node)));throw error;}
finally{await app.close();server.stop();rmSync(directory,{recursive:true,force:true});}
