import {expect,test} from "bun:test";
import {Effect} from "effect";
import {loadConfig} from "../src/config";
import {DaemonConnection} from "../src/daemon/client";
import {WorkspaceModel} from "../src/model";

function fixture(){
 const config=loadConfig(["--config","/nonexistent/tree-move-test"],{}),client=new DaemonConnection(config),model=new WorkspaceModel(client,config);
 const workspace={id:"move-test",host_id:"local",title:"Move test",root_dir:"/work",notes_vault_dir:"/notes"};
 const files=new Map([["/work/src/a.ts","original"],["/work/src/note.md","# Note"],["/work/dest/taken.ts","keep"]]);
 const requests:{root?:string;message:any}[]=[];
 client.request=((family:string,message:any,root?:string)=>{
  requests.push({root,message});let reply:unknown;
  if(message.ListDir){const prefix=`${root}/${message.ListDir.path?message.ListDir.path+"/":""}`,names=new Map<string,boolean>();for(const path of files.keys())if(path.startsWith(prefix)){const rest=path.slice(prefix.length);names.set(rest.split("/")[0],rest.includes("/"));}reply={DirListing:{path:message.ListDir.path,entries:[...names].map(([name,is_dir])=>({name,is_dir}))}};}
  else if(message.ReadFile)reply={FileContent:{path:message.ReadFile.path,bytes:[...new TextEncoder().encode(files.get(`${root}/${message.ReadFile.path}`)||"")]}};
  else if(message.WriteFile){files.set(`${root}/${message.WriteFile.path}`,new TextDecoder().decode(Uint8Array.from(message.WriteFile.bytes)));reply={FileWritten:{path:message.WriteFile.path}};}
  else if(message.Rename){const{from,to}=message.Rename;for(const[path,text]of [...files])if(path===`${root}/${from}`||path.startsWith(`${root}/${from}/`)){files.delete(path);files.set(`${root}/${to}`+path.slice(`${root}/${from}`.length),text);}reply={Renamed:{from,to}};}
  else throw Error(`Unexpected ${family} ${JSON.stringify(message)}`);
  return Effect.succeed(reply);
 }) as typeof client.request;
 model.update({activeWorkspace:workspace,workspaces:[workspace],loading:false,expanded:new Set(["","src","dest"])});
 return {model,files,requests,dispose:()=>{model.dispose();client.stop();}};
}
test("moving a folder rebinds open dirty code and Markdown without losing content or saving to old paths",async()=>{
 const f=fixture();try{
  await f.model.openFile("src/a.ts");const code=f.model.state.surfaces.find(s=>s.path==="src/a.ts")!,editor=f.model.getEditor(code);editor.call("paste",{text:"dirty "});const text=editor.text();
  await f.model.openFile("src/note.md");const note=f.model.state.surfaces.find(s=>s.path==="src/note.md")!,markdown=f.model.getMarkdown(note);
  await f.model.moveTreePath("/work/src","/work/dest");
  const moved=f.model.state.surfaces.find(s=>s.id===code.id)!;expect(moved.path).toBe("dest/src/a.ts");expect(f.model.getEditor(moved)).toBe(editor);expect(editor.text()).toBe(text);expect(editor.frame.dirty).toBe(true);
  expect(f.model.state.surfaces.find(s=>s.id===note.id)?.path).toBe("dest/src/note.md");expect(f.model.getMarkdown(f.model.state.surfaces.find(s=>s.id===note.id)!)).toBe(markdown);
  await f.model.saveFile(code);expect(f.files.get("/work/dest/src/a.ts")).toBe(text);expect(f.files.has("/work/src/a.ts")).toBe(false);expect(f.model.state.expanded.has("dest/src")).toBe(true);
 }finally{f.dispose();}
});
test("tree moves reject collisions and escaping/self targets before sending a rename",async()=>{
 const f=fixture();try{
  f.files.set("/work/src/taken.ts","source");
  await expect(f.model.moveTreePath("src/taken.ts","dest")).rejects.toThrow("already exists");
  await expect(f.model.moveTreePath("src","src/child")).rejects.toThrow("inside itself");
  await expect(f.model.moveTreePath("src/a.ts","../outside")).rejects.toThrow("inside this tree");
  await f.model.moveTreePath("src/a.ts","src");
  expect(f.requests.filter(r=>r.message.Rename)).toHaveLength(0);expect(f.files.get("/work/dest/taken.ts")).toBe("keep");
  await f.model.moveTreePath("src/a.ts","");expect(f.files.get("/work/a.ts")).toBe("original");
 }finally{f.dispose();}
});
test("notes moves use the vault root and do not rename equally named workspace buffers",async()=>{
 const f=fixture();try{
  f.files.set("/notes/src/a.ts","note");await f.model.openFile("src/a.ts");const code=f.model.state.surfaces.find(s=>s.path==="src/a.ts")!;
  await f.model.moveTreePath("/notes/src/a.ts","/notes",true);
  expect(f.files.get("/notes/a.ts")).toBe("note");expect(f.model.state.surfaces.find(s=>s.id===code.id)?.path).toBe("src/a.ts");expect(f.requests.find(r=>r.message.Rename)?.root).toBe("/notes");
 }finally{f.dispose();}
});
test("a directory reply started before a move cannot restore the old path afterward",async()=>{
 const f=fixture();try{
  const client=f.model.client,request=client.request.bind(client);let release!:(reply:unknown)=>void,held=false;
  client.request=((family:string,message:any,root?:string)=>{
   if(message.ListDir?.path==="src"&&!held){held=true;return Effect.promise(()=>new Promise(resolve=>{release=resolve;}));}
   return Reflect.apply(request,client,[family,message,root]);
  }) as typeof client.request;
  const old=f.model.listDirectory("src");await Bun.sleep(1);
  await f.model.moveTreePath("src/a.ts","dest");
  release({DirListing:{path:"src",entries:[{name:"a.ts",is_dir:false}]}});await old;
  expect(f.model.state.directories.src.entries.some(e=>e.name==="a.ts")).toBe(false);
  expect(f.model.state.directories.dest.entries.some(e=>e.name==="a.ts")).toBe(true);
 }finally{f.dispose();}
});
