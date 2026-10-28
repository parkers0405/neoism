import {Effect} from "effect";
import {readFileSync,writeFileSync,chmodSync} from "node:fs";
import {join} from "node:path";
import {DaemonConnection} from "../src/daemon/client";
import {expectVariant,isRecord} from "../src/daemon/protocol";
const directory=process.argv[2];if(!directory)throw new Error("Protected backup directory is required");
const env=JSON.parse(readFileSync(join(directory,"daemon-env.json"),"utf8"));
const client=new DaemonConnection({endpoint:"ws://127.0.0.1:7878/session",authToken:env.NEOISM_DAEMON_TOKEN});
const until=async(test:()=>boolean)=>{const end=Date.now()+10000;while(!test()){if(Date.now()>end)throw new Error(`Snapshot connection timeout: ${client.state.phase} ${client.state.reason||""}`);await Bun.sleep(10);}};
const req=(message:unknown)=>Effect.runPromise(client.request("Workspace",message as never));
const body=async(message:unknown,tag:string)=>{const value=expectVariant(await req(message),tag);if(!isRecord(value))throw new Error(`Invalid ${tag}`);return value as any;};
const save=(name:string,data:unknown)=>{const path=join(directory,name);writeFileSync(path,JSON.stringify(data,null,2));chmodSync(path,0o600);};
client.start();
try{
 await until(()=>client.state.phase==="connected");
 const tree=await body("RequestHostWorkspaceTree","HostWorkspaceTree"),roots=await body("ListProjectRoots","ProjectRootList"),windows=await body("ListWindows","WindowList");
 if(tree.workspaces.length<4)throw new Error(`Expected at least four live workspaces, got ${tree.workspaces.length}`);
 const snapshot:any={version:1,saved_at_unix_secs:Math.floor(Date.now()/1000),hosts:tree.hosts,host_workspaces:tree.workspaces,workspace_tabs:tree.tabs,active_workspace_by_host:Object.fromEntries(tree.hosts.filter((host:any)=>host.active_workspace_id).map((host:any)=>[host.id,host.active_workspace_id])),workspaces:roots.project_roots,sessions:[],editor_surfaces:[],pane_layouts:[],windows:windows.windows,preferences:{},next_route_id:1};
 const sessions=new Map(),surfaces=new Map();
 for(const root of roots.project_roots){const info=await body({GetProjectRootInfo:{id:root.id}},"ProjectRootInfo");for(const session of info.sessions)sessions.set(session.id,session);}
 const details=[];
 for(const workspace of tree.workspaces){
  await req({SubscribeWorkspace:{workspace_id:workspace.id}});
  const full=await body({RequestFullSnapshot:{since_offset:null}},"FullSnapshot"),editors=await body("ListEditorSurfaces","EditorSurfaceList");
  for(const session of full.sessions)sessions.set(session.id,session);
  for(const surface of editors.surfaces)surfaces.set(surface.surface_id,surface);
  if(full.layout)snapshot.pane_layouts.push(full.layout);
  Object.assign(snapshot.preferences,full.prefs);
  details.push({workspace:workspace.id,full,editors});
 }
 snapshot.sessions=[...sessions.values()];snapshot.editor_surfaces=[...surfaces.values()];
 snapshot.next_route_id=Math.max(1,...snapshot.editor_surfaces.map((surface:any)=>(surface.route_id||0)+1));
 save("live-tree.json",tree);save("live-details.json",details);save("migration-state.json",snapshot);
 console.log(JSON.stringify({workspaces:snapshot.host_workspaces.length,tabs:snapshot.workspace_tabs.length,sessions:snapshot.sessions.length,surfaces:snapshot.editor_surfaces.length,layouts:snapshot.pane_layouts.length,roots:snapshot.workspaces.length,windows:snapshot.windows.length}));
}finally{client.stop();}
