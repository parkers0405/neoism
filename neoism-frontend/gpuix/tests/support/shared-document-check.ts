import {canonicalPath} from "./canonical-path";
import assert from "node:assert/strict";
import {mkdtempSync,mkdirSync,writeFileSync,readFileSync,rmSync} from "node:fs";
import {tmpdir} from "node:os";
import {join,resolve} from "node:path";
import {Effect} from "effect";
import {daemonExecutable} from "../../src/daemon/local-startup";
import {loadConfig} from "../../src/config";
import {DaemonConnection} from "../../src/daemon/client";
import {expectVariant,type Workspace} from "../../src/daemon/protocol";
import {NativeEditor} from "../../src/native-core";
import {NativeMarkdown} from "../../src/markdown/NativeMarkdown";
import {PresenceController} from "../../src/collaboration/PresenceController";
import {WorkspaceModel,type Surface} from "../../src/model";
import {DocumentController} from "../../src/collaboration/DocumentController";
const temp=canonicalPath(mkdtempSync(join(tmpdir(),"neoism-shared-doc-"))),directory=join(temp,"project"),runtime=join(temp,"runtime");mkdirSync(directory);mkdirSync(runtime,{mode:0o700});
const path=join(directory,"shared.md"),initial="hello α\n";writeFileSync(path,initial);
const token=crypto.randomUUID(),nonce=crypto.randomUUID();let address="",output="";
const agent=Bun.serve({hostname:"127.0.0.1",port:0,fetch:()=>Response.json({workspaceId:"fixture-chat-namespace"})});
const daemon=Bun.spawn([daemonExecutable(),"--addr","127.0.0.1:0","--ephemeral","--no-unix-socket","--state-dir",join(temp,"state"),"--workspace",directory],{env:{...process.env,NEOISM_DAEMON_TOKEN:token,NEOISM_AGENT_SERVER:agent.url.origin,NEOISM_FAST_READINESS_NONCE:nonce,NEOISM_DAEMON_DATA_DIR:join(temp,"auth"),NEOISM_CONFIG_HOME:join(temp,"config"),XDG_CONFIG_HOME:join(temp,"config"),XDG_RUNTIME_DIR:runtime,XDG_DATA_HOME:join(temp,"data"),XDG_CACHE_HOME:join(temp,"cache"),NEOISM_REQUIRE_AUTH:"0"},stdout:"pipe",stderr:"pipe"});
const stdout=(async()=>{let pending="";const reader=daemon.stdout.getReader();while(true){const {done,value}=await reader.read();if(done)break;const text=new TextDecoder().decode(value);output+=text;pending+=text;let newline;while((newline=pending.indexOf("\n"))>=0){const line=pending.slice(0,newline);pending=pending.slice(newline+1);try{const ack=JSON.parse(line);if(ack.nonce===nonce&&typeof ack.address==="string")address=ack.address;}catch{}}}})();
const stderr=(async()=>{const reader=daemon.stderr.getReader();while(true){const {done,value}=await reader.read();if(done)break;output+=new TextDecoder().decode(value);}})();
const failures:unknown[]=[],editors:NativeEditor[]=[],controllers:DocumentController[]=[],presences:PresenceController[]=[],clients:DaemonConnection[]=[],models:WorkspaceModel[]=[];let markdown:NativeMarkdown|undefined;
const until=async(predicate:()=>boolean,reason:string)=>{const deadline=Date.now()+8000;while(!predicate()){if(Date.now()>deadline)throw new Error(`${reason}\n${output.slice(-2500)}`);await Bun.sleep(5);}};
try{
 await until(()=>!!address,"Isolated daemon readiness");
 const config={...loadConfig(["--config","/nonexistent/shared-doc"],{}),endpoint:`ws://${address}/session`,authToken:token,pairingToken:undefined,collaborative:true,reducedMotion:true,cursorBlink:false,vim:false};
 const make=(name:string,color:[number,number,number])=>{
  const client=new DaemonConnection(config,undefined,()=>{});clients.push(client);const presence=new PresenceController(client,config,{peerId:`fixture:${name}`,displayName:name,color});presences.push(presence);
  const document=new DocumentController(client,config,presence,error=>failures.push(error));controllers.push(document);client.start();return{client,presence,document};
 };
 const a=make("Alice",[230,110,130]),b=make("Bob",[110,180,230]);await until(()=>clients.every(client=>client.state.phase==="connected"),"Both real WebSocket clients authenticate: "+JSON.stringify(clients.map(client=>client.state)));
 const tree=expectVariant(await Effect.runPromise(a.client.request("Workspace","RequestHostWorkspaceTree")),"HostWorkspaceTree") as {hosts:Array<{id:string}>;workspaces:Workspace[]};assert.ok(tree.hosts.length);
 const declared=tree.workspaces.find(workspace=>workspace.root_dir&&canonicalPath(workspace.root_dir)===canonicalPath(directory));assert.ok(declared);const id=declared.id,workspace={...declared,visibility:"shared"};
 await Promise.all(clients.map(client=>Effect.runPromise(client.request("Workspace",{SubscribeWorkspace:{workspace_id:id}}))));a.presence.setWorkspace(workspace);b.presence.setWorkspace(workspace);
 const ea=new NativeEditor(path,initial,false),eb=new NativeEditor(path,initial,false);editors.push(ea,eb);ea.call("layout",{width:800,height:500,cellWidth:10,rowHeight:21});eb.call("layout",{width:800,height:500,cellWidth:10,rowHeight:21});
 a.document.attach(ea,path,workspace);b.document.attach(eb,path,workspace);markdown=new NativeMarkdown(eb,config,path);markdown.call("layout",{width:800,height:500});await until(()=>a.document.isBound(ea)&&b.document.isBound(eb),"Both documents seed from the daemon Yrs snapshot");
 assert.equal(ea.text(),initial);assert.equal(eb.text(),initial);
 // Both native surfaces edit before either waits for the other socket.
 ea.call("goto",{line:0,col:0});ea.call("paste",{text:"left "});markdown.call("goto",{line:0,col:8});markdown.call("key",{key:"!",char:"!"});
 await until(()=>ea.text()===eb.text()&&ea.text().startsWith("left ")&&ea.text().includes("!"),"Concurrent code/Markdown edits converge");assert.equal(markdown.text(),ea.text());
 markdown.call("key",{key:"z",cmd:true});await until(()=>ea.text()===eb.text()&&!ea.text().includes("!"),"Markdown undo removes its own CRDT edit");assert.ok(ea.text().startsWith("left "),"Undo preserves the other client's insertion");
 await a.document.save(ea);await until(()=>!ea.frame.dirty&&!eb.frame.dirty,"Daemon save clears both replicas");assert.equal(readFileSync(path,"utf8"),ea.text());
 ea.call("goto",{line:0,col:Buffer.byteLength(ea.text().split("\n")[0])});const caret=ea.getPresence()!;assert.ok(caret.cursor.column<ea.frame.cursor.col,"Greek bytes convert to UTF-16 wire columns");a.presence.updateLocal(a.document.bufferId(ea)!,caret.cursor,caret.selection,caret.insert);
 await until(()=>b.presence.filePeers(b.document.bufferId(eb)!).some(peer=>peer.display_name==="Alice"),"Remote peer arrives on the file presence plane");await until(()=>eb.frame.peerCommands!.some(command=>command.kind==="text"&&command.text==="Alice"),"Code host paints original remote name flag");assert.ok(markdown.frame.commands.some(command=>command.kind==="text"&&command.text==="Alice"),"Rich Markdown paints original remote name flag");
 b.client.stop();await until(()=>b.client.state.phase==="closed","Second peer disconnects");ea.call("goto",{line:0,col:0});ea.call("paste",{text:"online "});eb.call("goto",{line:0,col:0});eb.call("paste",{text:"offline "});b.client.start();
 await until(()=>b.client.state.phase==="connected"&&ea.text()===eb.text()&&ea.text().includes("online ")&&ea.text().includes("offline "),"Reconnect merges offline edits with concurrent server edits");
 await b.document.save(eb);await until(()=>!ea.frame.dirty&&!eb.frame.dirty,"Offline raw transactions are acknowledged and both replicas become clean");assert.equal(readFileSync(path,"utf8"),eb.text());assert.deepEqual(failures,[]);
 // Exercise production WorkspaceModel ownership, rather than wiring split
 // views by hand: a clone edits through one canonical document/undo owner.
 const splitPath=join(directory,"split.md"),splitInitial="body α";writeFileSync(splitPath,splitInitial);
 const modelFor=(client:DaemonConnection,label:string)=>{
  const model=new WorkspaceModel(client,config);models.push(model);
  const surface:Surface={id:`file:${label}`,kind:"file",title:"split.md",path:"split.md",rootDir:directory,workspaceId:id};
  model.update({workspaces:[workspace],activeWorkspace:workspace,surfaces:[surface],activeSurfaceId:surface.id,files:{[surface.id]:splitInitial},loading:false});
  return{model,surface,editor:model.getEditor(surface,11)};
 };
 const ma=modelFor(a.client,"split-a"),mb=modelFor(b.client,"split-b"),rich=mb.model.getMarkdown(mb.surface,11);
 await until(()=>ma.editor.frame.collaborationBound===true&&mb.editor.frame.collaborationBound===true,"Production split clones receive document binding");
 ma.editor.call("goto",{line:0,col:0});ma.editor.call("paste",{text:"owner-a "});rich.call("goto",{line:0,col:7});rich.call("key",{key:"?",char:"?"});
 await until(()=>ma.editor.text()===mb.editor.text()&&rich.text()===ma.editor.text()&&rich.text().includes("owner-a ")&&rich.text().includes("?"),"Production split owners converge with rich Markdown");
 rich.call("key",{key:"z",cmd:true});await until(()=>ma.editor.text()===mb.editor.text()&&!rich.text().includes("?"),"Split Markdown undo reaches the canonical CRDT owner");assert.ok(rich.text().includes("owner-a "));
 await ma.model.saveFile(ma.surface);assert.equal(readFileSync(splitPath,"utf8"),rich.text());
 // SSH transport reconstruction discards the old model/native editors. Dirty
 // source must resume the same Yrs actor, rather than replace today's server
 // document with yesterday's recovered whole-file text.
 b.client.stop();mb.editor.call("goto",{line:0,col:0});mb.editor.call("paste",{text:"recovered-local "});
 const recovery=JSON.parse(JSON.stringify(mb.model.captureSession()));
 const actor=(recovery.editors[mb.surface.id].collaboration as {clientId:number}).clientId;assert.ok(actor>0);
 mb.model.dispose();ma.editor.call("goto",{line:0,col:0});ma.editor.call("paste",{text:"peer-kept "});
 await Effect.runPromise(a.client.request("Crdt",{RequestSnapshot:{buffer_id:ma.model.collaboration.bufferId(ma.editor)!}}));
 const restored=new WorkspaceModel(b.client,config);models.push(restored);restored.restoreSession(recovery);restored.update({workspaces:[workspace],activeWorkspace:workspace,activeSurfaceId:mb.surface.id});
 const resumed=restored.getEditor(mb.surface,11),resumedRich=restored.getMarkdown(mb.surface,11);
 assert.equal((restored.getEditor(mb.surface).snapshot().collaboration as {clientId:number}).clientId,actor,"Recovery keeps the original document actor");
 b.client.start();await until(()=>b.client.state.phase==="connected"&&resumed.text()===ma.editor.text()&&resumed.text().includes("peer-kept ")&&resumed.text().includes("recovered-local "),"A recreated model merges offline recovery with intervening collaborator edits");assert.equal(resumedRich.text(),ma.editor.text());
 resumed.call("goto",{line:0,col:0});resumed.call("paste",{text:"after-recovery "});await until(()=>ma.editor.text()===resumed.text(),"Restored actor produces new causal transactions without id collision");
 resumed.call("key",{key:"z",ctrl:true});await until(()=>ma.editor.text()===resumed.text()&&!resumed.text().includes("after-recovery "),"New undo after recovery preserves both earlier collaborators' work");assert.ok(resumed.text().includes("peer-kept ")&&resumed.text().includes("recovered-local "));
 await restored.saveFile(mb.surface);await until(()=>!ma.editor.frame.dirty&&!resumed.frame.dirty,"Recreated model save acknowledges replayed pending updates and clears both dirty flags");assert.equal(readFileSync(splitPath,"utf8"),resumed.text());
 // Old recovery files contain no Yrs metadata. Their saved baseline still
 // lets the first join rebase only the pending local edit instead of replacing
 // the server's concurrent source with a stale whole-file snapshot.
 b.client.stop();resumed.call("goto",{line:0,col:0});resumed.call("paste",{text:"legacy-local "});resumed.call("replace-source",{text:resumed.text().replace("body α","")});
 const legacy=JSON.parse(JSON.stringify(restored.captureSession()));for(const snapshot of Object.values(legacy.editors) as Array<Record<string,unknown>>)delete snapshot.collaboration;
 restored.dispose();ma.editor.call("goto",{line:0,col:ma.editor.text().indexOf("body")+2});ma.editor.call("paste",{text:"legacy-peer "});await Effect.runPromise(a.client.request("Crdt",{RequestSnapshot:{buffer_id:ma.model.collaboration.bufferId(ma.editor)!}}));
 const legacyModel=new WorkspaceModel(b.client,config);models.push(legacyModel);legacyModel.restoreSession(legacy);legacyModel.update({workspaces:[workspace],activeWorkspace:workspace,activeSurfaceId:mb.surface.id});const legacyEditor=legacyModel.getEditor(mb.surface,11);
 b.client.start();await until(()=>legacyModel.collaboration.isBound(legacyEditor)&&legacyEditor.text()===ma.editor.text()&&legacyEditor.text().includes("legacy-local ")&&legacyEditor.text().includes("legacy-peer "),"Legacy dirty source rebases its local delta instead of overwriting the live peer");assert.ok(!legacyEditor.text().includes("body α"),"Local deletion survives while the concurrent insertion inside that range remains");
 await legacyModel.saveFile(mb.surface);await until(()=>!ma.editor.frame.dirty&&!legacyEditor.frame.dirty,"Legacy recovery save leaves both replicas clean");assert.equal(readFileSync(splitPath,"utf8"),legacyEditor.text());
 ma.model.closeSurface(ma.surface.id,true);legacyModel.closeSurface(mb.surface.id,true);
 a.presence.updateLocal(a.document.bufferId(ea)!,ea.getPresence()!.cursor,null,false);await Bun.sleep(30);assert.deepEqual(failures,[]);

 console.log(JSON.stringify({noWindow:true,isolatedRealDaemon:true,twoAuthenticatedClients:true,codeAndRichMarkdownConverge:true,originScopedUndo:true,authoritativeSave:true,utf16Presence:true,originalRemoteCursorPaint:true,offlineReconnectConverges:true,actualModelSplitOwner:true,recreatedModelCausalMerge:true,recoveryActorRetained:true,reconnectSavedClean:true,legacyRecoveryDiffRebased:true}));
}finally{for(const model of models)model.dispose();markdown?.dispose();for(const document of controllers)document.dispose();for(const presence of presences)presence.dispose();for(const editor of editors)editor.dispose();for(const client of clients)client.stop();daemon.kill("SIGTERM");await daemon.exited;await Promise.all([stdout,stderr]);agent.stop(true);rmSync(temp,{recursive:true,force:true});}
