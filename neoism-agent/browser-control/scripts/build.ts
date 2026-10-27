import {build} from "esbuild"
import fs from "node:fs/promises"
import path from "node:path"
await fs.rm("dist",{recursive:true,force:true});await fs.mkdir("dist",{recursive:true})
for(const name of ["cli","mcp","firefox"]) {
  await build({entryPoints:[`src/${name}.ts`],outfile:`dist/${name}.js`,bundle:true,platform:"node",format:"esm",target:"node22",packages:"external",banner:{js:"#!/usr/bin/env node"}})
  await fs.chmod(`dist/${name}.js`,0o755)
}
for(const engine of ["chrome","firefox"]) {
  const directory=`extension/${engine}/dist`
  await fs.rm(directory,{recursive:true,force:true});await fs.mkdir(directory,{recursive:true})
  for(const name of ["background","popup","content-script"]) await build({entryPoints:[`extension/${name}.ts`],outfile:path.join(directory,`${name}.js`),bundle:true,platform:"browser",format:name === "background" && engine === "chrome" || name === "popup" ? "esm":"iife",target:engine === "chrome" ? "chrome125":"firefox128",define:{NEOISM_ENGINE:JSON.stringify(engine)}})
  for(const name of ["popup.html","popup.css"]) await fs.copyFile(`extension/${name}`,path.join(directory,name))
  await fs.copyFile(`extension/${engine}/manifest.json`,path.join(directory,"manifest.json"))
}
