#!/usr/bin/env node
// Native host build: every platform produces its own frontend and services.
import {spawnSync} from 'node:child_process';
import {copyFileSync,mkdirSync} from 'node:fs';
import {join} from 'node:path';
import {fileURLToPath} from 'node:url';
const root=fileURLToPath(new URL('..',import.meta.url));
const frontend=join(root,'neoism-frontend/gpuix'),sdk=join(root,'neoism-agent/sdk/typescript');
const windows=process.platform==='win32',suffix=windows?'.exe':'';
const env={...process.env,NEOISM_GPUIX_RUST_BUILDER:'cargo',NEOISM_GPUIX_BUILD_PROFILE:'release',CARGO_PROFILE_RELEASE_DEBUG:'0',CARGO_PROFILE_RELEASE_LTO:'thin',CARGO_PROFILE_RELEASE_CODEGEN_UNITS:'16'};
function run(program,args,cwd=root){
 const result=spawnSync(program,args,{cwd,env,stdio:'inherit',shell:windows&&program==='npm'});
 if(result.error)throw result.error;
 if(result.status!==0)throw Error(`${program} ${args.join(' ')} failed (${result.status})`);
}
run('npm',['ci'],sdk);run('npm',['ci'],frontend);
const bun=join(frontend,'node_modules/bun/bin/bun.exe');
run('cargo',['build','--locked','--release','-p','neoism-workspace-daemon','-p','neoism-agent','-p','neoism-update-helper']);
run('cargo',['test','--locked','--release','-p','neoism-update-helper','--bin','neoism-update-helper']);
env.NEOISM_GPUIX_WORKSPACE_DAEMON=join(root,`target/release/neoism-workspace-daemon${suffix}`);
run(bun,['scripts/build-sdk.ts'],frontend);
run(bun,['scripts/build.ts'],frontend);
run('npm',['run','gui:build'],sdk);
run('node',['scripts/package-agent-gui.mjs',join(sdk,'packages/gui/dist'),join(frontend,'dist/web/agent-gui')]);
const dist=join(frontend,'dist'),licenses=join(dist,'licenses');mkdirSync(licenses,{recursive:true});
for(const name of ['neoism-agent','neoism-agent-lua-runner'])copyFileSync(join(root,`target/release/${name}${suffix}`),join(dist,`${name}${suffix}`));
const runtime=join(dist,'web/native');mkdirSync(runtime,{recursive:true});
copyFileSync(join(root,`target/release/neoism-update-helper${suffix}`),join(runtime,`neoism-update-helper${suffix}`));
for(const name of ['LICENSE','NOTICE'])copyFileSync(join(root,name),join(dist,name));
for(const [source,name]of [
 ['neoism-paint/src/font/resources/CascadiaCode/LICENSE','CascadiaCode.txt'],
 ['neoism-paint/src/font/resources/SymbolsNerdFontMono/LICENSE','SymbolsNerdFontMono.txt'],
 ['neoism-paint/src/font/resources/Geist/OFL.txt','Geist-OFL.txt'],
 ['neoism-paint/src/font/resources/PressStart2P/OFL.txt','PressStart2P-OFL.txt'],
 ['vendor/blaze-compat/icu_collator_data-2.4.0/LICENSE','ICU4X.txt'],
 ['vendor/blaze-compat/licenses/Tantivy.txt','Tantivy.txt'],
 ['vendor/blaze-compat/licenses/Tantivy-AUTHORS.txt','Tantivy-AUTHORS.txt'],
 ['vendor/blaze-compat/licenses/Turso.txt','Turso.txt'],
 ['neoism-frontend/gpuix/node_modules/@gpuix/native/LICENSE','GPUIX-native.txt'],
 ['neoism-frontend/gpuix/node_modules/@gpuix/solid/LICENSE','GPUIX-solid.txt'],
])copyFileSync(join(root,source),join(licenses,name));
run(join(dist,`neoism${suffix}`),['--version']);
run(bun,['scripts/verify-core.ts'],frontend);
run('npm',['run','typecheck'],frontend);
run('npm',['run','check:design'],frontend);
// The host test must find this run's freshly built standalone daemon.
run(bun,['test','./tests'],frontend);
run('python',['scripts/package-gpuix-release.py']);
