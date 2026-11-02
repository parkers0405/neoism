import assert from 'node:assert/strict';
import {readFileSync,writeFileSync,mkdtempSync,existsSync,unlinkSync} from 'node:fs';
import {join} from 'node:path';
import {launchNative} from '../tests/support/native-launch';
const saved=JSON.parse(readFileSync(process.argv[2]||'/tmp/neoism-profile-stream-source.json','utf8'));
assert.ok(saved.project?.startsWith('/tmp/neoism-frame-workload-'));
const name=`self-drop-${crypto.randomUUID()}.txt`,file=join(saved.project,'src',name),rootFile=join(saved.project,name);
writeFileSync(file,'Self-drop must preserve this file.\n',{flag:'wx'});
const state=join(mkdtempSync('/tmp/neoism-self-drop-'),'session.json');
writeFileSync(state,JSON.stringify({version:1,activeWorkspaceId:saved.activeWorkspaceId,surfaces:[],editors:{},providerStates:{},drafts:{},composerDrafts:{},ui:{sidebar:'files',openWorkspaceIds:[saved.activeWorkspaceId]}}));
const app=await launchNative({command:join(process.cwd(),'dist/neoism'),args:[],cwd:process.cwd(),env:{...process.env,NEOISM_GPUIX_STATE_PATH:state}});
try{
 await Bun.sleep(4000);const {pid}=await app.call('initialize',{protocolVersion:1,client:'tree-self-drop'});
 if(process.env.HYPRLAND_INSTANCE_SIGNATURE)await Bun.$`hyprctl dispatch ${`hl.dsp.focus({window="pid:${pid}"})`}`.quiet();
 await app.getByTestId('file:src').click();const row=app.getByTestId(`file:src/${name}`);await row.waitFor({timeoutMs:15000});
 const b=await row.bounds(),x=b.x+b.width/2,y=b.y+b.height/2;
 await app.call('mouseDown',{x,y,button:0});await Bun.sleep(200);
 await app.call('mouseMove',{x:x+45,y,pressedButton:0});await app.getByTestId('tree-drag-preview').waitFor({timeoutMs:5000});
 await app.call('mouseMove',{x,y,pressedButton:0});await Bun.sleep(100);await app.call('mouseUp',{x,y,button:0});await Bun.sleep(700);
 assert.ok(existsSync(file),'Source file must remain in its original directory');assert.ok(!existsSync(rootFile),'Self-drop must not move the file to root');assert.equal(readFileSync(file,'utf8'),'Self-drop must preserve this file.\n');
 console.log('Production mouse drag back onto original nested file: unchanged path and contents.');
}finally{await app.close();for(const p of [file,rootFile])if(existsSync(p))unlinkSync(p);}
