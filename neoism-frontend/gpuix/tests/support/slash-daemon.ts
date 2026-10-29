import type {Session} from "@neoism/sdk";
export function createSlashDaemon(){
 const requests:Array<{workspace:string;path:string;method:string;body:any;directory:string|null;authorization:string|null}>=[];
 const session=(id:string,workspace="a",model={providerId:"openai",id:"base",variant:"high",connectionId:"account-a"}):Session=>({id,title:id,directory:`/project/${workspace}`,projectId:`project-${workspace}`,slug:id,version:"1",time:{created:1,updated:1},agent:"build",model,permission:[{permission:"read",pattern:"*",action:"ask"}]});
 const sessions=new Map([['one',session('one')],['two',{...session('two','a',{providerId:"other",id:"second",variant:"low",connectionId:"account-b"}),parentId:"one"}]]);
 const capabilities=new Set(["neoism.goals","neoism.mcp","neoism.subagents"]);
 const permissions=[{id:"p-one",sessionId:"one",messageId:"m",permission:"read",patterns:["*"],always:["*"],title:"Read file"},{id:"p-two",sessionId:"two",messageId:"m",permission:"read",patterns:["*"],always:["*"],title:"Read other file"}];
 const questions=[{id:"q-one",sessionId:"one",messageId:"m",questions:[{question:"First?"},{question:"Second?"}]},{id:"q-two",sessionId:"two",messageId:"m",questions:[{question:"Other?"}]}];
 let goal:any=null,connected=false,failCommand=false,deferCreation=false,count=0;
 const creations:Array<()=>void>=[];
 const deferred=new Set<string>(),releases=new Map<string,Array<()=>void>>();
 const release=(path:string)=>{deferred.delete(path);for(const done of releases.get(path)||[])done();releases.delete(path);};
 const server=Bun.serve({hostname:"127.0.0.1",port:0,async fetch(request,server){
  const url=new URL(request.url);if(url.pathname==="/session")return server.upgrade(request)?undefined:new Response("",{status:400});
  const route=url.pathname.match(/^\/agent\/workspaces\/([^/]+)\/v2(.*)$/);if(!route)return new Response("Not found",{status:404});
  const workspace=decodeURIComponent(route[1]),path=route[2],body=await request.json().catch(()=>undefined);
  requests.push({workspace,path,method:request.method,body,directory:url.searchParams.get("directory"),authorization:request.headers.get("authorization")});
  if(deferred.has(path))await new Promise<void>(resolve=>{const list=releases.get(path)||[];list.push(resolve);releases.set(path,list);});
  if(path==="/providers")return Response.json({all:[{id:"openai",name:"OpenAI",env:[],models:{},options:{},source:"builtin"}],connected:["openai"],default:{}});
  if(path==="/providers/auth-methods")return Response.json({openai:[{type:"api",label:"API key"},{type:"oauth",label:"Browser sign in",prompts:[{key:"region",message:"Region",type:"select",options:[{value:"us",label:"US"}]},{key:"tenant",message:"Tenant",type:"text",condition:{key:"region",op:"eq",value:"us"}}]}]});
  if(path==="/providers/openai/connections")return Response.json({providerId:"openai",connectionId:"added",label:body.label,scope:"workspace",authType:"api",isDefault:false});
  if(path==="/providers/openai/oauth/authorize")return Response.json({attemptId:"attempt-one",instructions:"Sign in with your browser",method:"code",url:"https://auth.example/sign-in",expiresAt:Date.now()+60000});
  if(path==="/providers/openai/oauth/callback")return Response.json(true);
  if(path==="/providers/openai/usage")return Response.json({accounts:[{auth_type:"oauth",connection_id:"account-a",error:null,is_default:true,label:"Personal",plan_type:"pro",windows:[{label:"5 hours",limit_window_seconds:18000,reset_at:1900000000,used_percent:24}]}]});
  if(path==="/commands")return Response.json([{name:"review",description:"Review this repository"},{name:"compact",description:"Duplicate builtin"},{name:"models",description:"Duplicate alias"}]);
  if(path==="/capabilities")return Response.json([...capabilities].map(id=>({id,enabled:true,version:"1.0.0",source:"plugin",disableable:true})));
  if(path==="/skills")return Response.json([{name:"review",description:"Repository review",path:"/project/a/.neoism/skills/review.md"}]);
  if(path==="/directories"){const directory=url.searchParams.get("path")||`/project/${workspace}`;return Response.json({path:directory,parent:"/project",entries:[{name:"sub",path:`${directory}/sub`}]});}
  if(path==="/sessions"&&request.method==="GET")return Response.json({items:[...sessions.values()],cursor:{next:null,hasMore:false}});
  if(path==="/sessions"&&request.method==="POST"){
   const created={...session(`created-${++count}`,workspace),directory:url.searchParams.get("directory")||`/project/${workspace}`,...body};sessions.set(created.id,created);
   if(deferCreation)return new Promise<Response>(resolve=>creations.push(()=>resolve(Response.json(created))));return Response.json(created);
  }
  const match=path.match(/^\/sessions\/([^/]+)(?:\/(.*))?$/);
  if(match){const id=decodeURIComponent(match[1]),suffix=match[2],current=sessions.get(id)||session(id,workspace);
   if(!suffix){if(request.method==="PATCH"){const next={...current,...body};sessions.set(id,next);return Response.json(next);}return Response.json(current);}
   if(suffix==="commands"){if(failCommand){failCommand=false;return Response.json({message:"Command failed"},{status:500});}return Response.json({info:{id:"command-result",sessionId:id,role:"assistant"},parts:[{type:"text",text:"Actual command result"}]});}
   if(suffix==="prompt")return new Response(null,{status:204});
   if(suffix==="queue")return Response.json(request.method==="DELETE"?{sessionId:id,removed:2,queue:{count:0,items:[]}}:{sessionId:id,count:2,items:[{index:0,text:"queued"}]});
   if(suffix==="queue/pop")return Response.json({sessionId:id,removed:1,queue:{count:1,items:[]}});
   if(suffix==="undo"||suffix==="redo")return Response.json(current);
   if(suffix==="summarize"||suffix==="abort")return Response.json(true);
  }
  if(path.startsWith("/plugins/dev.neoism.goals/")){
   if(request.method==="DELETE")goal=null;
   else if(request.method==="POST")goal={created:1,updated:Date.now(),status:"active",summary:"",research:[],text:goal?.text||"",paused:goal?.paused||false,...body};
   return Response.json({goal,researchEnabled:true});
  }
  if(path==="/interactions/permissions")return Response.json(permissions.filter(p=>!url.searchParams.get("sessionId")||p.sessionId===url.searchParams.get("sessionId")));
  if(path==="/interactions/questions")return Response.json(questions.filter(q=>!url.searchParams.get("sessionId")||q.sessionId===url.searchParams.get("sessionId")));
  const permission=path.match(/^\/interactions\/permissions\/([^/]+)\/reply$/);if(permission){const index=permissions.findIndex(p=>p.id===permission[1]);if(index>=0)permissions.splice(index,1);return Response.json(true);}
  const question=path.match(/^\/interactions\/questions\/([^/]+)\/(reply|reject)$/);if(question){const index=questions.findIndex(q=>q.id===question[1]);if(index>=0)questions.splice(index,1);return Response.json(true);}
  if(path==="/plugins/dev.neoism.mcp/catalog")return Response.json({local:{configScope:"workspace",configWritable:true,enabled:true,hasCredentials:false,oauthCapable:false,runtimeConnected:connected,status:{status:connected?"connected":"disabled"}}});
  if(path.includes("/plugins/dev.neoism.mcp/local/connect")){connected=true;return Response.json(true);}
  if(path.includes("/plugins/dev.neoism.mcp/local/disconnect")){connected=false;return Response.json(true);}
  if(path.startsWith("/plugins/dev.neoism.subagents/sessions/"))return Response.json([{id:"task",sessionId:"one",childSessionId:"two",agent:"plan",description:"Inspect source",status:"done",nested:false}]);
  return Response.json({message:`Unhandled fixture operation ${request.method} ${path}`},{status:404});
 },websocket:{message(socket,data){const [family,envelope]=Object.entries(JSON.parse(String(data)))[0]as[string,{request_id:number;message:any}];const reply=(message:unknown)=>socket.send(JSON.stringify({[family+"Reply"]:{request_id:envelope.request_id,message}}));if(envelope.message.Hello)reply({HelloAck:{accepted:true}});else if(envelope.message.GetHistory)reply({HistoryChunk:{session_id:envelope.message.GetHistory.session_id,messages:[]}});}}});
 return {defer:(path:string)=>{deferred.add(path);},release,endpoint:`ws://127.0.0.1:${server.port}/session`,requests,sessions,capabilities,creations,setFailCommand:()=>{failCommand=true;},deferCreation:()=>{deferCreation=true;},stop:()=>{for(const path of deferred)release(path);server.stop(true);}};
}
