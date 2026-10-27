import {test} from "node:test"
import assert from "node:assert/strict"
import fs from "node:fs/promises"
import os from "node:os"
import path from "node:path"
import {Client} from "@modelcontextprotocol/sdk/client/index.js"
import {StdioClientTransport} from "@modelcontextprotocol/sdk/client/stdio.js"
for(const entry of ["mcp","firefox"])test(`${entry} MCP initializes and exposes Neoism tools without starting a browser`,async()=> {
  const directory=await fs.mkdtemp(path.join(os.tmpdir(),"neoism-mcp-fixture-"))
  const transport=new StdioClientTransport({command:process.execPath,args:[path.resolve(`dist/${entry}.js`)],env:{...Object.fromEntries(Object.entries(process.env).filter((entry):entry is [string,string]=>entry[1]!==undefined)),NEOISM_BROWSER_STATE_DIR:directory}})
  const client=new Client({name:"neoism-local-test",version:"1.0"})
  try {
    await client.connect(transport)
    const tools=(await client.listTools()).tools
    for(const name of ["execute","snapshot","session_adopt","network_start","recording_start","flight_recorder_save_last","handoff"])assert.ok(tools.some(tool=>tool.name === name))
    const result=await client.callTool({name:"guide",arguments:{}})
    assert.ok(JSON.stringify(result).includes("Neoism's own local relay"))
    assert.deepEqual(await fs.readdir(directory),[])
  }finally {await client.close();await transport.close();await fs.rm(directory,{recursive:true,force:true})}
})
