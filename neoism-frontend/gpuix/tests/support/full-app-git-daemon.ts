import{createAgentDaemon}from"./agent-daemon";
/** Minimal repository state behind the full production Workspace/Files/Agent
 * fixture. All requests stay inside this in-memory daemon; no git process. */
export function createFullAppGitDaemon(){
 const root="/project/a",path="src/main.ts",patch="diff --git a/src/main.ts b/src/main.ts\n--- a/src/main.ts\n+++ b/src/main.ts\n@@ -1,2 +1,2 @@\n-const value = 1;\n+const value = 2;\n console.log(value);\n";
 const watchers=new Map<string,string>();let staged=false;
 const files=()=>[{path,status:staged?"Staged":"Modified",additions:1,deletions:1,staged}];
 const status=(scope:string)=>({workspace_root:scope,repo_root:scope,branch:"feature/fixture",upstream:"origin/feature",files:files(),error:null});
 const changed=()=>({ChangedFiles:{files:files(),branch:"feature/fixture",upstream:"origin/feature",error:null}});
 const summary=()=>({number:7,title:"Fixture pull request",url:"https://example.test/repo/pull/7",author:"fixture",state:"OPEN",draft:false,base_branch:"main",head_branch:"feature/fixture",base_sha:"b".repeat(40),head_sha:"a".repeat(40),review_decision:"REVIEW_REQUIRED",checks_state:"SUCCESS"});
 const detail=()=>({summary:summary(),body:"Original PR body",files:files(),diffs:[{path,patch}],conversation:[],threads:[],checks:[],truncated:false});
 const daemon=createAgentDaemon({deferCreations:false,gitHandler(message,scope,reply){const body=typeof message==="object"&&message!==null?message as Record<string,any>:{};if(body.WatchStatus){watchers.set(body.WatchStatus.token,scope||root);reply("Ack");daemon.pushGit({RepoStatus:{token:body.WatchStatus.token,snapshot:status(scope||root)}});return;}if(body.UnwatchStatus){watchers.delete(body.UnwatchStatus.token);reply("Ack");return;}
  if(message==="ChangedFiles"){reply(changed());return;}if(body.DiffFiles){reply({FileDiffs:{diffs:body.DiffFiles.paths.map((path:string)=>({path,patch}))}});return;}if(body.ReadBase){reply({BaseFile:{path:body.ReadBase.path,text:"const value = 1;\nconsole.log(value);\n"}});return;}
  if(body.Stage||body.Unstage){staged=!!body.Stage;reply(changed());return;}if(body.Restore||body.ApplyBlock){daemon.files.set(path,"const value = 1;\nconsole.log(value);\n");reply(changed());return;}
  if(body.Log){reply({Log:{commits:[{sha:"abc123def456",short_sha:"abc123d",author:"Fixture Author",message:"Original commit",timestamp:1700000000}]}});return;}if(body.ReadCommit){reply({CommitDetail:{detail:{sha:body.ReadCommit.revision,author:"Fixture Author",email:"fixture@example.test",timestamp:1700000000,offset_minutes:0,message:"Original commit\n\nFull body",files:files(),diffs:[{path,patch}]}}});return;}
  if(body.PullRequest){const request=body.PullRequest.request;let response:unknown={Error:{kind:"Invalid",message:"Unsupported full App fixture request"}};if(request.Discover)response={Discovered:{branch:"feature/fixture",head:"a".repeat(40),default_branch:"main",pull_request:summary()}};else if(request.List)response={Listed:{page:{repository:"fixture/repo",viewer:"fixture",generation:request.List.generation,items:[{summary:summary(),repository:"fixture/repo",updated_at:"2026-10-28T00:00:00Z",additions:1,deletions:1,labels:[],reviewers:[],authored:true,review_requested:false,assigned:false}],next_cursor:null,total_count:1}}};else if(request.Read)response={Detail:{detail:detail()}};else if(request.ReadInbox)response={InboxDetail:{detail:detail(),item:{summary:summary(),repository:"fixture/repo",updated_at:"2026-10-28T00:00:00Z",additions:1,deletions:1,labels:[],reviewers:[],authored:true,review_requested:false,assigned:false}}};reply({PullRequest:{reply:response}});return;}
  if(message==="Branches"){reply({Branches:{branches:["main","feature/fixture"]}});return;}reply("Ack");
 }});daemon.files.set(path,"const value = 2;\nconsole.log(value);\n");
 return{...daemon,root,path,patch,filesChanged:files,pushStatus(){for(const[token,scope]of watchers)daemon.pushGit({RepoStatus:{token,snapshot:status(scope)}});}};
}
