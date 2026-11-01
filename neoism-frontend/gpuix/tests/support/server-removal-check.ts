import assert from "node:assert/strict";
import {mkdtempSync,rmSync} from "node:fs";
import {tmpdir} from "node:os";
import {join} from "node:path";
import {coreCall} from "../../src/native-core";
import {loadConfig} from "../../src/config";
import {DaemonConnection} from "../../src/daemon/client";
import {WorkspaceModel} from "../../src/model";
import {ChromeActionsController} from "../../src/menu-actions/ChromeActionsController";
import {serverJob} from "../../src/menu-actions/server-actions";

const directory=mkdtempSync(join(tmpdir(),"neoism-removal-"));
process.env.NEOISM_CONFIG_HOME=directory;
const config=loadConfig(["--config",join(directory,"missing.json")],{});
const client=new DaemonConnection(config),model=new WorkspaceModel(client,config);
const controller=new ChromeActionsController({model,config,reload:()=>{}});
const createConfirm=(id:string,system:boolean)=>(controller as unknown as {create:(kind:string,payload:Record<string,unknown>)=>void}).create("server-confirm",{serverId:id,system,title:system?"Remove system?":"Remove server?",body:"Remove this saved connection?"});
const until=async(check:()=>boolean)=>{const deadline=Date.now()+4000;while(!check()){if(Date.now()>deadline)throw new Error("Confirmed removal did not finish");await Bun.sleep(20);}};
try{
 const system=coreCall<{system:{id:string}}>("ssh.systems.add",{target:"fixture.local",name:"Fixture system"}).system;
 assert.ok(coreCall<{systems:Array<{id:string}>}>("ssh.systems.list").systems.some(entry=>entry.id===system.id));
 createConfirm(system.id,true);controller.call("key",{key:"enter"});
 await until(()=>!coreCall<{systems:Array<{id:string}>}>("ssh.systems.list").systems.some(entry=>entry.id===system.id));
 assert.equal(controller.frame?.kind,"systems");

 const server=await serverJob("save",{address:"ws://127.0.0.1:12345/session",name:"Fixture server"});
 assert.ok(coreCall<{entries:Array<{id:string}>}>("server.list").entries.some(entry=>entry.id===server.id));
 createConfirm(server.id,false);controller.call("key",{key:"enter"});
 await until(()=>!coreCall<{entries:Array<{id:string}>}>("server.list").entries.some(entry=>entry.id===server.id)&&controller.frame?.kind==="servers");
 assert.equal(controller.frame?.kind,"servers");
 console.log(JSON.stringify({systemRemoved:true,serverRemoved:true,listsRefreshed:true}));
}finally{controller.dispose();model.dispose();client.stop();rmSync(directory,{recursive:true,force:true});}
