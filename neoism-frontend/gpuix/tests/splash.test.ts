import {Effect} from "effect";
import {afterEach,expect,test} from "bun:test";
import {coreCall} from "../src/native-core";
import {loadConfig} from "../src/config";
import {DaemonConnection} from "../src/daemon/client";
import {WorkspaceModel} from "../src/model";
import {NativeEditor} from "../src/native-core";
import {createAgentDaemon} from "./support/agent-daemon";
const cleanups:Array<()=>void>=[];afterEach(()=>cleanups.splice(0).reverse().forEach(close=>close()));
const until=async(predicate:()=>boolean)=>{const deadline=Date.now()+5000;while(!predicate()){if(Date.now()>deadline)throw new Error("Splash state did not converge");await Bun.sleep(5);}};
function splash(){const {id}=coreCall<{id:number}>("splash.new",{fontFamily:"Cascadia Code NF",fontWeight:400});cleanups.push(()=>coreCall("splash.dispose",{id}));return {id,frame:(values:Record<string,unknown>={})=>coreCall<any>("splash.frame",{id,width:1200,height:800,cols:120,rows:43,cellHeight:18.2,scale:1,theme:"pastel_dark",...values})};}
test("original splash paints all eight source menu actions and exact hit geometry",()=>{
 const host=splash(),frame=host.frame();
 expect(frame.rows.map((row:any)=>row.label)).toEqual(["Change Directory","Notes","Open file tree","Neoism Agent","Search","Agent panel","Command palette","New Terminal"]);
 expect(frame.wordmark[2]).toBeLessThanOrEqual(720);expect(frame.wordmark[0]+frame.wordmark[2]/2).toBeCloseTo(600);
 expect(frame.rows[0].rect[1]).toBe(frame.rows[1].rect[1]);expect(frame.rows[0].rect[0]).toBeLessThan(frame.rows[1].rect[0]);
 for(const row of frame.rows){const [x,y,w,h]=row.rect;expect(coreCall<any>("splash.hit",{id:host.id,x:x+w/2,y:y+h/2}).action).toBe(row.action);expect(x).toBeGreaterThanOrEqual(0);expect(x+w).toBeLessThanOrEqual(1200);}
 const row=frame.rows[0];expect(host.frame({pointer:true,x:row.rect[0]+1,y:row.rect[1]+1}).commands.some((command:any)=>command.kind==="rect")).toBe(true);
 const narrow=host.frame({width:300,height:500,cols:30,rows:27});expect(narrow.rows.every((row:any)=>row.rect[0]===narrow.rows[0].rect[0])).toBe(true);
 expect(host.frame({dismissedMs:420}).rows).toEqual([]);
});
async function fixture(restore?:boolean){
 const daemon=createAgentDaemon({deferCreations:false});const config={...loadConfig(["--config","/nonexistent/splash"],{}),endpoint:daemon.endpoint,authToken:undefined};const client=new DaemonConnection(config),model=new WorkspaceModel(client,config);
 cleanups.push(()=>{model.dispose();client.stop();daemon.stop();});
 if(restore){const editor=new NativeEditor("README.md","saved",true);editor.call("paste",{text:"dirty "});const snapshot=editor.snapshot();editor.dispose();model.restoreSession({version:1,activeWorkspaceId:"a",activeSurfaceId:"file:a:README.md",surfaces:[{id:"file:a:README.md",workspaceId:"a",kind:"file",title:"README.md",path:"README.md",rootDir:"/project/a"}],editors:{"file:a:README.md":snapshot},providerStates:{},drafts:{},composerDrafts:{},ui:{terminalSplash:{legacy:true}}});}
 model.start();client.start();await until(()=>model.state.activeWorkspace?.id==="a"&&!model.state.loading);return{daemon,model,client};
}
test("Welcome is an independent first tab and repeated '+' tabs never create PTYs",async()=>{
 const {daemon,model}=await fixture();expect(model.state.activeSurfaceId).toBe("welcome:a");expect(model.state.surfaces[0].kind).toBe("welcome");
 expect(model.getLayout().frame.panes[0].tabs[0].surfaceId).toBe("welcome:a");expect(model.mainTerminalSurface()).toBeUndefined();
 model.ensureWelcome();model.ensureWelcome();expect(model.state.surfaces).toHaveLength(1);
 const first=model.newWelcome()!,second=model.newWelcome()!;expect(first.id).not.toBe(second.id);expect(first.id).not.toBe("welcome:a");expect(model.state.activeSurfaceId).toBe(second.id);
 expect(model.getLayout().frame.panes[0].tabs.map(tab=>tab.kind)).toEqual(["welcome","welcome","welcome"]);
 await model.activateWorkspace(model.state.workspaces[1]);await model.activateWorkspace(model.state.workspaces[0]);expect(model.state.activeSurfaceId).toBe(second.id);
 expect(model.state.surfaces.filter(surface=>surface.id==="welcome:a")).toHaveLength(1);expect(model.state.surfaces.filter(surface=>surface.id==="welcome:b")).toHaveLength(1);
 expect(daemon.ptyRequests).toHaveLength(0);
});
test("cold recovery selects Welcome while preserving the recovered dirty document and stable leaf",async()=>{
 const {daemon,model}=await fixture(true);expect(model.state.activeSurfaceId).toBe("welcome:a");
 const file=model.state.surfaces.find(surface=>surface.kind==="file")!,editor=model.getEditor(file);expect(editor.text()).toBe("dirty saved");expect(editor.frame.dirty).toBe(true);
 const pane=model.getLayout().frame.panes[0];expect(pane.tabs[0].kind).toBe("welcome");expect(pane.tabs.at(-1)?.surfaceId).toBe(file.id);
 const leaf=pane.tabs.find(tab=>tab.surfaceId===file.id)!.leaf;model.selectSurface(file.id);model.ensureWelcome({select:false});expect(model.getLayout().frame.focusedLeaf).toBe(leaf);expect(model.getEditor(file)).toBe(editor);
 expect(model.state.ui.terminalSplash).toBeUndefined();expect(model.captureSession().ui.terminalSplash).toBeUndefined();expect(daemon.ptyRequests).toHaveLength(0);
});
test("a recovered active terminal never wins startup selection or recreates a shell",async()=>{
 const daemon=createAgentDaemon({deferCreations:false}),config={...loadConfig(["--config","/nonexistent/splash"],{}),endpoint:daemon.endpoint,authToken:undefined};
 const client=new DaemonConnection(config),model=new WorkspaceModel(client,config);cleanups.push(()=>{model.dispose();client.stop();daemon.stop();});
 const terminal={id:"terminal:existing",kind:"terminal" as const,title:"Terminal",workspaceId:"a",sessionId:"existing"};
 model.restoreSession({version:1,activeWorkspaceId:"a",activeSurfaceId:terminal.id,surfaces:[terminal],editors:{},providerStates:{},drafts:{},composerDrafts:{},ui:{mainTerminals:{a:"existing"}}});
 const visible:string[]=[];cleanups.push(model.subscribe(()=>{if(model.state.activeWorkspace)visible.push(model.state.activeSurfaceId||"");}));
 model.start();client.start();await until(()=>model.state.activeWorkspace?.id==="a"&&!model.state.loading);
 expect(model.state.activeSurfaceId).toBe("welcome:a");expect(visible).not.toContain(terminal.id);
 expect(model.state.surfaces.find(surface=>surface.id===terminal.id)).toEqual(terminal);
 expect(daemon.ptyRequests).toHaveLength(0);
});
test("all terminal tabs can close, including the main and final terminal, with Welcome remaining",async()=>{
 const {daemon,model}=await fixture();await model.newTerminal();const first=model.mainTerminalSurface()!;
 await model.newTerminal();const second=model.state.surfaces.find(surface=>surface.id===model.state.activeSurfaceId)!;
 model.setUI({terminalCwds:{[first.sessionId!]:"/project/a/sub"}});
 model.closePane(first.id,model.getLayout().frame.leaves.find(leaf=>leaf.surfaceId===first.id)!.leaf);
 expect(model.state.surfaces.some(surface=>surface.id===first.id)).toBe(false);
 expect((model.state.ui.mainTerminals as any).a).toBeUndefined();expect((model.state.ui.terminalCwds as any)[first.sessionId!]).toBeUndefined();
 expect(model.mainTerminalSurface()?.id).toBe(second.id);
 model.closePane(second.id,model.getLayout().frame.leaves.find(leaf=>leaf.surfaceId===second.id)!.leaf);
 expect(model.mainTerminalSurface()).toBeUndefined();expect(model.state.activeSurfaceId).toBe("welcome:a");
 expect(daemon.ptyRequests.filter(request=>request.tag==="CreatePty")).toHaveLength(2);
 await model.newTerminal();expect(model.mainTerminalSurface()?.sessionId).toBe("fixture-pty-3");
});
test("last Welcome closes normally and replaces the empty route without launching a shell",async()=>{
 const {daemon,model}=await fixture();const prior=model.getLayout().frame.focusedLeaf!,old=model.state.surfaces[0];model.closePane("welcome:a",prior);
 expect(model.state.surfaces).toHaveLength(1);expect(model.state.activeSurfaceId).toBe("welcome:a");expect(model.state.surfaces[0]).not.toBe(old);
 expect(model.getLayout().frame.panes[0].tabs[0].kind).toBe("welcome");expect(daemon.ptyRequests).toHaveLength(0);
});
test("explicit New Terminal creates a plain grid shell alongside Welcome and retains main cwd role",async()=>{
 const {daemon,model}=await fixture();await model.newTerminal();const main=model.mainTerminalSurface()!;
 expect(main.kind).toBe("terminal");expect(model.state.activeSurfaceId).toBe(main.id);expect(model.getLayout().frame.panes[0].tabs[0].kind).toBe("welcome");
 expect(daemon.ptyRequests.filter(request=>request.tag==="CreatePty")).toHaveLength(1);expect(model.state.ui.terminalSplash).toBeUndefined();
 model.selectSurface("welcome:a");model.newWelcome();expect(daemon.ptyRequests.filter(request=>request.tag==="CreatePty")).toHaveLength(1);expect(model.mainTerminalSurface()?.id).toBe(main.id);
});
test("late explicit background terminal creation binds after its workspace returns, without startup shells",async()=>{
 const {daemon,model,client}=await fixture();const original=client.request.bind(client);let release!:()=>void,held=false;const gate=new Promise<void>(resolve=>release=resolve),bindings:Array<{workspace:string|undefined;session:string}>=[];
 client.request=((family:any,message:any,root?:string)=>{
  const request=original(family,message,root);
  if(family==="Pty"&&message.CreatePty?.cwd==="/project/a"&&!held){held=true;return Effect.andThen(Effect.promise(()=>gate),request);}
  if(family==="Workspace"&&message.BindEditorSurface)bindings.push({workspace:model.state.activeWorkspace?.id,session:message.BindEditorSurface.session_id});return request;
 }) as typeof client.request;
 const creating=model.newTerminal();await until(()=>held);await model.activateWorkspace(model.state.workspaces.find(workspace=>workspace.id==="b")!);release();await creating;
 const mainA=model.mainTerminalSurface("a")!;expect(bindings.some(bind=>bind.session===mainA.sessionId)).toBe(false);
 expect((model.captureSession().ui.pendingTerminalBindings as any)[mainA.id]).toEqual({workspaceId:"a",sessionId:mainA.sessionId});
 await model.activateWorkspace(model.state.workspaces.find(workspace=>workspace.id==="a")!);expect(bindings.filter(bind=>bind.session===mainA.sessionId)).toEqual([{workspace:"a",session:mainA.sessionId!}]);
 expect(model.state.ui.pendingTerminalBindings).toEqual({});await model.activateWorkspace(model.state.workspaces[1]);await model.activateWorkspace(model.state.workspaces[0]);expect(bindings.filter(bind=>bind.session===mainA.sessionId)).toHaveLength(1);
 expect(daemon.ptyRequests.filter(request=>request.tag==="CreatePty")).toHaveLength(1);expect(model.getLayout().frame.panes[0].tabs[0].kind).toBe("welcome");
});
