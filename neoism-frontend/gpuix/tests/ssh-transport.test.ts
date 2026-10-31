import {fileURLToPath} from "node:url";
import {expect,test} from "bun:test";
import {mkdtempSync,rmSync,readFileSync,existsSync} from "node:fs";
import {join,delimiter} from "node:path";
import {tmpdir} from "node:os";
import {attachSsh,type SshNativeCall} from "../src/daemon/ssh-transport";
const ready={phase:"ready",endpoint:"ws://127.0.0.1:45678/session",authToken:"test-only-credential",workspaceId:"neoism-quick-ssh-home-0123456789abcdef",target:"dev@fixture.invalid"};
function native(frames:unknown[]){const calls:Array<{op:string;args?:Record<string,unknown>}>=[];const call=((op:string,args?:Record<string,unknown>)=>{calls.push({op,args});if(op==="ssh.start")return{id:7};if(op==="ssh.poll")return frames.length>1?frames.shift():frames[0];return{closed:true};})as SshNativeCall;return{call,calls};}
test("SSH attachment retains transport identity and closes only its exact tunnel once",async()=>{
 const f=native([{phase:"pending"},ready]);const attachment=await attachSsh({systemId:"saved-system"},{call:f.call,pollMs:1});
 expect(attachment.endpoint).toBe(ready.endpoint);expect(attachment.authToken).toBe(ready.authToken);expect(attachment.identity).toBe(`ssh:${ready.workspaceId}`);expect(attachment.alive()).toBe(true);
 attachment.dispose();attachment.dispose();expect(f.calls.filter(item=>item.op==="ssh.close")).toEqual([{op:"ssh.close",args:{id:7}}]);expect(attachment.alive()).toBe(false);
 expect(f.calls[0]).toEqual({op:"ssh.start",args:{systemId:"saved-system",args:undefined}});
});
test("SSH failure and cancellation never return a local fallback and retire pending native jobs",async()=>{
 const failed=native([{phase:"failed",message:"fixture SSH authentication rejected"}]);await expect(attachSsh({target:"missing"},{call:failed.call})).rejects.toThrow("authentication rejected");expect(failed.calls.at(-1)?.op).toBe("ssh.close");
 const cancelled=native([{phase:"pending"}]),abort=new AbortController();const promise=attachSsh({target:"host"},{call:cancelled.call,signal:abort.signal,pollMs:1000});abort.abort(new Error("Stopped by user"));await expect(promise).rejects.toThrow("Stopped by user");expect(cancelled.calls.at(-1)?.op).toBe("ssh.close");
 const malformed=native([{...ready,endpoint:"ws://localhost:7878/session"}]);await expect(attachSsh({target:"host"},{call:malformed.call})).rejects.toThrow("invalid daemon attachment");expect(malformed.calls.at(-1)?.op).toBe("ssh.close");
});
test("actual original SSH transport and Ghostty route terminal I/O exclusively to the selected remote daemon",async()=>{
 const directory=mkdtempSync(join(tmpdir(),"neoism-ssh-e2e-"));
 try{
  // Windows executable lookup checks the working directory before PATH.
  // Keep the fake ssh.exe there and avoid duplicate Path/PATH environment keys.
  const inherited=Object.fromEntries(Object.entries(process.env).filter(([key])=>key.toUpperCase()!=="PATH"));
  const searchPath=Object.entries(process.env).find(([key])=>key.toUpperCase()==="PATH")?.[1]||"";
  const child=Bun.spawn([process.execPath,"run",fileURLToPath(new URL("./support/ssh-terminal-check.ts",import.meta.url))],{cwd:directory,env:{...inherited,NEOISM_SSH_FIXTURE_DIR:directory,NEOISM_CONFIG_HOME:directory,PATH:directory+delimiter+searchPath},stdout:"pipe",stderr:"pipe"});
  const [status,stdout,stderr]=await Promise.all([child.exited,new Response(child.stdout).text(),new Response(child.stderr).text()]);
  expect({status,stderr}).toEqual({status:0,stderr:""});expect(JSON.parse(stdout)).toMatchObject({noWindow:true,originalSshTransport:true,remoteHomeWorkspace:true,ghosttyTerminal:true,remoteCreateAttachInputResizeDirectory:true,noLocalPtyOperations:true,reconnectRetainsRemoteRoute:true,tunnelDisposed:true,savedSystemRegistry:true,shutdownReapsCancelledTunnel:true,selectedTunnelReconnectResumesPty:true});
  expect(existsSync(join(directory,"systems.json"))).toBe(true);expect(JSON.parse(readFileSync(join(directory,"systems.json"),"utf8")).systems).toEqual([]);
 }finally{rmSync(directory,{recursive:true,force:true});}
},20000);
