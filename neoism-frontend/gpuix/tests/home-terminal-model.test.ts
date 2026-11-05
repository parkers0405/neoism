import {afterEach,expect,test} from "bun:test";
import {Effect} from "effect";
import {loadConfig} from "../src/config";
import {DaemonConnection} from "../src/daemon/client";
import {WorkspaceModel} from "../src/model";

const cleanups:Array<()=>void>=[];
afterEach(()=>cleanups.splice(0).reverse().forEach(cleanup=>cleanup()));
function fixture(){
  const config={...loadConfig(["--config","/nonexistent/home-terminal-test"],{}),terminalBlockUi:true,shell:{program:"/bin/zsh",args:["-l"]}};
  const client=new DaemonConnection(config),model=new WorkspaceModel(client,config);
  const a={id:"a",host_id:"host",title:"A",root_dir:"/project/a"},b={...a,id:"b",root_dir:"/project/b"};
  const requests:Array<{family:string;message:any}>=[];
  let sequence=0,presentation="blocks",failBind=false;
  let holdCreate=false,resolveCreate:(()=>void)|undefined;
  client.request=((family:string,message:any)=>{
    requests.push({family,message});
    if(message.CreatePty){
      const result={PtyCreated:{session_id:`pty-${++sequence}`,presentation}};
      return holdCreate?Effect.promise(()=>new Promise(resolve=>{resolveCreate=()=>resolve(result);})):Effect.succeed(result);
    }
    if(message.CloseWindowWorkspace)return Effect.succeed({HostWorkspaceTree:{hosts:[{id:"host",label:"Host"}],workspaces:[a,b].filter(workspace=>workspace.id!==message.CloseWindowWorkspace.workspace_id),tabs:[]}});
    if(message.BindEditorSurface&&failBind)return Effect.fail(new Error("bind failed"));
    return Effect.succeed("Ack");
  }) as typeof client.request;
  model.update({workspaces:[a,b],activeWorkspace:a,connection:{phase:"connected",attempt:0,workspaceLifecycle:true}});
  const home=model.ensureWelcome()!;
  cleanups.push(()=>{model.dispose();client.stop();});
  return {model,client,a,b,home,requests,
    creates:()=>requests.filter(r=>r.message.CreatePty),closes:()=>requests.filter(r=>r.message.ClosePty),
    hold:()=>{holdCreate=true;},release:()=>resolveCreate!(),mismatch:()=>{presentation="grid";},fail:()=>{failBind=true;}};
}
async function tick(){await Bun.sleep(0);}

test("Home is single-flight, honors shell, and only the first shell gets the main role",async()=>{
  const f=fixture(),before=f.model.state.activeSurfaceId;
  const first=f.model.ensureHomeTerminal(f.home.id);
  expect(f.model.ensureHomeTerminal(f.home.id)).toBe(first);
  await first;
  await f.model.ensureHomeTerminal(f.home.id);
  expect(f.creates()).toHaveLength(1);
  expect(f.creates()[0].message.CreatePty).toMatchObject({cwd:f.a.root_dir,presentation:"blocks",shell:"/bin/zsh",shell_args:["-l"]});
  expect(f.model.state.surfaces.find(s=>s.id===f.home.id)).toMatchObject({kind:"welcome",sessionId:"pty-1",rootDir:f.a.root_dir});
  expect(f.model.state.activeSurfaceId).toBe(before);
  expect(f.model.mainTerminalSurface()?.id).toBe(f.home.id);
  expect(f.model.state.ui.mainTerminals).toEqual({a:"pty-1"});
  const second=f.model.newWelcome()!;await f.model.ensureHomeTerminal(second.id);
  expect(f.model.mainTerminalSurface()?.id).toBe(f.home.id);
  expect(f.model.state.surfaces.filter(s=>s.kind==="terminal")).toHaveLength(0);
  f.model.closeSurface(f.home.id);
  await tick();
  expect(f.closes().map(r=>r.message.ClosePty.session_id)).toEqual(["pty-1"]);
  expect(f.model.state.surfaces.find(s=>s.id===second.id)?.sessionId).toBe("pty-2");
  expect(f.model.state.ui.mainTerminals).toEqual({});
  expect(f.model.mainTerminalSurface()?.id).toBe(second.id);
});

test("workspace switch during create queues binding for the original Home and cwd",async()=>{
  const f=fixture();f.hold();const pending=f.model.ensureHomeTerminal(f.home.id);await tick();
  f.model.update({activeWorkspace:f.b});f.release();await pending;
  expect(f.creates()[0].message.CreatePty.cwd).toBe(f.a.root_dir);
  expect(f.requests.some(r=>r.message.BindEditorSurface)).toBe(false);
  expect(f.model.state.ui.pendingTerminalBindings).toEqual({[f.home.id]:{workspaceId:"a",sessionId:"pty-1"}});
  f.model.update({activeWorkspace:f.a});
  await (f.model as any).bindPendingTerminals("a");
  expect(f.requests.filter(r=>r.message.BindEditorSurface)).toHaveLength(1);
  expect(f.model.state.ui.pendingTerminalBindings).toEqual({});
});

