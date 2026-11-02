import {test,expect} from 'bun:test';
import {fileURLToPath} from 'node:url';
test('timeline engines are bounded and disposed with closed chats and their model',async()=>{
 const child=Bun.spawn([process.execPath,'run','tests/support/timeline-retention-check.ts'],{cwd:fileURLToPath(new URL('..',import.meta.url)),stdout:'pipe',stderr:'pipe'});
 const [status,stderr]=await Promise.all([child.exited,new Response(child.stderr).text(),new Response(child.stdout).text()]);expect({status,stderr}).toEqual({status:0,stderr:''});
});
