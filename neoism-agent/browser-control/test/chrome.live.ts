import {test} from "node:test"
import assert from "node:assert/strict"
import {spawn} from "node:child_process"
import fs from "node:fs/promises"
import os from "node:os"
import path from "node:path"
import http from "node:http"
import {once} from "node:events"
import {startServer} from "../src/server.ts"
import {Cdp,waitFor,fixture,verifyDriver} from "./helpers.ts"
test("Neoism Chrome extension pairs and drives real Playwright pages",{skip:process.env.NEOISM_BROWSER_LIVE!=="1",timeout:60000},async(context)=> {
  const profile=await fs.mkdtemp(path.join(os.tmpdir(),"neoism-chrome-live-")), previous=process.env.NEOISM_BROWSER_STATE_DIR
  process.env.NEOISM_BROWSER_STATE_DIR=path.join(profile,"pairing")
  const server=await startServer("chrome"), app=http.createServer((_,res)=>{res.setHeader("content-type","text/html");res.end(fixture)})
  app.listen(0,"127.0.0.1");await once(app,"listening")
  const url=`http://127.0.0.1:${(app.address() as import("node:net").AddressInfo).port}`
  const extension=path.resolve("extension/chrome/dist")
  const child=spawn(process.env.NEOISM_TEST_CHROMIUM ?? "chromium",["--headless=new","--no-sandbox","--disable-gpu",`--user-data-dir=${profile}`,"--remote-debugging-address=127.0.0.1","--remote-debugging-port=0",`--disable-extensions-except=${extension}`,`--load-extension=${extension}`,url],{stdio:["ignore","ignore","pipe"]})
  context.signal.addEventListener("abort",()=>child.kill("SIGTERM"),{once:true})
  let diagnostics="",cdp:Cdp|undefined
  child.stderr!.on("data",data=>{diagnostics=(diagnostics+data).slice(-16000)})
  try {
    const endpoint=await waitFor(async()=>{try {const [port,pathname]=(await fs.readFile(path.join(profile,"DevToolsActivePort"),"utf8")).split("\n");return `ws://127.0.0.1:${port}${pathname}`}catch{return ""}},Boolean)
    cdp=await Cdp.connect(endpoint)
    const targets=await waitFor(async()=> (await cdp!.send("Target.getTargets")).targetInfos as any[],values=>values.some(value=>value.type === "service_worker"))
    const worker=targets.find(value=>value.type === "service_worker")!, id=new URL(worker.url).hostname
    const workerSession=(await cdp.send("Target.attachToTarget",{targetId:worker.targetId,flatten:true})).sessionId
    const popup=(await cdp.send("Target.createTarget",{url:`chrome-extension://${id}/popup.html`})).targetId
    const popupSession=(await cdp.send("Target.attachToTarget",{targetId:popup,flatten:true})).sessionId
    await waitFor(()=>cdp!.evaluate(popupSession,"Boolean(document.querySelector('#connect')) && document.querySelector('#status').textContent !== 'Checking connection…'"),Boolean)
    await cdp.evaluate(popupSession,`document.querySelector('#token').value=${JSON.stringify(server.token)};document.querySelector('#connect').click();true`)
    await waitFor(async()=>({connected:server.bridge.connected,status:await cdp!.evaluate(popupSession,"document.querySelector('#status').textContent"),stored:await cdp!.evaluate(workerSession,"chrome.storage.local.get('connection')")}),value=>value.connected)
    await cdp.evaluate(workerSession,`chrome.tabs.query({}).then(tabs=>chrome.tabs.update(tabs.find(tab=>tab.url === ${JSON.stringify(url+"/")}).id,{active:true}))`)
    await cdp.evaluate(popupSession,"document.querySelector('#attach').click();true")
    await waitFor(async()=>({tabs:server.bridge.tabs.size,error:server.bridge.lastError,popup:await cdp!.evaluate(popupSession,"document.querySelector('#status').textContent")}),value=>value.tabs>0 || Boolean(value.error) || /failed|debugger|denied|timed out/i.test(value.popup))
    assert.equal(server.bridge.lastError,undefined,diagnostics)
    assert.equal(server.bridge.tabs.size,1,await cdp.evaluate(popupSession,"document.querySelector('#status').textContent"))
    await verifyDriver(server.driver,[...server.bridge.tabs.keys()][0]!,url)
    await server.driver.invoke("page_new",{url:url+"/created"})
    assert.equal(server.bridge.tabs.size,2)
    await server.driver.invoke("session_reset")
    await waitFor(async()=>server.bridge.tabs.size,value=>value === 1)
    await cdp.evaluate(workerSession,`chrome.tabs.query({}).then(tabs=>chrome.tabs.update(tabs.find(tab=>tab.url === ${JSON.stringify(url+"/")}).id,{active:true}))`)
    const tabId=[...server.bridge.tabs.keys()][0]!
    await server.driver.invoke("session_adopt",{tabId})
    await server.driver.invoke("execute",{code:"await page.evaluate(()=>{setTimeout(()=>{const b=document.createElement('button');b.textContent='Late arrival';b.onclick=()=>{document.body.dataset.late='clicked'};document.body.append(b)},1500)});return true"})
    await assert.rejects(server.driver.invoke("execute",{code:"await page.getByRole('button',{name:'Late arrival'}).click({timeout:5000})",timeoutMs:1000}),/revoked/)
    await waitFor(async()=>server.bridge.tabs.size,value=>value === 0)
    await server.driver.invoke("session_reset")
    await cdp.evaluate(popupSession,"document.querySelector('#attach').click();true")
    await waitFor(async()=>server.bridge.tabs.size,value=>value === 1)
    await server.driver.invoke("session_adopt",{tabId})
    const noGhost:any=await server.driver.invoke("execute",{code:"await page.getByRole('button',{name:'Late arrival'}).waitFor();return await page.evaluate(()=>document.body.dataset.late ?? 'untouched')"})
    assert.equal(noGhost.result,"untouched")
    await cdp.evaluate(popupSession,"document.querySelector('#detach').click();true")
    await waitFor(async()=>server.bridge.tabs.size,value=>value === 0)
  }finally {
    cdp?.close();child.kill("SIGTERM");if(child.exitCode===null)await once(child,"exit");await server.close()
    app.closeAllConnections();await new Promise<void>(resolve=>app.close(()=>resolve()))
    if(previous===undefined)delete process.env.NEOISM_BROWSER_STATE_DIR;else process.env.NEOISM_BROWSER_STATE_DIR=previous
    await fs.rm(profile,{recursive:true,force:true})
  }
})
