import {expect,test} from "bun:test";
import {Effect} from "effect";
import {DaemonConnection} from "../src/daemon/client";
import {loadConfig} from "../src/config";
import {FinderController} from "../src/finder/FinderController";

test("Finder uses the owning daemon Search capability, correlation and cancellation rather than client filesystem matching",async()=>{
 const requests:Array<{id:number;tag:string;body:Record<string,unknown>;root?:string}>=[],timers=new Map<number,ReturnType<typeof setTimeout>>();
 const server=Bun.serve({port:0,hostname:"127.0.0.1",fetch(request,server){return server.upgrade(request)?undefined:new Response("Not found",{status:404});},websocket:{message(socket,data){
  const [family,envelope]=Object.entries(JSON.parse(String(data)))[0] as [string,{request_id:number;message:unknown;workspace_root?:string}];
  const tag=typeof envelope.message==="string"?envelope.message:Object.keys(envelope.message as object)[0]!,body=typeof envelope.message==="string"?{}:(envelope.message as Record<string,Record<string,unknown>>)[tag]!;
  if(family==="Workspace"&&tag==="Hello"){socket.send(JSON.stringify({Workspace:{request_id:envelope.request_id,message:{HelloAck:{accepted:true}}}}));return;}
  if(family!=="Search")return;requests.push({id:envelope.request_id,tag,body,root:envelope.workspace_root});
  if(tag==="CancelSearch"){clearTimeout(timers.get(Number(body.req_id)));timers.delete(Number(body.req_id));return;}
  const reply=()=>{timers.delete(envelope.request_id);socket.send(JSON.stringify({Search:{request_id:envelope.request_id,message:tag==="CollectFiles"?{CollectFilesResult:{req_id:body.req_id,paths:["src/found.rs"]}}:{SearchFilesResult:{req_id:body.req_id,hits:[{score:1234,path:`src/${body.query||"initial"}.rs`}]}}}}));};
  timers.set(envelope.request_id,setTimeout(reply,body.query==="old"?250:5));
 }}});
 const config={...loadConfig(["--config","/nonexistent/finder-transport"],{}),endpoint:`ws://127.0.0.1:${server.port}/session`,authToken:undefined,pairingToken:undefined};
 const client=new DaemonConnection(config);let finder:FinderController|undefined;
 const until=async(predicate:()=>boolean)=>{const limit=Date.now()+3000;while(!predicate()){if(Date.now()>limit)throw new Error("Finder transport did not settle");await Bun.sleep(5);}};
 try{
  client.start();await until(()=>client.state.phase==="connected");
  finder=new FinderController({cwd:"/owning/daemon/project",mode:"files",search:(message,signal)=>Effect.runPromise(client.request("Search",message,"/owning/daemon/project"),{signal})});
  await until(()=>finder!.frame.rows[0]?.path==="src/initial.rs");
  finder.query("old");await until(()=>requests.some(request=>request.tag==="SearchFiles"&&request.body.query==="old"));
  const old=requests.find(request=>request.tag==="SearchFiles"&&request.body.query==="old")!;
  finder.query("new");await until(()=>finder!.frame.rows[0]?.path==="src/new.rs");
  await until(()=>requests.some(request=>request.tag==="CancelSearch"&&request.body.req_id===old.id));
  finder.cycle();await until(()=>requests.some(request=>request.tag==="SearchFiles"&&request.body.mode==="Exact")&&finder!.selectedTarget()?.path==="src/new.rs");
  expect(requests.every(request=>request.root==="/owning/daemon/project")).toBe(true);
  expect(requests.filter(request=>request.tag==="SearchFiles").every(request=>request.body.cwd===""&&request.body.req_id===request.id)).toBe(true);
  expect(requests.some(request=>request.tag==="SearchFiles"&&request.body.mode==="Fuzzy")).toBe(true);
  expect(finder.selectedTarget()).toMatchObject({mode:"files",path:"src/new.rs"});
  await Bun.sleep(270);expect(finder.frame.rows[0]?.path).toBe("src/new.rs");
 }finally{finder?.dispose();client.stop();for(const timer of timers.values())clearTimeout(timer);server.stop(true);}
});
