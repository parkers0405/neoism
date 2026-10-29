import {expect,test} from "bun:test";
import {loadConfig} from "../src/config";
import {DaemonConnection} from "../src/daemon/client";
import {PresenceController,collaborativeWorkspace,type PresencePeer} from "../src/collaboration/PresenceController";
import {coreCall} from "../src/native-core";
import type {ServerWebSocket} from "bun";
const until=async(check:()=>boolean)=>{const end=Date.now()+3000;while(!check()){if(Date.now()>end)throw new Error("Presence clients did not converge");await Bun.sleep(5);}};

test("two real websocket clients publish original presence, hide self, clear scope and recover after reconnect",async()=>{
 const sockets=new Set<ServerWebSocket<unknown>>(),owned=new Map<ServerWebSocket<unknown>,Set<string>>(),peers=new Map<string,PresencePeer>();const sent:unknown[]=[];
 const broadcast=(message:unknown)=>{for(const socket of sockets)socket.send(JSON.stringify({CrdtReply:{request_id:0,message}}));};
 const server=Bun.serve({hostname:"127.0.0.1",port:0,fetch:(request,server)=>server.upgrade(request)?undefined:new Response("",{status:400}),websocket:{
  open(socket){sockets.add(socket);owned.set(socket,new Set());},
  close(socket){sockets.delete(socket);for(const [key,peer]of peers)if(owned.get(socket)?.has(peer.peer_id)){peers.delete(key);broadcast({Presence:{update:{Remove:{buffer_id:peer.buffer_id,peer_id:peer.peer_id}}}});}owned.delete(socket);},
  message(socket,data){const [family,envelope]=Object.entries(JSON.parse(String(data)))[0] as [string,any];
   const reply=(message:unknown)=>socket.send(JSON.stringify({[family+"Reply"]:{request_id:envelope.request_id,message}}));
   if(family==="Workspace"){if(envelope.message.Hello)reply({HelloAck:{accepted:true}});return;}
   sent.push(envelope.message);
   if(envelope.message.PublishPresence){const peer=envelope.message.PublishPresence.presence as PresencePeer;peers.set(`${peer.buffer_id}\0${peer.peer_id}`,peer);owned.get(socket)!.add(peer.peer_id);broadcast({Presence:{update:{Upsert:peer}}});}
   if(envelope.message.ClearPresence){const p=envelope.message.ClearPresence;peers.delete(`${p.buffer_id}\0${p.peer_id}`);broadcast({Presence:{update:{Remove:p}}});}
   if(envelope.message.RequestPresenceSnapshot){const p=envelope.message.RequestPresenceSnapshot;reply({PresenceSnapshot:{buffer_id:p.buffer_id,peers:[...peers.values()].filter(peer=>peer.buffer_id===p.buffer_id&&peer.peer_id!==p.exclude_peer_id)}});}
  }
 }});
 const config={...loadConfig(["--config","/nonexistent/presence"],{}),endpoint:`ws://127.0.0.1:${server.port}/session`,collaborative:true};
 const a=new DaemonConnection(config),b=new DaemonConnection(config);
 const alice=new PresenceController(a,config,{peerId:"alice@linux",displayName:"Alice",color:[10,20,30]}),bob=new PresenceController(b,config,{peerId:"bob@windows",displayName:"Bob",color:[40,50,60]});
 const workspace={id:"shared",host_id:"remote",title:"Shared",visibility:"shared"};
 try{
  a.start();b.start();await until(()=>a.state.phase==="connected"&&b.state.phase==="connected");alice.setWorkspace(workspace);bob.setWorkspace(workspace);
  await until(()=>alice.peers().length===1&&bob.peers().length===1);expect(alice.peers()[0].peer_id).toBe("bob@windows");expect(bob.peers()[0].peer_id).toBe("alice@linux");
  alice.updateLocal("file:///project/a.md",{line:3,column:4},{anchor:{line:2,column:1},head:{line:3,column:4}},true);
  bob.updateLocal("file:///project/a.md",{line:7,column:2});
  await until(()=>bob.filePeers("file:///project/a.md").length===1);
  expect(bob.filePeers("file:///project/a.md")[0]).toMatchObject({peer_id:"alice@linux",display_name:"Alice",cursor:{line:3,column:4},selection:{anchor:{line:2,column:1},head:{line:3,column:4}},insert:true});
  expect(alice.filePeers("file:///project/a.md").some(peer=>peer.peer_id==="alice@linux")).toBe(false);
  expect(bob.bufferId("/project/a.md")).toBe("file:///project/a.md");const treeRow=bob.row("file:///project/a.md",config);expect(treeRow.width).toBeGreaterThan(0);expect(treeRow.commands.length).toBeGreaterThan(0);expect(bob.row("file:///another.md",config).width).toBe(0);
  const before=sent.length;for(let i=0;i<20;i++)alice.updateLocal("file:///project/a.md",{line:3,column:i});await Bun.sleep(100);
  await until(()=>bob.filePeers("file:///project/a.md")[0]?.cursor.column===19);
  expect(sent.slice(before).filter((message:any)=>message.PublishPresence?.presence.peer_id==="alice@linux").length).toBeLessThanOrEqual(2);
  alice.clearLocal();await until(()=>!bob.filePeers("file:///project/a.md").length);expect(bob.peers()).toHaveLength(1);
  a.stop();await until(()=>bob.peers().length===0);expect(alice.peers()).toHaveLength(0);
  a.start();await until(()=>a.state.phase==="connected"&&bob.peers().length===1&&alice.peers().length===1);
  alice.leaveWorkspace();await until(()=>bob.peers().length===0);expect(alice.peers()).toHaveLength(0);
  alice.setWorkspace({...workspace,visibility:"private"});await until(()=>bob.peers().length===1); // Joined remote session remains collaborative.
 }finally{alice.dispose();bob.dispose();a.stop();b.stop();server.stop(true);}
},10000);

