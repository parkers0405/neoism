import {expect,test} from "bun:test";
import {fileURLToPath} from "node:url";
import {existsSync} from "node:fs";
const root=fileURLToPath(new URL("..",import.meta.url));
// This exercises a real isolated daemon, without a renderer or a user socket.
test.skipIf(!existsSync(`${root}/artifacts/neoism-workspace-daemon-grid`))("two real daemon clients coedit code/rich Markdown, share cursors, save and rejoin",async()=>{
 const child=Bun.spawn([process.execPath,"run","tests/support/shared-document-check.ts"],{cwd:root,stdout:"pipe",stderr:"pipe"});
 const[status,stdout,stderr]=await Promise.all([child.exited,new Response(child.stdout).text(),new Response(child.stderr).text()]);
 expect({status,stderr}).toEqual({status:0,stderr:""});expect(JSON.parse(stdout)).toEqual({noWindow:true,isolatedRealDaemon:true,twoAuthenticatedClients:true,codeAndRichMarkdownConverge:true,originScopedUndo:true,authoritativeSave:true,utf16Presence:true,originalRemoteCursorPaint:true,offlineReconnectConverges:true,actualModelSplitOwner:true,recreatedModelCausalMerge:true,recoveryActorRetained:true,reconnectSavedClean:true,legacyRecoveryDiffRebased:true});
},30000);
