// Verify activation before release in the production window, with disposable workspaces.
import assert from 'node:assert/strict';
import {mkdtempSync,readFileSync,writeFileSync} from 'node:fs';
import {join} from 'node:path';
import * as Effect from 'effect/Effect';
import {loadConfig} from '../src/config';
import {DaemonConnection} from '../src/daemon/client';
import {launchNative} from '../tests/support/native-launch';
const directory=mkdtempSync('/tmp/neoism-workspace-press-'),state=join(directory,'session.json');
const client=new DaemonConnection(loadConfig([],process.env)),ids=[crypto.randomUUID(),crypto.randomUUID()].map(id=>'neoism-press-'+id);
let app:Awaited<ReturnType<typeof launchNative>>|undefined;
try{
 client.start();const deadline=Date.now()+10000;while(client.state.phase!=='connected'){if(Date.now()>deadline)throw Error('Daemon unavailable');await Bun.sleep(50);}
 for(const [index,id]of ids.entries())await Effect.runPromise(client.request('Workspace',{CreateWorkspace:{workspace_id:id,title:`Press diagnostic ${index}`,root_dir:directory}}));
 writeFileSync(state,JSON.stringify({version:1,activeWorkspaceId:ids[0],surfaces:[],editors:{},providerStates:{},drafts:{},composerDrafts:{},ui:{openWorkspaceIds:ids}}));
 app=await launchNative({command:process.env.LIVE_BINARY||join(process.cwd(),'dist/neoism'),args:[],cwd:process.cwd(),env:{...process.env,NEOISM_GPUIX_STATE_PATH:state}});
 await Bun.sleep(4000);const{pid}=await app.call('initialize',{protocolVersion:1,client:'workspace-press'});
 if(process.env.HYPRLAND_INSTANCE_SIGNATURE)await Bun.$`hyprctl dispatch ${`hl.dsp.focus({window="pid:${pid}"})`}`.quiet();
 const tab=app.getByTestId(`workspace:${JSON.stringify(['local',ids[1]])}`);await tab.waitFor({timeoutMs:10000});
 const bounds=await tab.bounds(),x=bounds.x+Math.min(30,bounds.width/3),y=bounds.y+bounds.height/2;
 await app.call('mouseDown',{x,y,button:0});
 // Recovery checkpoints expose the actual active model without releasing the mouse.
 const end=Date.now()+6000;let active='';do{await Bun.sleep(100);active=JSON.parse(readFileSync(state,'utf8')).activeWorkspaceId||'';}while(!active.includes(ids[1])&&Date.now()<end);
 assert.ok(active.includes(ids[1]),'Workspace must switch while the primary mouse button is still held');
 await app.call('mouseUp',{x,y,button:0});await Bun.sleep(300);
 assert.ok(JSON.parse(readFileSync(state,'utf8')).activeWorkspaceId.includes(ids[1]));
 console.log('Production workspace switches on mouseDown before mouseUp.');
}finally{if(app)await app.close();for(const id of ids)await Effect.runPromise(client.request('Workspace',{CloseHostWorkspace:{workspace_id:id}})).catch(()=>{});client.stop();}
