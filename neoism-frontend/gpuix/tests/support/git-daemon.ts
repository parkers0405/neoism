import type {ServerWebSocket} from "bun";
export interface GitFixtureRequest{family:string;message:unknown;workspaceRoot?:string}
/** Isolated real daemon wire protocol. Every Git mutation changes this fixture
 * state only; no git executable or user repository is accessed. */
export function createGitDaemon(options:{pullRequests?:boolean}={}){
 const requests:GitFixtureRequest[]=[],sockets=new Set<ServerWebSocket<unknown>>(),watchers=new Map<string,{socket:ServerWebSocket<unknown>;root:string}>();
 let heldDiff:{received:()=>void;reply?:()=>void}|undefined;
 const fileContents=new Map([["src/main.ts","const value = 2;\nconsole.log(value);\n"]]);
 let restoreError:string|undefined;
 let staged=false,branch="feature/fixture",upstream:string|null=null,commitError:string|undefined;
 const patch="diff --git a/src/main.ts b/src/main.ts\n--- a/src/main.ts\n+++ b/src/main.ts\n@@ -1,2 +1,2 @@\n-const value = 1;\n+const value = 2;\n console.log(value);\n";
 let prNumber=7,prTitle="Fixture pull request",prBody="Original PR body",merged=false;
 const summary=()=>({number:prNumber,title:prTitle,url:`https://example.test/fixture/pull/${prNumber}`,author:"fixture",state:merged?"MERGED":"OPEN",draft:false,base_branch:"main",head_branch:branch,base_sha:"b".repeat(40),head_sha:"a".repeat(40),review_decision:"REVIEW_REQUIRED",checks_state:"SUCCESS"});
 const detail=()=>({summary:summary(),body:prBody,files:files(),diffs:[{path:"src/main.ts",patch}],conversation:[],threads:[],checks:[],truncated:false});
 const item=()=>({summary:summary(),repository:"fixture/repo",updated_at:"2026-10-28T00:00:00Z",additions:1,deletions:1,labels:[],reviewers:[],authored:true,review_requested:false,assigned:false});
 const files=()=>[{path:"src/main.ts",status:staged?"Staged":"Modified",additions:1,deletions:1,staged},{path:"new.txt",status:"Untracked",additions:1,deletions:0,staged:false}];
 const changed=(error:string|null=null)=>({ChangedFiles:{files:files(),branch,upstream,error}});
 const snapshot=(root:string)=>({workspace_root:root,repo_root:root,files:files(),branch,upstream,error:null});
 const push=()=>{for(const[token,watch]of watchers)watch.socket.send(JSON.stringify({GitReply:{request_id:0,message:{RepoStatus:{token,snapshot:snapshot(watch.root)}}}}));};
 const server=Bun.serve({hostname:"127.0.0.1",port:0,fetch(request,server){return server.upgrade(request)?undefined:new Response("Fixture wire protocol",{status:400});},websocket:{
  open(socket){sockets.add(socket);},close(socket){sockets.delete(socket);for(const[token,watch]of watchers)if(watch.socket===socket)watchers.delete(token);},
  message(socket,data){const wire=JSON.parse(String(data)),[family,envelope]=Object.entries(wire)[0] as[string,{request_id:number;message:unknown;workspace_root?:string}];const message=envelope.message,record=typeof message==="object"&&message!==null?message as Record<string,any>:{};
   requests.push({family,message,workspaceRoot:envelope.workspace_root});const reply=(message:unknown)=>socket.send(JSON.stringify({[family+"Reply"]:{request_id:envelope.request_id,message}}));
   if(family==="Workspace"){if(record.Hello)reply({HelloAck:{accepted:true}});else reply("Ack");return;}
   if(family==="Files"){if(record.WriteFile){fileContents.set(record.WriteFile.path,new TextDecoder().decode(Uint8Array.from(record.WriteFile.bytes)));reply({FileWritten:{path:record.WriteFile.path}});}else if(record.ReadFile)reply({FileContent:{path:record.ReadFile.path,bytes:[...new TextEncoder().encode(fileContents.get(record.ReadFile.path)||"")]}});else reply("Ack");return;}
   if(family!=="Git"){reply("Ack");return;}
   if(record.WatchStatus){watchers.set(record.WatchStatus.token,{socket,root:envelope.workspace_root||"/fixture"});reply("Ack");push();return;}
   if(record.UnwatchStatus){watchers.delete(record.UnwatchStatus.token);reply("Ack");return;}
   if(message==="ChangedFiles"){reply(changed());return;}
   if(record.DiffFiles){const respond=()=>reply({FileDiffs:{diffs:record.DiffFiles.paths.map((path:string)=>({path,patch:path==="src/main.ts"?patch:"diff --git a/new.txt b/new.txt\n--- /dev/null\n+++ b/new.txt\n@@ -0,0 +1 @@\n+new file\n"}))}});if(heldDiff){heldDiff.reply=respond;heldDiff.received();}else respond();return;}
   if(record.ReadBase){reply({BaseFile:{path:record.ReadBase.path,text:"const value = 1;\nconsole.log(value);\n"}});return;}
   if(record.Stage||record.Unstage){staged=!!record.Stage;reply(changed());push();return;}
   if(record.Restore||record.ApplyBlock){if(record.Restore||record.ApplyBlock.restore){if(restoreError){reply({Error:{message:restoreError}});return;}fileContents.set((record.Restore||record.ApplyBlock).path,"const value = 1;\nconsole.log(value);\n");}reply(changed());push();return;}
   if(record.Commit){reply(changed(commitError||null));return;}
   if(message==="Branches"){reply({Branches:{branches:["main","feature/fixture","other"]}});return;}
   if(record.Checkout){branch=record.Checkout.branch;reply(changed());push();return;}
   if(message==="Publish"){upstream="origin/"+branch;reply(changed());return;}
   if(["Fetch","Pull","Push"].includes(String(message))){reply(changed());return;}
   if(record.Log){reply({Log:{commits:[{sha:"abc123def456",short_sha:"abc123d",author:"Fixture Author",message:"Original commit",timestamp:1_700_000_000}]}});return;}
   if(record.ReadCommit){reply({CommitDetail:{detail:{sha:record.ReadCommit.revision,author:"Fixture Author",email:"fixture@example.test",timestamp:1_700_000_000,offset_minutes:0,message:"Original commit\n\nFull body",files:files(),diffs:[{path:"src/main.ts",patch}]}}});return;}
   if(record.PullRequest){
    const request=record.PullRequest.request,kind=Object.keys(request)[0],body=request[kind],respond=(value:unknown)=>reply({PullRequest:{reply:value}});
    if(!options.pullRequests){respond({Error:{kind:"Unavailable",message:"Fixture has no hosted pull request"}});return;}
    if(body.expected_head&&body.expected_head!=="a".repeat(40)){respond({Error:{kind:"Stale",message:"Fixture PR head changed"}});return;}
    switch(kind){
     case"Discover":respond({Discovered:{branch,head:"a".repeat(40),default_branch:"main",pull_request:summary()}});break;
     case"List":respond({Listed:{page:{repository:"fixture/repo",viewer:"fixture",generation:body.generation,items:[item()],next_cursor:null,total_count:1}}});break;
     case"Read":respond({Detail:{detail:detail()}});break;
     case"ReadInbox":respond({InboxDetail:{detail:detail(),item:item()}});break;
     case"Create":if(body.expected_branch!==branch){respond({Error:{kind:"Stale",message:"Fixture branch changed"}});break;}prNumber=8;prTitle=body.title;prBody=body.body;respond({Created:{pull_request:summary()}});break;
     case"SubmitReview":respond({ReviewSubmitted:{number:body.number}});break;
     case"ReplyThread":case"ResolveThread":respond({ThreadUpdated:{number:body.number}});break;
     case"Checkout":respond({CheckedOut:{number:body.number,path:envelope.workspace_root+(body.new_worktree?"/.worktrees/pr":""),worktree:body.new_worktree}});break;
     case"Merge":merged=true;respond({Merged:{number:body.number}});break;
     default:respond({Error:{kind:"Invalid",message:"Unsupported fixture PR operation"}});
    }return;
   }
   reply({Error:{message:"Unsupported fixture Git message"}});
  }
 }});
 return{endpoint:`ws://127.0.0.1:${server.port}`,requests,files,push,setRestoreError(value:string|undefined){restoreError=value;},holdNextDiff(){if(heldDiff)throw new Error("A fixture diff is already held");let received!:()=>void;const pending=new Promise<void>(resolve=>{received=resolve;});const held={received,reply:undefined as undefined|(()=>void)};heldDiff=held;return{received:pending,release(){if(heldDiff!==held)return;heldDiff=undefined;held.reply?.();}};},setCommitError(value:string|undefined){commitError=value;},close(){for(const socket of sockets)socket.close();server.stop(true);}};
}
