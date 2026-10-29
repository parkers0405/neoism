import{expect,test}from"bun:test";
import{fileURLToPath}from"node:url";
test("actual App /connect follows original grouped account flow and native label/secret entry",async()=>{
 const child=Bun.spawn([process.execPath,"run","tests/support/connect-app-check.tsx"],{cwd:fileURLToPath(new URL("..",import.meta.url)),stdout:"pipe",stderr:"pipe"});
 const[status,stdout,stderr]=await Promise.all([child.exited,new Response(child.stdout).text(),new Response(child.stderr).text()]);expect({status,stderr}).toEqual({status:0,stderr:""});expect(JSON.parse(stdout)).toEqual({actualApp:true,noWindow:true,originalGroupedProviders:true,workspaceAccountIsolation:true,addAccountStage:true,nativeLabelEntry:true,escapeReturnsToAccounts:true,nativeMaskedSecretEntry:true,exactScopedCreate:true,doneStage:true,otherDraftPreserved:true,noSessionOrPtyCreation:true});
},15000);
