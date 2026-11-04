#!/usr/bin/env node
// Native host build: every platform produces its own frontend and services.
import {spawnSync} from 'node:child_process';
import {join} from 'node:path';
import {fileURLToPath} from 'node:url';
import {stageGpuixRelease} from './stage-gpuix-release.mjs';
const root=fileURLToPath(new URL('..',import.meta.url));
const frontend=join(root,'neoism-frontend/gpuix'),sdk=join(root,'neoism-agent/sdk/typescript'),serviceClient=join(root,'neoism-frontend/service-client');
const windows=process.platform==='win32',suffix=windows?'.exe':'';
// Experimental device previews remain available to opt-in development builds.
// Release packaging must exclude them even when the caller enabled the feature.
const env={...process.env,NEOISM_GPUIX_DEVICE_PREVIEW:'0',NEOISM_GPUIX_RUST_BUILDER:'cargo',NEOISM_GPUIX_BUILD_PROFILE:'release',CARGO_PROFILE_RELEASE_DEBUG:'0',CARGO_PROFILE_RELEASE_LTO:'thin',CARGO_PROFILE_RELEASE_CODEGEN_UNITS:'16'};
function run(program,args,cwd=root){
 const label=`${program} ${args.join(' ')}`;
 console.log(process.env.GITHUB_ACTIONS?`::group::${label}`:label);
 const result=spawnSync(program,args,{cwd,env,stdio:'inherit',shell:windows&&program==='npm'});
 if(process.env.GITHUB_ACTIONS)console.log('::endgroup::');
 if(result.error)throw result.error;
 if(result.status!==0)throw Error(`${program} ${args.join(' ')} failed (${result.status})`);
}
run('npm',['ci'],sdk);run('npm',['ci'],serviceClient);run('npm',['ci'],frontend);
const bun=join(frontend,'node_modules/bun/bin/bun.exe');
// Version/design drift must fail before the expensive native compilation.
run('npm',['run','check:design'],frontend);
run(bun,['scripts/build-sdk.ts'],frontend);
run('python',['scripts/check-native-toolchain.py']);
// Exercise shell argument forwarding and process ownership on the actual host
// before the much larger service/frontend compilation.
run('cargo',['test','--locked','--release','-p','teletypewriter','macos_exec_tests','--lib']);
run('cargo',['test','--locked','--release','-p','neoism-terminal-pty','--test','process_lifecycle']);
// Fail early on toolchain/network problems before compiling the Rust services.
run(bun,['scripts/build-ghostty.ts'],frontend);
run('cargo',['build','--locked','--release','-p','neoism-workspace-daemon','-p','neoism-agent','-p','neoism-update-helper']);
run('cargo',['test','--locked','--release','-p','neoism-update-helper','--bin','neoism-update-helper']);
env.NEOISM_GPUIX_WORKSPACE_DAEMON=join(root,`target/release/neoism-workspace-daemon${suffix}`);
run(bun,['scripts/build.ts'],frontend);
const dist=stageGpuixRelease(root);
run(join(dist,`neoism${suffix}`),['--version']);
run(bun,['scripts/verify-core.ts'],frontend);
// WorkspaceModel constructs native presence state: even this focused socket
// test requires the freshly built core DLL, not just the generated SDK.
if(windows)run(bun,['test','./tests/window-workspaces.test.ts','--test-name-pattern','default resolution sends'],frontend);
run('npm',['run','typecheck'],frontend);
// The host test must find this run's freshly built standalone daemon.
run(bun,['test','./tests'],frontend);
run('python',['scripts/package-gpuix-release.py']);
