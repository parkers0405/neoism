import assert from 'node:assert/strict';
import {readFileSync} from 'node:fs';
import {join} from 'node:path';
import {test} from 'node:test';
import {fileURLToPath} from 'node:url';
import {runInNewContext} from 'node:vm';

const scriptUrl=new URL('./build-gpuix-release.mjs',import.meta.url);
// Run the actual orchestration with fake external commands. No npm installs,
// Rust compilation, native artifacts, or Windows host are needed for this test.
const source=readFileSync(scriptUrl,'utf8')
 .replace(/^import .*;\r?\n/gm,'')
 .replaceAll('import.meta.url',JSON.stringify(scriptUrl.href));

function release(platform,{rejectFocused=false}={}){
 const calls=[];
 let built=false,verified=false;
 const focused=args=>args[0]==='test'&&args[1]==='./tests/window-workspaces.test.ts';
 const context={
  join,fileURLToPath,URL,console:{log(){}},process:{platform,env:{}},
  stageGpuixRelease(root){assert.ok(built);return join(root,'dist');},
  spawnSync(program,args,options){
   calls.push({program,args,options});
   if(args[0]==='scripts/build.ts')built=true;
   if(args[0]==='scripts/verify-core.ts'){
    assert.ok(built,'core verification must follow native build');verified=true;
   }
   if(focused(args)||args[0]==='test'&&args[1]==='./tests'){
    assert.ok(built,'WorkspaceModel tests require a built native core (clean runner has no DLL)');
    assert.ok(verified,'native core must pass FFI verification before model tests');
    assert.ok(options.env.NEOISM_GPUIX_WORKSPACE_DAEMON,'tests must use this build’s daemon');
   }
   return {status:rejectFocused&&focused(args)?1:0};
  },
 };
 return {calls,run:()=>runInNewContext(source,context,{filename:fileURLToPath(scriptUrl)})};
}

test('clean Windows release builds and verifies core before focused rejection and full suite',()=>{
 const {calls,run}=release('win32');run();
 const focused=calls.filter(({args})=>args[1]==='./tests/window-workspaces.test.ts');
 assert.equal(focused.length,1);
 assert.deepEqual(Array.from(focused[0].args),['test','./tests/window-workspaces.test.ts','--test-name-pattern','default resolution sends']);
 assert.ok(calls.some(({args})=>args[0]==='test'&&args[1]==='./tests'),'keep the complete test suite');
 assert.equal(calls.at(-1).args[0],'scripts/package-gpuix-release.py');
});

test('Windows focused rejection failure prevents packaging',()=>{
 const {calls,run}=release('win32',{rejectFocused:true});
 assert.throws(run,/default resolution sends failed \(1\)/);
 assert.ok(!calls.some(({args})=>args[0]==='scripts/package-gpuix-release.py'));
});

test('non-Windows releases retain full tests without the Windows-only focused check',()=>{
 for(const platform of ['linux','darwin']){
  const {calls,run}=release(platform);run();
  assert.ok(!calls.some(({args})=>args[1]==='./tests/window-workspaces.test.ts'));
  assert.ok(calls.some(({args})=>args[0]==='test'&&args[1]==='./tests'));
 }
});