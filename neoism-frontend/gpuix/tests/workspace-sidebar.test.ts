import {expect,test} from "bun:test";
import {fileURLToPath} from "node:url";
import {initialWorkspaceSidebars,workspaceSidebar,withWorkspaceSidebar} from "../src/ui/workspace-sidebar";
test("legacy sidebar migrates only to its workspace and closed choices stay explicit",()=>{
 expect(initialWorkspaceSidebars({sidebar:"notes"},"a")).toEqual({a:"notes"});
 expect(initialWorkspaceSidebars({sidebar:null},"a")).toEqual({a:null});
 const ui={sidebar:"notes",workspaceSidebars:{a:"notes",b:null}};
 expect(initialWorkspaceSidebars(ui,"c")).toBeUndefined();
 expect(workspaceSidebar(ui,"c")).toBeUndefined();expect(workspaceSidebar(ui,"b")).toBeUndefined();
 expect(withWorkspaceSidebar(ui,"c","conversations")).toEqual({a:"notes",b:null,c:"conversations"});
});
test("fresh windows and unseen workspaces start closed without losing saved panel choices",()=>{
 expect(workspaceSidebar({},"fresh")).toBeUndefined();
 expect(initialWorkspaceSidebars({},"fresh")).toEqual({fresh:null});
 const ui={workspaceSidebars:{a:"files",b:"conversations",c:"notes"}};
 expect(workspaceSidebar(ui,"new")).toBeUndefined();
 expect(workspaceSidebar(ui,"a")).toBe("files");
 expect(workspaceSidebar(ui,"b")).toBe("conversations");
 expect(workspaceSidebar(ui,"c")).toBe("notes");
});
test("actual App releases closed workspace sidebar choices and retains open choices through font generations and cold recovery",async()=>{
 const child=Bun.spawn([process.execPath,"run","tests/support/workspace-sidebar-check.tsx"],{cwd:fileURLToPath(new URL("..",import.meta.url)),env:{...process.env,NODE_ENV:"test"},stdout:"pipe",stderr:"pipe"});
 const[status,stdout,stderr]=await Promise.all([child.exited,new Response(child.stdout).text(),new Response(child.stderr).text()]);
   expect({status,stderr}).toEqual({status:0,stderr:""});expect(JSON.parse(stdout)).toEqual({actualApp:true,noWindow:true,workspacePlusStartsClosed:true,independentFiveStates:true,closedWorkspaceReleased:true,fontGenerationRetained:true,coldRecoveryRetained:true,databaseSidebarIntegrated:true,noPtyOrPromptDuringSwitches:true,localWelcomeNoReplacement:true,emptyWelcomeAction:true});
},15000);
