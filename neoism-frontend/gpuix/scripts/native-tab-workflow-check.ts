import {mkdtempSync,readFileSync,rmSync,writeFileSync} from "node:fs";
import {join} from "node:path";
import {tmpdir} from "node:os";
import type {TreeNode} from "@gpuix/solid/automation";
import {launchNative} from "../tests/support/native-launch";
import {createAgentDaemon} from "../tests/support/agent-daemon";
const directory=mkdtempSync(join(tmpdir(),"neoism-tab-workflow-")),config=join(directory,"config.json"),recovery=join(directory,"state.json");
const configuration={appearance:{fonts:{family:"Cascadia Code NF",size:14}},editor:{"vim-mode":true},terminal:{cursor:{blinking:false}}};
writeFileSync(config,JSON.stringify(configuration));
const server=createAgentDaemon({deferCreations:false,configuration,directoryEntries:{"":[{name:"README.md",is_dir:false},{name:"code.rs",is_dir:false}]}});
server.files.set("code.rs",Array.from({length:80},(_,i)=>`pub fn code_${i}() -> usize { ${i} }`).join("\n"));
writeFileSync(recovery,JSON.stringify({version:1,activeWorkspaceId:"a",activeSurfaceId:"terminal:restored",surfaces:[{id:"terminal:restored",kind:"terminal",title:"Restored terminal",workspaceId:"a",sessionId:"restored"}],ui:{openWorkspaceIds:["a","b"],mainTerminals:{a:"restored"}},editors:{},providerStates:{},drafts:{},composerDrafts:{}}));
const packaged=process.argv.includes("--packaged");
const app=await launchNative({openWorkspaceIds:["a","b"],command:packaged?join(process.cwd(),"dist/neoism-gpuix"):process.execPath,args:[...(packaged?[]:["src/main.tsx"]),"--daemon",server.endpoint,"--workspace","a","--config",config],cwd:process.cwd(),env:{...process.env,NEOISM_GPUIX_STATE_PATH:recovery}});
const until=async(predicate:()=>boolean|Promise<boolean>)=>{const end=Date.now()+15000;while(!await predicate()){if(Date.now()>end)throw new Error("Tab workflow did not converge");await Bun.sleep(10);}};
const flatten=(nodes:TreeNode[]):TreeNode[]=>nodes.flatMap(node=>[node,...flatten(node.children||[])]);
const nodes=async()=>{const {tree}=await app.call("getTree",{});return flatten(tree?[tree]:[]);};
const tabs=async(title:string)=>(await nodes()).filter(node=>/^pane-tab:\d+$/.test(node.testId||"")&&flatten([node]).some(child=>child.text===title));
const creates=()=>server.ptyRequests.filter(request=>request.tag==="CreatePty").length;
let lastCaretGeometry:unknown;
const press=(keys:string)=>app.call("keystrokes",{keys});
const plus=async()=>{const node=(await nodes()).find(node=>/^pane-new-welcome:/.test(node.testId||""));if(!node?.testId)throw new Error("Missing Welcome plus");return app.getByTestId(node.testId);};
const assertPlus=async()=>{
 const tree=await nodes(),scroll=tree.find(node=>/^pane-tab-scroll:/.test(node.testId||"")),last=flatten(scroll?.children||[]).filter(node=>/^pane-tab:\d+$/.test(node.testId||"")).at(-1)?.bounds;
 const box=await(await plus()).bounds(),strip=scroll?.bounds;
 if(!strip||!last||Math.abs(box.x-last.x-last.width)>1.3||Math.abs(box.y-last.y)>1.3||Math.abs(box.height-last.height)>1.3||box.width<10||box.x<strip.x-1.3||box.x+box.width>strip.x+strip.width+1.3)throw new Error(`Inline plus is not visible immediately after the last tab ${JSON.stringify({box,last,strip})}`);
};
const assertMarkdownTabCaret=async()=>{
 const tab=(await tabs("README.md"))[0]!;await press("alt-up");await app.getByTestId("chrome-focus-cursor:buffers").waitFor();
 const aligned=async()=>{const caret=await app.getByTestId("chrome-focus-cursor:buffers").bounds(),icon=await app.getByTestId(tab.testId!.replace("pane-tab:","pane-tab-icon:")).bounds();lastCaretGeometry={caret,icon};return Math.abs(caret.x+caret.width-icon.x)<1.3;};
 await until(aligned);const treeBefore=await app.getByTestId("file-tree").count();await press("alt-e");await until(async()=>await app.getByTestId("file-tree").count()!==treeBefore);await Bun.sleep(70);await until(aligned);await press("alt-e");await until(async()=>await app.getByTestId("file-tree").count()===treeBefore);await Bun.sleep(70);await until(aligned);
};
const openTerminal=async(label:string)=>{const before=creates();await press("alt-t");await until(()=>creates()===before+1);await app.getByTestId("terminal").waitFor();if(await app.getByTestId("splash-wordmark").count())throw new Error(`AltT from ${label} left Welcome inside the terminal`);};
let stage="Welcome selected before attaching a restored terminal";
try{
 await app.getByTestId("splash-wordmark").waitFor({timeoutMs:20000});if(creates()||server.ptyRequests.some(request=>request.tag==="AttachPty"))throw new Error("Restored terminal mounted before Welcome");
 const logo=await app.getByTestId("splash-wordmark").bounds(),logoCanvas=(await app.getByTestId("splash-wordmark").getByType("canvas").all())[0]?.bounds;
 if(!logoCanvas||logoCanvas.width<=logo.width||logoCanvas.height<=logo.height)throw new Error("Wordmark paint is still clipped to its logo hitbox");
 const welcome=(await tabs("Welcome"))[0]!;
 stage="main restored terminal is closable";const restored=(await tabs("Restored terminal"))[0]!;await app.getByTestId(restored.testId!.replace("pane-tab:","pane-close:")).click();await until(async()=>!(await tabs("Restored terminal")).length);
 stage="inline plus follows the last tab and remains usable under overflow";await assertPlus();for(let i=0;i<14;i++){await(await plus()).click();await until(async()=>(await tabs("Welcome")).length===i+2);await assertPlus();}
 if(creates())throw new Error("Welcome plus created a terminal");
 stage="AltT from Welcome and Terminal";await openTerminal("Welcome");const firstTerminal=(await tabs("Terminal"))[0]!;await openTerminal("Terminal");
 stage="all terminals close including first/main";await app.getByTestId(firstTerminal.testId!.replace("pane-tab:","pane-close:")).click();await until(async()=>(await tabs("Terminal")).length===1);
 stage="Code breadcrumbs reserve original header above the editor";await app.getByTestId("file:code.rs").click();await app.getByTestId("code-editor").waitFor();await app.getByTestId("code-breadcrumbs").waitFor();
 const breadcrumb=await app.getByTestId("code-breadcrumbs").bounds(),editor=await app.getByTestId("code-editor").bounds();if(Math.abs(breadcrumb.height-26)>1||Math.abs(editor.y-breadcrumb.y-breadcrumb.height)>1.3)throw new Error(`Breadcrumb geometry differs ${JSON.stringify({breadcrumb,editor})}`);
 stage="AltT from Code";await openTerminal("Code");
 stage="Markdown tab cursor aligns before icon after scrolling and sidebar moves";await app.getByTestId("file:README.md").click();await app.getByTestId("rich-markdown-editor").waitFor();await assertMarkdownTabCaret();
 stage="AltT from Markdown tab strip";await openTerminal("Markdown");
 stage="AltT from Agent draft";await(await plus()).click();await app.getByTestId("splash-wordmark").waitFor();await app.getByTestId("splash-action:open-agent").click();await app.getByTestId("composer").waitFor();await app.getByTestId("composer").fill("keep my Agent draft");await openTerminal("Agent");
 await until(()=>{try{return JSON.parse(readFileSync(recovery,"utf8")).composerDrafts["created-a"]?.text==="keep my Agent draft";}catch{return false;}});
 if(server.requests.some(request=>request.tag==="SubmitPrompt"))throw new Error("AltT submitted the Agent draft");
 stage="AltT from Settings overlay";await app.getByTestId("topbar-menu").click();await app.getByTestId("menu:OpenSettings").click();await app.getByTestId("chrome-action-overlay").waitFor();await openTerminal("Settings");await until(async()=>await app.getByTestId("chrome-action-overlay").count()===0);
 stage="global font zoom persists and retains the native root";const shell=(await app.getByTestId("app-shell").element()).id,height=(await app.getByTestId("original-topbar").bounds()).height,count=creates();await press("ctrl-=");await until(()=>server.configRequests.some(request=>request.tag==="SetSetting"&&request.body.key==="appearance.fonts.size"&&request.body.value===15));await until(async()=>(await app.getByTestId("original-topbar").bounds()).height>height+.5);
 if((await app.getByTestId("app-shell").element()).id!==shell)throw new Error("Zoom replaced the native root and can expose an empty frame");
 stage="font size retained across workspace change";await app.getByTestId("workspace:b").click();await app.getByTestId("splash-wordmark").waitFor();if((await app.getByTestId("original-topbar").bounds()).height<=height+.5)throw new Error("Font zoom applied only to the previous page");
 stage="fractional-scale Markdown tab caret after zoom";await app.getByTestId("file:README.md").click();await app.getByTestId("rich-markdown-editor").waitFor();await assertMarkdownTabCaret();
 stage="global zoom out from tab-strip focus";
 await press("ctrl--");await until(()=>server.configRequests.some(request=>request.tag==="SetSetting"&&request.body.key==="appearance.fonts.size"&&request.body.value===14));await until(async()=>Math.abs((await app.getByTestId("original-topbar").bounds()).height-height)<1);if(creates()!==count)throw new Error("Font zoom respawned terminals");
 if((await app.getByTestId("app-shell").element()).id!==shell)throw new Error("Zoom out replaced the native root");
 console.log(JSON.stringify({packaged,welcomeWinsRecovery:true,inlinePlusFollowsLastTabUnderOverflow:true,allTerminalsClosable:true,altTFromWelcomeTerminalCodeMarkdownAgentSettings:true,agentDraftPreserved:true,globalFontZoom:true,zoomDoesNotCreatePty:true,codeBreadcrumbs:true,markdownTabCaretTracksIcon:true,wordmarkPaintOverflowsLogo:true}));
}catch(error){console.error(`Native tab workflow failed at ${stage}`);console.error(JSON.stringify({pty:server.ptyRequests,config:server.configRequests,lastCaretGeometry}));throw error;}
finally{await app.close();server.stop();rmSync(directory,{recursive:true,force:true});}
