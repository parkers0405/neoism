import fs from "node:fs/promises"
import path from "node:path"
import {zipSync} from "fflate"
await fs.mkdir("artifacts",{recursive:true})
for(const engine of ["chrome","firefox"]) {
  const directory=`extension/${engine}/dist`, files:Record<string,Uint8Array>={}
  for(const entry of await fs.readdir(directory,{withFileTypes:true})) {
    if(!entry.isFile() || entry.name.endsWith(".map") || /test|bootstrap/i.test(entry.name))throw new Error(`Unexpected extension artifact ${entry.name}`)
    files[entry.name]=new Uint8Array(await fs.readFile(path.join(directory,entry.name)))
  }
  files["LICENSE"]=new Uint8Array(await fs.readFile("LICENSE"))
  const file=`artifacts/neoism-browser-${engine}-0.2.0.${engine === "chrome" ? "zip":"xpi"}`
  await fs.writeFile(file,zipSync(files,{level:9}));process.stdout.write(`${file}\n`)
}
