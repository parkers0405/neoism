import {expect,test} from "bun:test";
import {fileURLToPath} from "node:url";

test("actual code and rich Markdown carets blink after cached native holds expire, with original opaque ink",async()=>{
 const child=Bun.spawn([process.execPath,"run","tests/support/editor-blink-check.tsx"],{cwd:fileURLToPath(new URL("..",import.meta.url)),stdout:"pipe",stderr:"pipe"});
 const[status,stdout,stderr]=await Promise.all([child.exited,new Response(child.stdout).text(),new Response(child.stderr).text()]);
 expect({status,stderr,stdout:status===0?undefined:stdout}).toEqual({status:0,stderr:"",stdout:undefined});
 expect(JSON.parse(stdout)).toEqual({noWindow:true,actualCodeAndMarkdown:true,opaqueCursorInk:true,cachedHoldExpires:true,idleBlinkWithoutNewDocumentFrame:true,motionRestoresVisibility:true});
},12000);
