import assert from "node:assert/strict";
import {mkdtempSync,rmSync} from "node:fs";
import {join} from "node:path";
import {DaemonConnection} from "../../src/daemon/client";
import {loadConfig} from "../../src/config";
import {Effect} from "effect";
import {expectVariant,isRecord} from "../../src/daemon/protocol";
import {coreCall} from "../../src/native-core";
import {serverJob} from "../../src/menu-actions/server-actions";
const configDir=process.env.NEOISM_CONFIG_HOME!;assert.ok(configDir?.includes("neoism-host-test-"),"An isolated startup registry is mandatory");
const directory=mkdtempSync(join(configDir,"project-"));let id:string|undefined;const clients:DaemonConnection[]=[];
const until=async(check:()=>boolean,message:string)=>{const end=Date.now()+12000;while(!check()){if(Date.now()>end)throw new Error(message);await Bun.sleep(10);}};
try{
 const server=await serverJob("host",{directory,name:"Isolated real host",password:"host-test-password"});id=server.id;
 const config={...loadConfig(["--config","/nonexistent/host-test"],{}),endpoint:server.endpoint,authToken:server.authToken,pairingToken:server.pairingToken};
 const a=new DaemonConnection(config,undefined,()=>{}),b=new DaemonConnection(config,undefined,()=>{});clients.push(a,b);a.start();b.start();
 await until(()=>a.state.phase==="connected"&&b.state.phase==="connected","Both clients must authenticate to the hosted daemon");
 const tree=expectVariant(await Effect.runPromise(a.request("Workspace","RequestHostWorkspaceTree")),"HostWorkspaceTree");assert.ok(isRecord(tree)&&Array.isArray(tree.workspaces));assert.ok(tree.workspaces.some((workspace:any)=>workspace.root_dir===directory),"Hosted daemon declares the requested directory");
 const listed=coreCall<{entries:Array<{id:string;hosted:boolean}>}>("server.list");assert.ok(listed.entries.some(entry=>entry.id===id&&entry.hosted));
 await serverJob("remove",{serverId:id});id=undefined;
 await until(()=>a.state.phase!=="connected"&&b.state.phase!=="connected","Removing the exact hosted recipe stops its daemon and disconnects both clients");
 console.log(JSON.stringify({noWindow:true,isolatedRealHost:true,twoAuthenticatedClients:true,requestedFolder:true,persistedHostRecipe:true,removeStopsOnlyOwnedHost:true}));
}finally{for(const client of clients)client.stop();if(id)await serverJob("remove",{serverId:id});coreCall("server.shutdown");rmSync(directory,{recursive:true,force:true});}
