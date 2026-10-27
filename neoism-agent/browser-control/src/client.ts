import { spawn } from "node:child_process"
import path from "node:path"
import { fileURLToPath } from "node:url"
import { pairingToken } from "./bridge.ts"
import { enginePort, type Engine } from "./protocol.ts"

export async function daemonReady(engine:Engine):Promise<void> {
  const url=`http://127.0.0.1:${enginePort(engine)}`
  const health=async()=> {
    try {
      const response=await fetch(`${url}/health`,{signal:AbortSignal.timeout(600)})
      const value=await response.json() as {service?:string;engine?:string;version?:number}
      if (value.service !== "neoism-browser" || value.engine !== engine || value.version !== 1) throw new Error("Port is occupied by an incompatible browser service")
      return true
    } catch(error) {
      if(error instanceof Error && error.message.includes("incompatible"))throw error
      return false
    }
  }
  if(await health())return
  const cli=path.join(path.dirname(fileURLToPath(import.meta.url)),"cli.js")
  const child=spawn(process.execPath,[cli,"serve",engine],{detached:true,stdio:"ignore",env:{...process.env,NEOISM_BROWSER_ENGINE:engine}})
  child.unref()
  for(let attempt=0;attempt<30;attempt++){if(await health())return;await new Promise(resolve=>setTimeout(resolve,100))}
  throw new Error("Neoism browser driver did not start; run neoism-browser serve to see the startup error")
}
export async function invoke(engine:Engine,method:string,args:Record<string,unknown>={}):Promise<unknown> {
  await daemonReady(engine)
  const token=await pairingToken(engine)
  const response=await fetch(`http://127.0.0.1:${enginePort(engine)}/rpc`,{method:"POST",headers:{"Content-Type":"application/json",Authorization:`Bearer ${token}`},body:JSON.stringify({method,args}),signal:AbortSignal.timeout(650000)})
  if(!response.ok)throw new Error(`Neoism browser driver returned HTTP ${response.status}`)
  const value=await response.json() as {result?:unknown;error?:string}
  if(value.error)throw new Error(value.error)
  return value.result
}
