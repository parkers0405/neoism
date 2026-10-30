import {expect,test} from "bun:test";
import {createServer} from "node:net";
import {mkdtempSync,rmSync} from "node:fs";
import {tmpdir} from "node:os";
import {join} from "node:path";
import {loadConfig} from "../src/config";
import {ensureLocalDaemon,reachable} from "../src/daemon/local-startup";
const configuration=()=>loadConfig([],{},{});
test("an explicit remote daemon never probes or starts local services",async()=>{
 for(const [args,env] of [[["--daemon","ws://remote/session"],{}],[[],{NEOISM_GPUIX_DAEMON_URL:"ws://remote/session"}]] as const){
  const config=configuration();const result=await ensureLocalDaemon(config,args,env,{probe:async()=>{throw new Error("local probe");},launch:async()=>{throw new Error("local launch");}});
  expect(result).toBe(config);
 }
});
test("an available local daemon is reused without a restart",async()=>{
 const config=configuration();let starts=0;
 expect(await ensureLocalDaemon(config,[],{},{probe:async()=>true,launch:async()=>{starts++;}})).toBe(config);
 expect(starts).toBe(0);
});
test("startup refreshes the endpoint and newly published upgrade credential",async()=>{
 const config={...configuration(),endpoint:"ws://127.0.0.1:1234/session"};let started=false,now=0;
 const ready={...config,endpoint:"ws+unix:///tmp/fixture.sock:/session",authToken:"new-local-credential"};
 const result=await ensureLocalDaemon(config,[],{},{probe:async endpoint=>started&&endpoint===ready.endpoint,launch:async()=>{started=true;},refresh:()=>ready,sleep:async()=>{now+=100;},now:()=>now,timeoutMs:1000});
 expect(result).toBe(ready);
});
test("a stale Unix socket can fall back to the existing local TCP daemon",async()=>{
 const config={...configuration(),endpoint:"ws+unix:///tmp/stale.sock:/session"};let starts=0;
 const result=await ensureLocalDaemon(config,[],{},{probe:async endpoint=>endpoint==="ws://127.0.0.1:7878/session",refresh:()=>config,launch:async()=>{starts++;}});
 expect(result.endpoint).toBe("ws://127.0.0.1:7878/session");expect(starts).toBe(0);
});
test("failed startup terminates its wait and reports the diagnostic location",async()=>{
 let now=0,starts=0;
 await expect(ensureLocalDaemon(configuration(),[],{},{probe:async()=>false,launch:async()=>{starts++;},sleep:async()=>{now+=100;},now:()=>now,timeoutMs:200})).rejects.toThrow("daemon.log");
 expect(starts).toBe(1);
});
test("the readiness probe checks a real Unix listener and does not trust socket-file existence",async()=>{
 if(process.platform==="win32")return;
 const folder=mkdtempSync(join(tmpdir(),"neoism-startup-")),path=join(folder,"daemon.sock");
 const server=createServer(socket=>socket.end());
 try{
  await new Promise<void>((resolve,reject)=>{server.once("error",reject);server.listen(path,resolve);});
  expect(await reachable(`ws+unix://${path}:/session`)).toBe(true);
  await new Promise<void>(resolve=>server.close(()=>resolve()));
  expect(await reachable(`ws+unix://${path}:/session`)).toBe(false);
 }finally{server.close();rmSync(folder,{recursive:true,force:true});}
});

test("a detached daemon that exits immediately reports startup failure instead of waiting for readiness", async () => {
 if(process.platform === "win32")return;
 const folder=mkdtempSync(join(tmpdir(),"neoism-startup-exit-"));
 const env={...process.env,NEOISM_GPUIX_DAEMON_URL:undefined,NEOISM_DAEMON_SOCKET:join(folder,"missing.sock"),NEOISM_DAEMON_ADDR:"127.0.0.1:0",NEOISM_GPUIX_WORKSPACE_DAEMON:"/bin/false",XDG_STATE_HOME:folder};
 try {
  const config=loadConfig([],env,{});
  await expect(ensureLocalDaemon(config,[],env,{refresh:()=>config,timeoutMs:3000})).rejects.toThrow("exited during startup");
 } finally {rmSync(folder,{recursive:true,force:true});}
});
