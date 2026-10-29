import {expect,test} from "bun:test";
test("cold recovered Agent tabs hydrate and Tab Build/Plan uses original session routing",async()=>{
 const child=Bun.spawn([process.execPath,"run","tests/support/agent-recovery-mode-check.tsx"],{stdout:"pipe",stderr:"pipe"});const[status,stdout,stderr]=await Promise.all([child.exited,new Response(child.stdout).text(),new Response(child.stderr).text()]);
 expect({status,stderr}).toEqual({status:0,stderr:""});expect(JSON.parse(stdout)).toMatchObject({noWindow:true,actualApp:true,coldAgentTabsHydrateWithoutScrolling:true,pendingWorkspaceAckDoesNotBlockTranscript:true,originalTimelinePaints:true,buildPlanUsesCanonicalAgentCommand:true,emptyHomeRemainsLocal:true,dirtyFileAndDraftRetained:true,noPtyOrSessionCreation:true});
},20000);
