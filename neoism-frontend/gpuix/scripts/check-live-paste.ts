import {readFileSync,writeFileSync,mkdtempSync} from 'node:fs';
import assert from 'node:assert/strict';
import {readClipboard,writeClipboard} from '../src/clipboard';
import {launchNative} from '../tests/support/native-launch';
// Real native clipboard and selection check, using a disposable Agent workload.
const saved=JSON.parse(readFileSync(process.argv[2]||'/tmp/neoism-profile-stream-source.json','utf8'));
if(!saved.project?.startsWith('/tmp/neoism-frame-workload-'))throw Error('Use a disposable frame workload');
const path=mkdtempSync('/tmp/neoism-paste-check-')+'/session.json';
writeFileSync(path,JSON.stringify({version:1,activeWorkspaceId:saved.activeWorkspaceId,activeSurfaceId:saved.surfaces[0].id,surfaces:saved.surfaces,editors:{},providerStates:{},drafts:{},composerDrafts:{},ui:{openWorkspaceIds:[saved.activeWorkspaceId]}}));
const app=await launchNative({command:process.env.LIVE_BINARY||process.execPath,args:process.env.LIVE_BINARY?[]:['src/main.tsx'],cwd:process.cwd(),env:{...process.env,NEOISM_GPUIX_STATE_PATH:path}});
try{
 await Bun.sleep(4500);const flatten=(n:any[]):any[]=>n.flatMap(x=>[x,...flatten(x.children||[])]);
 if(process.env.HYPRLAND_INSTANCE_SIGNATURE){const {pid}=await app.call('initialize',{protocolVersion:1,client:'paste-focus'});await Bun.$`hyprctl dispatch ${`hl.dsp.focus({window="pid:${pid}"})`}`.quiet();await Bun.sleep(300);}
 const tree=await app.call('getTree',{});const tab=flatten([tree.tree]).find(n=>/^pane-tab:\d+$/.test(n.testId||'')&&flatten([n]).some(x=>x.text===saved.surfaces[0].title));if(tab)await app.getByTestId(tab.testId).click();
 const composer=app.getByTestId('composer');await composer.waitFor({timeoutMs:20000});
 for(const key of (process.env.LIVE_PASTE_KEYS?.split(',')||['ctrl-v','cmd-v','ctrl-cmd-v',...(process.env.LIVE_COMPOSITOR?['desktop-super-v']:[])])){
  console.log('Checking',key);if(process.env.HYPRLAND_INSTANCE_SIGNATURE){const {pid}=await app.call('initialize',{protocolVersion:1,client:'paste-focus'});await Bun.$`hyprctl dispatch ${`hl.dsp.focus({window="pid:${pid}"})`}`.quiet();await Bun.sleep(200);}await composer.fill('seed ');await writeClipboard('clipboard test 界');await Bun.sleep(500);if(key==='desktop-super-v'){const {pid}=await app.call('initialize',{protocolVersion:1,client:'paste-desktop-check'});await Bun.$`hyprctl dispatch ${`hl.dsp.focus({window="pid:${pid}"})`}`.quiet();await Bun.$`python3 tests/support/desktop-super-paste.py`.quiet();}else await composer.press(key);await Bun.sleep(2500);
  const draft=JSON.parse(readFileSync(path,'utf8')).composerDrafts[saved.surfaces[0].sessionId]?.text;assert.equal(draft,'seed clipboard test 界');console.log(key,'paste passed');
 }
 await composer.fill('');await Bun.sleep(100);const box=await app.getByTestId('conversation').bounds();
 let selectionChecked=false;
 for(let y=box.y+35;y<box.y+box.height-20;y+=30){
  await app.call('mouseDown',{x:box.x+150,y,button:0});await Bun.sleep(100);
  const t=await app.call('getTree',{});if(flatten([t.tree]).some(n=>n.testId==='timeline-selection-capture')){await Bun.$`grim /tmp/neoism-selection-held.png`.quiet();await app.call('mouseMove',{x:box.x+350,y:y+24,pressedButton:0});await Bun.sleep(100);await app.call('mouseUp',{x:box.x+350,y:y+24,button:0});await Bun.sleep(300);const copied=await readClipboard();assert.ok(copied.length);await app.call('keystrokes',{keys:'ctrl-v'});await Bun.sleep(2500);const draft=JSON.parse(readFileSync(path,'utf8')).composerDrafts[saved.surfaces[0].sessionId]?.text;assert.equal(draft,copied,'selection should paste into the still-focused composer');selectionChecked=true;console.log('Captured held selection; copy then paste without clicking input passed');break;}
  await app.call('mouseUp',{x:box.x+150,y,button:0});
 }
 assert.ok(selectionChecked,'Expected a selectable timeline text row');
}finally{await app.close();}
