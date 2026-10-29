import{expect,test}from"bun:test";
import{fileURLToPath}from"node:url";
test("actual App connects original chrome/tree peer paint and focused Markdown presence without windows",async()=>{
 const child=Bun.spawn([process.execPath,"run","tests/support/presence-app-check.tsx"],{cwd:fileURLToPath(new URL("..",import.meta.url)),stdout:"pipe",stderr:"pipe"});
 const[status,stdout,stderr]=await Promise.all([child.exited,new Response(child.stdout).text(),new Response(child.stderr).text()]);
 expect({status,stderr}).toEqual({status:0,stderr:""});expect(JSON.parse(stdout)).toEqual({actualApp:true,noWindow:true,topChromeOriginalPeer:true,fileTreeOriginalPeer:true,focusedMarkdownPublishesCursor:true,remoteDepartureClearsChrome:true,noPty:true});
},15000);
