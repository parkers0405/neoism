import assert from 'node:assert/strict';
import {loadConfig} from '../../src/config';
import {DaemonConnection} from '../../src/daemon/client';
import {WorkspaceModel} from '../../src/model';
import {scrollAgentHalfPage} from '../../src/agent/AgentTimelineView';
import {coreCall} from '../../src/native-core';
const config=loadConfig(['--config','/nonexistent/timeline-retention'],{}),client=new DaemonConnection(config),model=new WorkspaceModel(client,config);
const stats=()=>coreCall<Array<{sessionId:string}>>('agenttimeline.diagnostics');
try{
 // Half-page commands target the active workspace; engines and their owning
 // surfaces must share that workspace, just as they do in a mounted chat.
 const workspace={id:'retention',host_id:'host',title:'Retention',root_dir:'/project/retention'};
 model.update({activeWorkspace:workspace,workspaces:[workspace],surfaces:Array.from({length:12},(_,i)=>({id:`agent:retention-${i}`,kind:'agent' as const,title:`Chat ${i}`,workspaceId:workspace.id,sessionId:`retention-${i}`}))});
 for(let i=0;i<12;i++){scrollAgentHalfPage(model,`retention-${i}`,config,true);model.update({error:String(i)});}
 assert.equal(stats().length,8);assert.ok(!stats().some(s=>s.sessionId==='retention-0'));
 model.update({surfaces:model.state.surfaces.filter(s=>s.sessionId!=='retention-11')});assert.equal(stats().length,7);
 model.dispose();assert.equal(stats().length,0);console.log('Timeline retention evicts dormant engines, closes native state, and cleans up on model disposal.');
}finally{model.dispose();client.stop();}
