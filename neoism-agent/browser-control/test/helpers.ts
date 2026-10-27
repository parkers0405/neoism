import {WebSocket} from "ws"
import {once} from "node:events"
import assert from "node:assert/strict"
import fs from "node:fs/promises"
import path from "node:path"
import {Client} from "@modelcontextprotocol/sdk/client/index.js"
import {StdioClientTransport} from "@modelcontextprotocol/sdk/client/stdio.js"
import type {Driver} from "../src/driver.ts"
export async function waitFor<T>(read:()=>Promise<T>,accept:(value:T)=>boolean,timeout=15000):Promise<T> {
  const until=Date.now()+timeout;let value:T
  do {value=await read();if(accept(value))return value;await new Promise(resolve=>setTimeout(resolve,100))}while(Date.now()<until)
  throw new Error(`Timed out waiting for fixture: ${JSON.stringify(value!)}`)
}
export class Cdp {
  private id=0
  private pending=new Map<number,{resolve:(value:any)=>void;reject:(error:Error)=>void;timer:NodeJS.Timeout}>()
  constructor(readonly socket:WebSocket) {
    socket.on("message",raw=>{const value=JSON.parse(raw.toString()),item=this.pending.get(value.id);if(!item)return;clearTimeout(item.timer);this.pending.delete(value.id);value.error ? item.reject(new Error(value.error.message)) : item.resolve(value.result)})
  }
  static async connect(endpoint:string):Promise<Cdp>{const socket=new WebSocket(endpoint);await once(socket,"open");return new Cdp(socket)}
  send(method:string,params:object={},sessionId?:string):Promise<any> {
    const id=++this.id
    return new Promise((resolve,reject)=> {const timer=setTimeout(()=>{this.pending.delete(id);reject(new Error(`CDP timeout ${method}`))},15000);this.pending.set(id,{resolve,reject,timer});this.socket.send(JSON.stringify({id,method,params,...(sessionId?{sessionId}:{})}))})
  }
  async evaluate(sessionId:string,expression:string):Promise<any> {const value=await this.send("Runtime.evaluate",{expression,awaitPromise:true,returnByValue:true},sessionId);if(value.exceptionDetails)throw new Error(JSON.stringify(value.exceptionDetails));return value.result.value}
  close():void {for(const value of this.pending.values()){clearTimeout(value.timer);value.reject(new Error("CDP fixture closed"))}this.pending.clear();this.socket.close()}
}
export const fixture=`<!doctype html><title>Neoism local fixture</title><label>Name<input></label><button onclick="document.body.dataset.clicked='yes'">Submit</button><iframe srcdoc="<button>Inside frame</button>"></iframe><input type="password" value="private"><div id="shadow"></div><script>document.querySelector('#shadow').attachShadow({mode:'open'}).innerHTML='<button>Shadow action</button>'</script>`
export async function verifyDriver(driver:Driver,tabId:number,url:string):Promise<void> {
  await driver.invoke("session_adopt",{tabId})
  const snapshot=String(await driver.invoke("snapshot"))
  assert.match(snapshot,/Submit/);assert.match(snapshot,/Inside frame/);assert.match(snapshot,/Shadow action/);assert.doesNotMatch(snapshot,/private/)
  const reference=snapshot.match(/\[(r\d+)\] input "Name"/)?.[1];assert.ok(reference,snapshot)
  const again=String(await driver.invoke("snapshot"));assert.ok(again.includes(`[${reference}] input "Name"`))
  const result:any=await driver.invoke("execute",{code:`await (await ref(${JSON.stringify(reference)})).fill('Neoism independent'); await page.getByRole('button',{name:'Submit',exact:true}).click(); return {value:await page.getByRole('textbox',{name:'Name'}).inputValue(),clicked:await page.evaluate(()=>document.body.dataset.clicked),frames:await page.frameLocator('iframe').getByRole('button',{name:'Inside frame'}).count()}`})
  assert.equal(result.result.value,"Neoism independent");assert.equal(result.result.clicked,"yes");assert.equal(result.result.frames,1)
  const shadowReference=snapshot.match(/\[(r\d+)\] button "Shadow action"/)?.[1];assert.ok(shadowReference)
  await driver.invoke("execute",{code:"await page.locator('#shadow').evaluate(el=>{el.shadowRoot.querySelector('button').textContent='Changed shadow action'});return true"})
  await assert.rejects(driver.invoke("execute",{code:`return (await ref(${JSON.stringify(shadowReference)})).click()`}),/changed/)
  await driver.invoke("network_start")
  await driver.invoke("execute",{code:`await page.evaluate(async url=>{await fetch(url)},${JSON.stringify(url+"/network?secret=should-not-appear")});return true`})
  const network=JSON.stringify(await driver.invoke("network_stop"));assert.match(network,/network/);assert.doesNotMatch(network,/should-not-appear/)
  const image:any=await driver.invoke("screenshot");assert.ok(Buffer.from(image.data,"base64").subarray(0,8).equals(Buffer.from([137,80,78,71,13,10,26,10])))
  await driver.invoke("recording_start",{fps:5})
  await waitFor(()=>driver.invoke("recording_status"),value=>(value as any).frames>=2)
  const video:any=await driver.invoke("recording_stop"), bytes=await fs.readFile(video.path)
  assert.ok(bytes.subarray(0,4).equals(Buffer.from([0x1a,0x45,0xdf,0xa3])))
  await driver.invoke("flight_recorder_start",{fps:5,seconds:2})
  const rolling:any=await driver.invoke("flight_recorder_save_last",{seconds:2});assert.ok((await fs.stat(rolling.path)).size>100)
  await driver.invoke("flight_recorder_cancel")
  await driver.invoke("session_reset")
  // Adopted tabs survive reset and remain available.
  await driver.invoke("session_adopt",{tabId})
  const preserved:any=await driver.invoke("execute",{code:"return await page.getByRole('textbox',{name:'Name'}).inputValue()"});assert.equal(preserved.result,"Neoism independent")
  // Exercise the actual built stdio -> authenticated HTTP -> driver path.
  const transport=new StdioClientTransport({command:process.execPath,args:[path.resolve(`dist/${driver.bridge.engine === "chrome" ? "mcp":"firefox"}.js`)],env:{...Object.fromEntries(Object.entries(process.env).filter((entry):entry is [string,string]=>entry[1]!==undefined)),NEOISM_BROWSER_ENGINE:driver.bridge.engine}})
  const client=new Client({name:"neoism-browser-live-fixture",version:"1.0"})
  try {
    await client.connect(transport)
    const reply=await client.callTool({name:"execute",arguments:{code:"return await page.getByRole('textbox',{name:'Name'}).inputValue()"}})
    assert.equal(reply.isError,undefined);assert.match(JSON.stringify(reply),/Neoism independent/)
  }finally{await client.close();await transport.close()}
}
