import {expect,test} from "bun:test";
import {fileURLToPath} from "node:url";
test("original Git panel paint/input uses scoped daemon requests inside a stable reserved slot",async()=>{
 const child=Bun.spawn([process.execPath,"run","tests/support/git-panel-view-check.tsx"],{cwd:fileURLToPath(new URL("..",import.meta.url)),env:{...process.env,NODE_ENV:"test"},stdout:"pipe",stderr:"pipe"});
 const[status,stdout,stderr]=await Promise.all([child.exited,new Response(child.stdout).text(),new Response(child.stderr).text()]);
 expect({status,stderr}).toEqual({status:0,stderr:""});expect(JSON.parse(stdout)).toEqual({noWindow:true,originalRustGitPaint:true,sourceCoordinateTranslation:true,actualScopedFixtureRequests:true,nativeFileReviewAndStage:true,commitMessagePreservedOnFailure:true,branchCheckoutAndResize:true,exactGlobalCursorTarget:true,stablePaintAndFocusHosts:true,backgroundRefreshDoesNotRefocus:true,closeWorks:true});
},12000);
