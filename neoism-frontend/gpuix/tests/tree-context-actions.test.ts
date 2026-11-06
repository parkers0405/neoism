import {expect,test} from "bun:test";
import * as Effect from "effect/Effect";
import {posix} from "node:path";
import {loadConfig} from "../src/config";
import {DaemonConnection} from "../src/daemon/client";
import {WorkspaceModel} from "../src/model";
import {performTreeContext,type TreeContextTarget} from "../src/ui/tree-context-actions";
import {installCrdtFixture} from "./support/crdt-fixture";

function fixture(){
 const config=loadConfig(["--config","/nonexistent/context-test"],{}),client=new DaemonConnection(config);
 const workspace={id:"a",host_id:"remote",title:"A",root_dir:"/remote",notes_vault_dir:"/vault"};
 const files=new Map<string,string|null>([["/remote/src",null],["/remote/src/a.ts","original"],["/remote/dest",null],["/remote/dest/a.ts","keep"],["/vault",null]]);
 const requests:any[]=[];
 client.request=((family:string,message:any,root:string)=>Effect.try({try:()=>{
  requests.push({family,message,root});const path=(p:string)=>posix.join(root,p);const entry=(p:string)=>({name:posix.basename(p),is_dir:files.get(p)===null,size:0});
  if(message.ListDir)return{DirListing:{path:message.ListDir.path,entries:[...files.keys()].filter(p=>posix.dirname(p)===path(message.ListDir.path)).map(entry)}};
  if(message.Stat){const p=path(message.Stat.path);if(!files.has(p))throw Error("Missing path");return{Stat:{path:message.Stat.path,entry:entry(p)}};}
  if(message.ReadFile){const p=path(message.ReadFile.path);if(typeof files.get(p)!=="string")throw Error("Missing file");return{FileContent:{path:message.ReadFile.path,bytes:[...new TextEncoder().encode(files.get(p)!)]}};}
  if(message.WriteFile){files.set(path(message.WriteFile.path),new TextDecoder().decode(Uint8Array.from(message.WriteFile.bytes)));return{FileWritten:{path:message.WriteFile.path}};}
  if(message.CreateFile||message.CreateDir){const spec=message.CreateFile||message.CreateDir,p=path(posix.join(spec.dir,spec.name));if(files.has(p))throw Error("Already exists");files.set(p,message.CreateDir?null:"");return{FileCreated:{path:posix.join(spec.dir,spec.name),is_dir:!!message.CreateDir}};}
  if(message.Delete){const p=path(message.Delete.path);for(const key of [...files.keys()])if(key===p||key.startsWith(p+"/"))files.delete(key);return{Deleted:{path:message.Delete.path,was_dir:false}};}
  if(message.Rename){const{from,to}=message.Rename;for(const[key,content]of [...files])if(key===path(from)||key.startsWith(path(from)+"/")){files.delete(key);files.set(path(to)+key.slice(path(from).length),content);}return{Renamed:{from,to}};}
  throw Error(`Unexpected ${family} ${JSON.stringify(message)}`);
 },catch:error=>error})) as typeof client.request;
 const documents=installCrdtFixture(client,path=>files.get(path)??undefined,(path,text)=>files.set(path,text)),model=new WorkspaceModel(client,config);
 model.update({activeWorkspace:workspace,workspaces:[workspace],loading:false,expanded:new Set(["","src","dest"])});
 const target=(path?:string,directory="",notes=false):TreeContextTarget=>({workspaceId:"a",root:notes?"/vault":"/remote",notes,path,directory});
 return{model,files,requests,target,dispose:()=>{model.dispose();client.stop();documents.dispose();}};
}
test("tree copy/paste duplicates files and folders through the daemon without overwriting collisions",async()=>{
 const f=fixture();try{
  await performTreeContext(f.model,f.target("src/a.ts","src"),"copy");await performTreeContext(f.model,f.target(undefined,"dest"),"paste");expect(f.files.get("/remote/dest/a copy.ts")).toBe("original");expect(f.files.get("/remote/dest/a.ts")).toBe("keep");
  await performTreeContext(f.model,f.target("src","src"),"copy");await performTreeContext(f.model,f.target(),"paste");expect(f.files.get("/remote/src copy/a.ts")).toBe("original");
  await expect(performTreeContext(f.model,f.target("src","src"),"paste")).rejects.toThrow("inside itself");
 }finally{f.dispose();}
});
test("tree creation, rename and delete retain dirty editor identity and isolate notes roots",async()=>{
 const f=fixture();try{
  await f.model.openFile("src/a.ts");const surface=f.model.state.surfaces.find(s=>s.path==="src/a.ts")!,editor=f.model.getEditor(surface);editor.call("paste",{text:"dirty "});const content=editor.text();
  await performTreeContext(f.model,f.target("src/a.ts","src"),"rename","renamed.ts");const renamed=f.model.state.surfaces.find(s=>s.id===surface.id)!;expect(renamed.path).toBe("src/renamed.ts");expect(f.model.getEditor(renamed)).toBe(editor);expect(editor.text()).toBe(content);expect(editor.frame.dirty).toBe(true);
  await f.model.saveFile(renamed);expect(f.files.get("/remote/src/renamed.ts")).toBe(content);expect(f.files.has("/remote/src/a.ts")).toBe(false);
  await performTreeContext(f.model,f.target(undefined,"",true),"new-folder","notes");await performTreeContext(f.model,f.target(undefined,"notes",true),"new-file","hello.md");expect(f.files.has("/vault/notes/hello.md")).toBe(true);expect(f.files.has("/remote/notes/hello.md")).toBe(false);
  await performTreeContext(f.model,f.target("notes","",true),"delete");expect(f.files.has("/vault/notes/hello.md")).toBe(false);expect(f.model.state.surfaces.some(surface=>surface.rootDir==="/vault"&&surface.path==="notes/hello.md")).toBe(false);
  const before=f.requests.length;f.model.update({activeWorkspace:{...f.model.state.activeWorkspace!,id:"b"}});await expect(performTreeContext(f.model,f.target("src/renamed.ts"),"delete")).rejects.toThrow("different workspace");expect(f.requests.length).toBe(before);
 }finally{f.dispose();}
});
