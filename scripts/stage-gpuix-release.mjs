#!/usr/bin/env node
// Stage already-built production services and notices without recompiling them.
import {copyFileSync,mkdirSync,renameSync} from 'node:fs';
import {join,resolve} from 'node:path';
import {fileURLToPath} from 'node:url';

export function stageGpuixRelease(root=fileURLToPath(new URL('..',import.meta.url))){
 const suffix=process.platform==='win32'?'.exe':'';
 const dist=join(root,'neoism-frontend/gpuix/dist'),licenses=join(dist,'licenses');
 mkdirSync(licenses,{recursive:true});
 // Replacing the inode keeps an already-running local service untouched.
 const stageBinary=(name,directory)=>{
  const destination=join(directory,`${name}${suffix}`),pending=`${destination}.next`;
  copyFileSync(join(root,`target/release/${name}${suffix}`),pending);renameSync(pending,destination);
 };
 for(const name of ['neoism-agent','neoism-agent-lua-runner'])stageBinary(name,dist);
 const runtime=join(dist,'web/native');mkdirSync(runtime,{recursive:true});
 stageBinary('neoism-update-helper',runtime);
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
 return dist;
}

if(process.argv[1]&&resolve(process.argv[1])===fileURLToPath(import.meta.url))console.log(stageGpuixRelease());