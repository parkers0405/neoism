import {expect,test} from "bun:test";
import {fileURLToPath} from "node:url";
import {workspaceTabRoot,workspaceLabel} from "../src/layout/tab-chrome";
const local={endpoint:"ws://127.0.0.1:7878/session"};
test("workspace label follows only a valid confirmed local first-terminal root and explicit/shared roots stay authoritative",()=>{
 const workspace={id:"a",host_id:"host",title:"stale",root_dir:"/home/me"},roots={a:{declaredRoot:"/home/me",root:"/home/me/projects/neoism"}};
 expect(workspaceLabel(workspace.title,workspaceTabRoot(workspace,local,roots),"/home/me")).toBe("neoism");
 expect(workspaceTabRoot({...workspace,root_dir:"/manual"},local,roots)).toBe("/manual");
 expect(workspaceTabRoot({...workspace,host_kind:"tailscale"},local,roots)).toBe("/home/me");
 expect(workspaceTabRoot(workspace,{...local,serverId:"joined"},roots)).toBe("/home/me");
 expect(workspaceTabRoot(workspace,{...local,transportIdentity:"ssh:other"},roots)).toBe("/home/me");
 for(const value of ["relative/path","",null,"/path\0invalid"])expect(workspaceTabRoot(workspace,local,{a:{declaredRoot:"/home/me",root:value}})).toBe("/home/me");
 expect(workspaceTabRoot(workspace,local,{b:roots.a})).toBe("/home/me");
});
test("actual App workspace strip follows confirmed terminal cwd in place and retains its own root across switching",async()=>{
 const child=Bun.spawn([process.execPath,"run","tests/support/workspace-cwd-label-check.tsx"],{cwd:fileURLToPath(new URL("..",import.meta.url)),stdout:"pipe",stderr:"pipe"});
 const [status,out,err]=await Promise.all([child.exited,new Response(child.stdout).text(),new Response(child.stderr).text()]);expect({status,err}).toEqual({status:0,err:""});
 expect(JSON.parse(out)).toEqual({actualApp:true,noWindow:true,confirmedTerminalCwdLabelsExistingTab:true,inactiveWorkspaceRetained:true,manualRootWins:true,noSharedRootMutation:true});
},15000);
