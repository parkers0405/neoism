import {test} from "node:test"
import assert from "node:assert/strict"
import {spawn} from "node:child_process"
import fs from "node:fs/promises"
import os from "node:os"
import path from "node:path"
import http from "node:http"
import {once} from "node:events"
import {firefox,type Browser} from "playwright-core"
import {startServer} from "../src/server.ts"
import {connectFirefox,detachFirefoxPage,hydrateFirefoxPages} from "../src/firefox-transport.ts"
import {pageTargetId} from "../src/identity.ts"
import {waitFor,fixture,verifyDriver} from "./helpers.ts"
test("Neoism Firefox extension pairs, preserves user tabs and cancels detached input",{skip:process.env.NEOISM_BROWSER_LIVE!=="1",timeout:60000},async(context)=> {
  const profile=await fs.mkdtemp(path.join(os.tmpdir(),"neoism-firefox-live-")),previous=process.env.NEOISM_BROWSER_STATE_DIR
  process.env.NEOISM_BROWSER_STATE_DIR=path.join(profile,"pairing")
  const server=await startServer("firefox"),app=http.createServer((_,res)=>{res.setHeader("content-type","text/html");res.end(fixture)})
  app.listen(0,"127.0.0.1");await once(app,"listening")
  const url=`http://127.0.0.1:${(app.address() as import("node:net").AddressInfo).port}`
  const child=spawn(process.env.NEOISM_TEST_FIREFOX ?? firefox.executablePath(),["--headless","--no-remote","--profile",profile,"--remote-debugging-port","0","--remote-allow-system-access"],{stdio:["ignore","pipe","pipe"]})
  context.signal.addEventListener("abort",()=>child.kill("SIGTERM"),{once:true})
  let browser:Browser|undefined,diagnostics=""
  child.stderr!.on("data",data=>{diagnostics=(diagnostics+data).slice(-16000)})
  try {
    const endpoint=await waitFor(async()=>{try{const value=JSON.parse(await fs.readFile(path.join(profile,"WebDriverBiDiServer.json"),"utf8"));return `ws://${value.ws_host}:${value.ws_port}/session`}catch{return ""}},Boolean)
    browser=await connectFirefox(endpoint)
    await assert.rejects(connectFirefox(endpoint),/session|active|Maximum/i)
    assert.equal(browser.isConnected(),true)
    const page=await browser.contexts()[0]!.newPage();await page.goto(url);await page.bringToFront()
    const target=await pageTargetId(page)
    const delayed=page.getByRole("button",{name:"Never present"}).click({timeout:5000})
    const cancelled=assert.rejects(delayed,/closed|detached/)
    await new Promise(resolve=>setTimeout(resolve,100));detachFirefoxPage(page);await cancelled
    const [restored]=await hydrateFirefoxPages(browser,{ids:new Set([target])});assert.ok(restored);await restored.bringToFront()
    const addon=path.join(profile,"test-extension");await fs.cp("extension/firefox/dist",addon,{recursive:true})
    const manifest=JSON.parse(await fs.readFile(path.join(addon,"manifest.json"),"utf8"));manifest.background.scripts.unshift("test-bootstrap.js");await fs.writeFile(path.join(addon,"manifest.json"),JSON.stringify(manifest))
    // Test-only extension view drives the exact toolbar handler. Firefox BiDi
    // intentionally cannot automate moz-extension:// pages. None of these
    // files, tokens or automatic approvals enter the packaged extension.
    await fs.writeFile(path.join(addon,"test-bootstrap.js"),`browser.storage.local.get('testConfigured').then(async value=>{if(!value.testConfigured){await browser.storage.local.set(${JSON.stringify({pairingToken:server.token,bidiEndpoint:endpoint,testConfigured:true})});browser.runtime.reload();return}const view=document.createElement('iframe');view.src=browser.runtime.getURL('test-ui.html');document.body.append(view)})`)
    // The test copy appends a driver script to the actual popup document.
    const popupPath=path.join(addon,"popup.html")
    await fs.writeFile(popupPath,(await fs.readFile(popupPath,"utf8")).replace("</body>",'<script src="test-ui.js"></script></body>'))
    await fs.writeFile(path.join(addon,"test-ui.html"),'<!doctype html><iframe src="popup.html"></iframe>')
    await fs.writeFile(path.join(addon,"test-ui.js"),`const timer=setInterval(async()=>{const state=await browser.storage.local.get('connection');if(!state.connection?.connected)return;const tabs=await browser.tabs.query({});const tab=tabs.find(tab=>tab.url===${JSON.stringify(url+"/")});if(!tab)return;clearInterval(timer);await browser.tabs.update(tab.id,{active:true});console.error('neoism-test-attach',JSON.stringify(await browser.runtime.sendMessage({action:'attach.active'})))},300)`)
    const implementation=(browser as any)._connection.toImpl(browser)
    await implementation._browserSession.send("webExtension.install",{extensionData:{type:"path",path:addon}})
    await browser.close();browser=undefined
    await waitFor(async()=>({tabs:server.bridge.tabs.size,error:server.bridge.lastError}),value=>value.tabs>0 || Boolean(value.error))
    assert.equal(server.bridge.lastError,undefined,diagnostics);assert.equal(server.bridge.tabs.size,1)
    await verifyDriver(server.driver,[...server.bridge.tabs.keys()][0]!,url)
    await server.driver.invoke("page_new",{url:url+"/created"})
    assert.equal(server.bridge.tabs.size,2)
    await server.driver.invoke("session_reset")
    await waitFor(async()=>server.bridge.tabs.size,value=>value === 1)
    assert.equal(child.exitCode,null)
  }finally {
    await server.close();await browser?.close().catch(()=>{});child.kill("SIGTERM");if(child.exitCode===null)await once(child,"exit")
    app.closeAllConnections();await new Promise<void>(resolve=>app.close(()=>resolve()))
    if(previous===undefined)delete process.env.NEOISM_BROWSER_STATE_DIR;else process.env.NEOISM_BROWSER_STATE_DIR=previous
    await fs.rm(profile,{recursive:true,force:true})
  }
})
