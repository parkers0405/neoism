import type {ServerWebSocket} from "bun";
import type {GitStatusSnapshot} from "../../src/daemon/git-status";
export const treeGitRoot="/host/repo/app",treeGitOtherRoot="/host/other";
export const treeGitEntries=[{name:"src",is_dir:true},{name:"deleted-dir",is_dir:true},...Array.from({length:90},(_,i)=>({name:`untouched-${String(i).padStart(3,"0")}.ts`,is_dir:false})),...['modified','staged','mixed','added','renamed','untracked','conflict'].map(name=>({name:`a-${name}.ts`,is_dir:false}))].sort((a,b)=>Number(a.is_dir!==b.is_dir)?Number(b.is_dir)-Number(a.is_dir):a.name.localeCompare(b.name));
const change=(path:string,status:string)=>({path,status,additions:2,deletions:1,staged:status==="Staged"});
/** Isolated wire host. Never runs Git or reads/writes any real repository. */
export function createTreeGitDaemon(){
 const sockets=new Set<ServerWebSocket<unknown>>(),watchers=new Map<ServerWebSocket<unknown>,{root:string;token:string}>(),requests:Array<{family:string;message:any;root?:string}>=[];
 const snapshots=new Map<string,GitStatusSnapshot>([[treeGitRoot,{workspace_root:treeGitRoot,repo_root:"/host/repo",branch:"main",upstream:"origin/main",error:null,files:[...['Modified','Staged','Mixed','Added','Deleted','Renamed','Untracked','Conflict'].map(status=>change(`app/a-${status.toLowerCase()}.ts`,status)),change("app/src/nested.rs","Conflict"),change("app/missing/deep/deleted.rs","Deleted"),change("app/deleted-dir/gone.ts","Deleted"),change("outside.rs","Modified")]}],[treeGitOtherRoot,{workspace_root:treeGitOtherRoot,repo_root:treeGitOtherRoot,branch:"other",upstream:null,error:null,files:[change("other.ts","Untracked")]}]]);
 const send=(socket:ServerWebSocket<unknown>,snapshot:GitStatusSnapshot,token?:string)=>{const watch=watchers.get(socket);if(watch?.root===snapshot.workspace_root)socket.send(JSON.stringify({GitReply:{request_id:0,message:{RepoStatus:{token:token||watch.token,snapshot}}}}));};
 const server=Bun.serve({hostname:"127.0.0.1",port:0,fetch(request,server){return server.upgrade(request)?undefined:new Response("Fixture only",{status:400});},websocket:{
  open(socket){sockets.add(socket);},close(socket){sockets.delete(socket);watchers.delete(socket);},
  message(socket,wire){const[family,envelope]=Object.entries(JSON.parse(String(wire)))[0]as[string,{request_id:number;message:any;workspace_root?:string}],message=envelope.message,workspaceRoot=envelope.workspace_root;requests.push({family,message,root:workspaceRoot});const reply=(value:unknown)=>socket.send(JSON.stringify({[`${family}Reply`]:{request_id:envelope.request_id,message:value}}));
   if(family==="Workspace"){reply(message.Hello?{HelloAck:{accepted:true}}:"Ack");return;}
   if(family==="Git"){
    if(message.WatchStatus){watchers.set(socket,{root:workspaceRoot!,token:message.WatchStatus.token});reply("Ack");const snapshot=snapshots.get(workspaceRoot!);if(snapshot)send(socket,snapshot);return;}
    if(message.UnwatchStatus){if(watchers.get(socket)?.token===message.UnwatchStatus.token)watchers.delete(socket);reply("Ack");return;}
    if(message==="ChangedFiles"){const snapshot=snapshots.get(workspaceRoot!);reply({ChangedFiles:{files:snapshot?.files||[],branch:snapshot?.branch||null,upstream:null,error:null}});return;}
    if(message.Log){reply({Log:{commits:[]}});return;}
    reply({Error:{message:"No fixture Git mutation permitted"}});return;
   }
   if(family==="Files"&&message.ListDir){const path=message.ListDir.path;reply({DirListing:{path,entries:path===""?(workspaceRoot===treeGitRoot?treeGitEntries:[{name:"other.ts",is_dir:false}]):path==="src"?[{name:"nested.rs",is_dir:false}]:path==="deleted-dir"?[{name:"survivor.ts",is_dir:false}]:[]}});return;}
   reply("Ack");
  }
 }});
 return{endpoint:`ws://127.0.0.1:${server.port}`,snapshots,watchers,requests,
  update(root:string,patch:Partial<GitStatusSnapshot>){const snapshot={...snapshots.get(root)!,...patch};snapshots.set(root,snapshot);for(const socket of sockets)send(socket,snapshot);},
  push(root:string,patch:Partial<GitStatusSnapshot>,token?:string){const snapshot={...snapshots.get(root)!,...patch};for(const socket of sockets)send(socket,snapshot,token);},
  pushCrdt(message:unknown){for(const socket of sockets)socket.send(JSON.stringify({CrdtReply:{request_id:0,message}}));},
  close(){for(const socket of sockets)socket.close();server.stop(true);}
 };
}
