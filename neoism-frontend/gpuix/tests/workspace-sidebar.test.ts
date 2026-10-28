import {expect,test} from "bun:test";
import {fileURLToPath} from "node:url";
import {initialWorkspaceSidebars,workspaceSidebar,withWorkspaceSidebar} from "../src/ui/workspace-sidebar";
test("legacy sidebar migrates only to its workspace and closed choices stay explicit",()=>{
 expect(initialWorkspaceSidebars({sidebar:"notes"},"a")).toEqual({a:"notes"});
 expect(initialWorkspaceSidebars({sidebar:null},"a")).toEqual({a:null});
 const ui={sidebar:"notes",workspaceSidebars:{a:"notes",b:null}};
 expect(initialWorkspaceSidebars(ui,"c")).toBeUndefined();
 expect(workspaceSidebar(ui,"c")).toBe("files");expect(workspaceSidebar(ui,"b")).toBeUndefined();
 expect(withWorkspaceSidebar(ui,"c","conversations")).toEqual({a:"notes",b:null,c:"conversations"});
});
test("actual App keeps all workspace sidebar choices through closing, font generations and cold recovery",async()=>{
 const child=Bun.spawn([process.execPath,"run","tests/support/workspace-sidebar-check.tsx"],{cwd:fileURLToPath(new URL("..",import.meta.url)),env:{...process.env,NODE_ENV:"test"},stdout:"pipe",stderr:"pipe"});
 const[status,stdout,stderr]=await Promise.all([child.exited,new Response(child.stdout).text(),new Response(child.stderr).text()]);
 expect({status,stderr}).toEqual({status:0,stderr:""});expect(JSON.parse(stdout)).toEqual({actualApp:true,noWindow:true,workspacePlusNoEmptyFrame:true,independentFourStates:true,closedWorkspaceRetained:true,fontGenerationRetained:true,coldRecoveryRetained:true,noPtyOrPrompt:true});
},15000);
