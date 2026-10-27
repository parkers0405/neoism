import fs from "node:fs/promises"
import {spawnSync} from "node:child_process"
import path from "node:path"
const directory=path.resolve(process.argv[2] ?? "artifacts/runtime")
await fs.mkdir(directory,{recursive:true})
for(const file of ["package.json","package-lock.json","README.md","LICENSE"])await fs.copyFile(file,path.join(directory,file))
await fs.cp("dist",path.join(directory,"dist"),{recursive:true})
for(const engine of ["chrome","firefox"])await fs.cp(`extension/${engine}/dist`,path.join(directory,`extension/${engine}/dist`),{recursive:true})
const result=spawnSync("npm",["ci","--omit=dev","--ignore-scripts","--no-audit","--no-fund"],{cwd:directory,stdio:"inherit"})
if(result.status !== 0)throw new Error("Runtime dependency staging failed")
process.stdout.write(`${directory}\n`)