for(const race of ["closed","disposed","root changed"]){
  test(`late Home create is closed when ${race}`,async()=>{
    const f=fixture();f.hold();const pending=f.model.ensureHomeTerminal(f.home.id);await tick();
    if(race==="closed")f.model.closeSurface(f.home.id);
    else if(race==="disposed")f.model.dispose();
    else f.model.update({workspaces:[{...f.a,root_dir:"/new"},f.b]});
    f.release();await pending;
    expect(f.closes().map(r=>r.message.ClosePty.session_id)).toEqual(["pty-1"]);
    expect(f.requests.some(r=>r.message.BindEditorSurface)).toBe(false);
  });
}

for(const failure of ["presentation","binding"]){
  test(`Home ${failure} failure closes only the fresh PTY`,async()=>{
    const f=fixture();if(failure==="presentation")f.mismatch();else f.fail();
    await f.model.ensureHomeTerminal(f.home.id);
    expect(f.closes().map(r=>r.message.ClosePty.session_id)).toEqual(["pty-1"]);
    expect(f.model.state.surfaces.find(s=>s.id===f.home.id)?.sessionId).toBeUndefined();
    expect(f.model.state.error).toBeDefined();
  });
}

test("recovery retains ownership; authoritative retirement keeps Home and permits a new shell",async()=>{
  const f=fixture();await f.model.ensureHomeTerminal(f.home.id);
  const saved=f.model.captureSession();
  const recovered=new WorkspaceModel(f.client,f.model.config);cleanups.push(()=>recovered.dispose());
  recovered.restoreSession(saved);recovered.update({workspaces:[f.a,f.b],activeWorkspace:f.a});
  await recovered.ensureHomeTerminal(f.home.id);
  expect(f.creates()).toHaveLength(1);
  recovered.retireTerminal("pty-1");
  expect(recovered.state.surfaces.find(s=>s.id===f.home.id)?.kind).toBe("welcome");
  await recovered.ensureHomeTerminal(f.home.id);
  expect(recovered.state.surfaces.find(s=>s.id===f.home.id)?.sessionId).toBe("pty-2");
  expect(f.closes()).toHaveLength(0);
});

test("an attached foreign shell is never adopted, replaced, or closed",async()=>{
  const f=fixture();f.model.update({surfaces:[{...f.home,sessionId:"foreign"}]});
  await f.model.ensureHomeTerminal(f.home.id);
  expect(f.model.state.error).toContain("ownership");
  expect(f.model.mainTerminalSurface()).toBeUndefined();
  f.model.closeSurface(f.home.id);
  expect(f.creates()).toHaveLength(0);expect(f.closes()).toHaveLength(0);
});

test("a queued Home binding error releases its owned shell",async()=>{
  const f=fixture();f.model.update({activeWorkspace:f.b});
  await f.model.ensureHomeTerminal(f.home.id);
  f.model.update({activeWorkspace:f.a});f.fail();
  await expect((f.model as any).bindPendingTerminals("a")).rejects.toThrow("bind failed");
  await tick();
  expect(f.closes().map(r=>r.message.ClosePty.session_id)).toEqual(["pty-1"]);
  expect(f.model.state.surfaces.find(s=>s.id===f.home.id)?.sessionId).toBeUndefined();
});

for(const inflight of [false,true]){
  test(`closing a retained workspace cleans Home ${inflight?"in-flight":"established"} shells`,async()=>{
    const f=fixture();f.model.setUI({openWorkspaceIds:["a","b"]});
    if(inflight)f.hold();
    const pending=f.model.ensureHomeTerminal(f.home.id);await tick();
    if(!inflight)await pending;
    f.model.update({activeWorkspace:f.b});
    expect(await f.model.closeWorkspace(false,"a")).toBe(true);
    if(inflight)f.release();await pending;await tick();
    expect(f.requests.filter(r=>r.message.CloseWindowWorkspace).map(r=>r.message)).toEqual([{CloseWindowWorkspace:{workspace_id:"a"}}]);
    // Established shells belong to host teardown (and may still be shared by
    // another window). Only an unbound late create is closed by this client.
    expect(f.closes().map(r=>r.message.ClosePty.session_id)).toEqual(inflight?["pty-1"]:[]);
    expect(f.model.state.workspaces.map(workspace=>workspace.id)).toEqual(["b"]);
    expect(f.model.state.ui.homeTerminalSessions).toEqual({});
    expect(f.model.state.surfaces.find(s=>s.id===f.home.id)?.sessionId).toBeUndefined();
    expect(f.model.state.ui.pendingTerminalBindings).toEqual({});
  });
}