test("local private workspaces stay private while explicitly shared local workspaces join presence",()=>{
 const workspace={id:"local",host_id:"host",title:"Local",visibility:"private"};
 expect(collaborativeWorkspace({endpoint:"ws://127.0.0.1:7878/session",collaborative:false},workspace)).toBe(false);
 expect(collaborativeWorkspace({endpoint:"ws://127.0.0.1:7878/session",collaborative:false},{...workspace,visibility:"shared"})).toBe(true);
});

test("original top chrome paints peer orbs and original Windows hover tooltip without opening the hamburger",()=>{
 const {id}=coreCall<{id:number}>("chrome.new",{kind:"topbar",family:"Cascadia Code NF",weight:400});
 const args={id,width:1200,height:800,scale:1,theme:"pastel_dark",connection:"connected",peers:[{peer_id:"bob",display_name:"Bob",client_os:"windows",color:[40,50,60],rainbow:false}]};
 type Frame={commands:Array<{kind:string;text?:string}>;regions:Array<{id:string;label:string;rect:number[]}>;animating:boolean;menuOpen:boolean};
 try{
  const frame=coreCall<Frame>("chrome.frame",args),peer=frame.regions.find(region=>region.id==="presence:bob")!;
  expect(peer.label).toBe("Bob");expect(frame.animating).toBe(true);expect(frame.menuOpen).toBe(false);
  const hovered=coreCall<Frame>("chrome.pointer",{...args,x:peer.rect[0]+peer.rect[2]/2,y:peer.rect[1]+peer.rect[3]/2});
  expect(hovered.commands.some(command=>command.kind==="text"&&command.text==="Bob (Windows)")).toBe(true);
  const cleared=coreCall<Frame>("chrome.frame",{...args,peers:[]});expect(cleared.animating).toBe(false);expect(cleared.regions.some(region=>region.id.startsWith("presence:"))).toBe(false);
 }finally{coreCall("chrome.dispose",{id});}
});
