import {expect,test} from "bun:test";
import {mkdtempSync,rmSync} from "node:fs";
import {tmpdir} from "node:os";
import {join} from "node:path";
import {fileURLToPath} from "node:url";
test("ordinary Host starts the real packaged daemon, accepts two clients and stops only its own recipe",async()=>{
 const directory=mkdtempSync(join(tmpdir(),"neoism-host-test-"));
 const agent=Bun.serve({hostname:"127.0.0.1",port:0,fetch:()=>Response.json({workspaceId:"host-test-chat-namespace",healthy:true,version:"test",providerCredentialStore:"test"})});
 try{
  const child=Bun.spawn([process.execPath,"run","tests/support/server-host-check.ts"],{cwd:fileURLToPath(new URL("..",import.meta.url)),env:{...process.env,NEOISM_CONFIG_HOME:directory,NEOISM_AGENT_SERVER:agent.url.origin,NEOISM_DAEMON_DATA_DIR:join(directory,"auth"),NEOISM_AGENT_STATE_DIR:join(directory,"agent"),XDG_RUNTIME_DIR:directory},stdout:"pipe",stderr:"pipe"});
  const [status,stdout,stderr]=await Promise.all([child.exited,new Response(child.stdout).text(),new Response(child.stderr).text()]);expect({status,stderr}).toEqual({status:0,stderr:""});expect(JSON.parse(stdout)).toMatchObject({isolatedRealHost:true,twoAuthenticatedClients:true,requestedFolder:true,persistedHostRecipe:true,removeStopsOnlyOwnedHost:true,noWindow:true});
 }finally{agent.stop(true);rmSync(directory,{recursive:true,force:true});}
},45000);
