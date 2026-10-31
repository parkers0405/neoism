import {expect,test} from "bun:test";
import type {ServerWebSocket} from "bun";
import {DaemonConnection} from "../src/daemon/client";
import {cachedGitStatus,watchGitStatus,type GitStatusSnapshot} from "../src/daemon/git-status";
const until=async(check:()=>boolean)=>{const deadline=Date.now()+2500;while(!check()){if(Date.now()>deadline)throw new Error("Git status scope did not settle");await Bun.sleep(3);}};
function daemon(){
 const sockets=new Set<ServerWebSocket<unknown>>(),watches=new Map<ServerWebSocket<unknown>,{root:string;token:string;requestId:number}>(),requests:Array<{socket:ServerWebSocket<unknown>;family:string;message:any;root?:string}>=[];
 const snapshot=(root:string,extra:Partial<GitStatusSnapshot>={})=>({workspace_root:root,repo_root:root+"/host-repository",branch:"main",upstream:"origin/main",files:[],error:null,...extra});
 const publish=(socket:ServerWebSocket<unknown>,root:string,extra:Partial<GitStatusSnapshot>={},token?:string)=>{const watch=watches.get(socket);if(watch?.root===root)socket.send(JSON.stringify({GitReply:{request_id:watch.requestId,message:{RepoStatus:{token:token||watch.token,snapshot:snapshot(root,extra)}}}}));};
 const server=Bun.serve({hostname:"127.0.0.1",port:0,fetch(request,server){if(new URL(request.url).searchParams.get("token")!=="fixture-auth")return new Response("fixture authentication required",{status:401});return server.upgrade(request)?undefined:new Response("fixture",{status:400});},websocket:{open(socket){sockets.add(socket);},close(socket){sockets.delete(socket);watches.delete(socket);},message(socket,data){const wire=JSON.parse(String(data)),[family,envelope]=Object.entries(wire)[0] as[string,any],message=envelope.message;requests.push({socket,family,message,root:envelope.workspace_root});if(family==="Workspace"&&message.Hello)socket.send(JSON.stringify({WorkspaceReply:{request_id:envelope.request_id,message:{HelloAck:{accepted:true}}}}));else if(family==="Git"&&message.WatchStatus){watches.set(socket,{root:envelope.workspace_root,token:message.WatchStatus.token,requestId:envelope.request_id});publish(socket,envelope.workspace_root);}else if(family==="Git"&&message.UnwatchStatus){if(watches.get(socket)?.token===message.UnwatchStatus.token)watches.delete(socket);}}}});
 return{endpoint:`ws://127.0.0.1:${server.port}`,sockets,watches,requests,push(root:string,extra:Partial<GitStatusSnapshot>={},token?:string){for(const socket of sockets)publish(socket,root,extra,token);},close(){for(const socket of sockets)socket.close();server.stop(true);}};
}
test("status pill, panel and reviews share one real watched root while another root stays independently live",async()=>{
 const server=daemon(),client=new DaemonConnection({endpoint:server.endpoint,authToken:"fixture-auth"}),a:GitStatusSnapshot[]=[],panel:GitStatusSnapshot[]=[],review:GitStatusSnapshot[]=[],b:GitStatusSnapshot[]=[],cleanups:Array<()=>void>=[];
 client.start();try{
  await until(()=>client.state.phase==="connected");cleanups.push(watchGitStatus(client,"/a",snapshot=>a.push(snapshot)));await until(()=>a.length===1);
  cleanups.push(watchGitStatus(client,"/a",snapshot=>panel.push(snapshot)),watchGitStatus(client,"/a",snapshot=>review.push(snapshot)));expect(panel).toHaveLength(1);expect(review).toHaveLength(1);expect(server.watches.size).toBe(1);expect(server.sockets.size).toBe(2);
  cleanups.push(watchGitStatus(client,"/b",snapshot=>b.push(snapshot)));await until(()=>b.length===1);expect(server.watches.size).toBe(2);expect(server.sockets.size).toBe(3);
  server.push("/a",{branch:"feature",files:[{path:"real.ts",additions:3,deletions:1}]});await until(()=>a.at(-1)?.branch==="feature");expect(panel.at(-1)).toEqual(a.at(-1));expect(review.at(-1)).toEqual(a.at(-1));expect(a.at(-1)?.repo_root).toBe("/a/host-repository");expect(b.at(-1)?.branch).toBe("main");
  server.push("/b",{branch:"stale"},"wrong-token");await Bun.sleep(15);expect(b.at(-1)?.branch).toBe("main");
  const shared=cleanups.shift()!;shared();expect(server.watches.size).toBe(2);cleanups.shift()!();cleanups.shift()!();await until(()=>server.watches.size===1);await until(()=>server.sockets.size===2);
  expect(server.requests.every(request=>request.family==="Git"&&(!!request.message.WatchStatus||!!request.message.UnwatchStatus)||request.family==="Workspace"&&!!request.message.Hello)).toBe(true);expect(server.requests.filter(request=>request.message.WatchStatus)).toHaveLength(2);
 }finally{cleanups.forEach(dispose=>dispose());client.stop();server.close();}
});
test("same-root cached status survives transport gaps and remounts, but successful non-repository status clears it",async()=>{
 const server=daemon(),client=new DaemonConnection({endpoint:server.endpoint,authToken:"fixture-auth"}),seen:GitStatusSnapshot[]=[];let release:()=>void=()=>{};
 client.start();try{
  await until(()=>client.state.phase==="connected");release=watchGitStatus(client,"/a",snapshot=>seen.push(snapshot));await until(()=>seen.length===1);const original=seen[0];client.stop();expect(seen.at(-1)).toBe(original);expect(cachedGitStatus(client,"/a")).toBe(original);release();
  const remount:GitStatusSnapshot[]=[];release=watchGitStatus(client,"/a",snapshot=>remount.push(snapshot));expect(remount).toEqual([original]);expect(cachedGitStatus(client,"/b")).toBeUndefined();client.start();await until(()=>client.state.phase==="connected"&&server.watches.size===1);
  server.push("/a",{branch:null,repo_root:null,error:"temporary host collector failure"});await until(()=>remount.at(-1)?.error==="temporary host collector failure");expect(cachedGitStatus(client,"/a")?.branch).toBe("main");
  server.push("/a",{branch:null,repo_root:null,error:null});await until(()=>cachedGitStatus(client,"/a")?.repo_root===null);expect(remount.at(-1)?.branch).toBeNull();
 }finally{release();client.stop();server.close();}
});
