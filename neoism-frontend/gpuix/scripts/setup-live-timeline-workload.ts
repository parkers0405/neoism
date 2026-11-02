// Disposable, explicitly requested workload on the real local Agent service.
import {mkdtempSync,writeFileSync,mkdirSync,readFileSync} from 'node:fs';
import {join} from 'node:path';
import {tmpdir} from 'node:os';
import {createHttpClient} from '@neoism/sdk';
if(!process.argv[2])throw Error('Usage: bun scripts/setup-live-timeline-workload.ts recovery.json [output.json]');
const recovery=JSON.parse(readFileSync(process.argv[2],'utf8'));
const w=recovery.activeWorkspaceId;if(typeof w!=='string')throw Error('Recovery requires an active workspace');
const diffCards=process.env.LIVE_DIFF_CARDS==='1';
const output=process.argv[3]||join(tmpdir(),'neoism-profile-stream-source.json');
const source={activeWorkspaceId:w,surfaces:[{id:'',kind:'agent',title:'',workspaceId:w,sessionId:''}],project:''};
let id='';
const directory=mkdtempSync(join(tmpdir(),'neoism-frame-workload-'));
mkdirSync(join(directory,'src'));mkdirSync(join(directory,'tests'));
writeFileSync(join(directory,'package.json'),JSON.stringify({name:'frame-workload',type:'module',scripts:{test:'node --test'}}));
const modules=['money','inventory','orders','discounts','shipping','tax','reports','returns','customers','ledger'];
for(const name of modules)writeFileSync(join(directory,'src',name+'.js'),`// Disposable inventory application: implement these functions.\nexport function ${name}(entries, options = {}) {\n  return null; // TODO\n}\n`);
writeFileSync(join(directory,'README.md'),'# Disposable inventory application\nImplement each module as a pure function over an array of entries. Use integer cents. Add node:test coverage for normal and edge cases. No network, packages, external services or files outside this project.\n');
await Bun.$`git -C ${directory} init -q`.quiet();
const system=createHttpClient({baseUrl:process.env.NEOISM_AGENT_SERVER||'http://127.0.0.1:4096/'});
try{
 const session=await system.sessions.create({title:diffCards?'Large patch timeline workload':'Paginated live rendering workload'});id=session.id;session.directory=directory;session.agent='build';session.model={providerId:'openai',id:'gpt-6.1-sol',variant:'medium'};source.surfaces[0].sessionId=id;source.surfaces[0].id='agent:'+id;
 const messages:any[]=[];const origin=Date.now()-86400000;
 for(let turn=0;turn<240;turn++){
  const created=origin+turn*1000,uid=`msg_${created.toString(16).padStart(12,'0')}000000000000`,aid=`msg_${created.toString(16).padStart(12,'0')}000000000001`;
  const mid=(role:string,messageId:string)=>({id:messageId,sessionId:id,role,time:{created,...role==='assistant'?{completed:created+500}:{}},agent:'build'});
  const file=modules[turn%modules.length]+'.js';
  const code=Array.from({length:24},(_,i)=>`const sample_${i} = { sku: "item-${turn}-${i}", quantity: ${i+1}, cents: ${(i+1)*125} };`).join('\n');
  messages.push({info:{...mid('user',uid),model:{providerId:'openai',modelId:'gpt-6.1-sol'}},parts:[{type:'text',id:`prt_${turn}_user`,sessionId:id,messageId:uid,text:`Historical diagnostic example ${turn}: review ${file} inventory arithmetic and report the result. These are seeded examples, not work to redo.`}]});
  messages.push({info:{...mid('assistant',aid),parentId:uid,mode:'build',path:{cwd:directory,root:directory},cost:0,tokens:{input:0,output:0,reasoning:0,cache:{read:0,write:0}},modelId:'gpt-6.1-sol',providerId:'openai',finish:'stop'},parts:[{type:'tool',id:`prt_${turn}_tool`,sessionId:id,messageId:aid,callId:`call_history_${turn}`,tool:'read',state:{status:'completed',input:{filePath:join(directory,'src',file)},output:code,title:`Read src/${file}`,metadata:{},time:{start:created+50,end:created+100}}},{type:'text',id:`prt_${turn}_assistant`,sessionId:id,messageId:aid,text:`## Historical review ${turn}\n\nReviewed the sample inventory arithmetic.\n\n\`\`\`js\n${code}\n\`\`\`\n\n| Check | Result |\n|---|---|\n| Integer cents | Pass |\n| Empty inputs | Pass |\n\nThis diagnostic example is complete.`}]});
  if(diffCards){
   const patchText='*** Begin Patch\n'+Array.from({length:2},(_,section)=>'*** Update File: src/'+modules[(turn+section)%modules.length]+'.js\n@@\n'+Array.from({length:turn%24===0?160:4},(_,row)=>`-export const old${row} = {sku: "before-${turn}-${row}", cents: ${row*125}, note: "${'old '.repeat(row%3)}"};\n+export const item${row} = {sku: "after-${turn}-${row}", cents: ${row*150}, note: "${'new '.repeat(row%3)}"};`).join('\n')).join('\n')+'\n*** End Patch';
   messages.at(-1).parts[0]={type:'tool',id:`prt_${turn}_tool`,sessionId:id,messageId:aid,callId:`call_history_${turn}`,tool:'apply_patch',state:{status:'completed',input:{patchText},output:'Success. Updated two diagnostic modules.',title:'Apply patch',metadata:{},time:{start:created+50,end:created+100}}};
   messages.at(-1).parts[1].text=`Historical patch ${turn} is complete. These are rendering fixtures, not changes to repeat.`;
  }
 }
 await system.operations.request('v2.sessions.import',{body:{bundle:{version:3,session,messages,queuedPrompts:[],workspaceRoot:directory},targetWorkspaceRoot:directory}});
 source.surfaces[0].title=session.title;source.project=directory;
 writeFileSync(output,JSON.stringify(source),{mode:0o600});
 console.log(JSON.stringify({directory,messages:messages.length,session:id,output}));
}catch(error){if(id)await system.sessions.delete(id).catch(()=>{});throw error;}

