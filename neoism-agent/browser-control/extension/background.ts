declare const NEOISM_ENGINE: "chrome" | "firefox"
const api = (globalThis as unknown as {browser?:typeof chrome}).browser ?? chrome
const engine = NEOISM_ENGINE
const attached = new Map<number, {tabId:number; targetId:string; url:string; title:string; owned:boolean}>()
const pending = new Map<number, {resolve:()=>void; reject:(error:Error)=>void; timer:ReturnType<typeof setTimeout>}>()
let socket: WebSocket | undefined, ready = false, reconnect: ReturnType<typeof setTimeout> | undefined
const errorText = (error:unknown) => error instanceof Error ? error.message : String(error)
async function status(error?:string):Promise<void> {
  await api.storage.session.set({attachedTabs:[...attached.values()]})
  await api.storage.local.set({connection:{connected:ready, attached:[...attached.keys()], ...(error ? {error}: {})}})
}
function send(value:unknown):void {
  if (!ready || socket?.readyState !== WebSocket.OPEN) throw new Error("Pair Neoism's local browser driver first")
  socket.send(JSON.stringify(value))
}
async function connect():Promise<void> {
  if (reconnect) clearTimeout(reconnect)
  const old = socket; socket = undefined; old?.close(); ready = false
  const {pairingToken, bidiEndpoint = "ws://127.0.0.1:9223/session"} = await api.storage.local.get(["pairingToken","bidiEndpoint"])
  if (typeof pairingToken !== "string" || !/^[a-f0-9]{64}$/.test(pairingToken)) { await status("Enter your local Neoism pairing token"); return }
  const current = new WebSocket(`ws://127.0.0.1:${engine === "chrome" ? 19990 : 19991}/bridge`)
  socket = current
  current.onopen = () => current.send(JSON.stringify({type:"hello",protocol:1,browser:engine,token:pairingToken,endpoint:bidiEndpoint}))
  current.onmessage = event => { void (async () => {
    const message = JSON.parse(String(event.data))
    if (socket !== current) return
    if (message.type === "ready") {
      ready = true; await api.alarms.clear("reconnect"); await status()
      // Rebind approvals after transport loss; input commands are never replayed.
      for (const tab of attached.values()) void attach(tab.tabId,tab.owned).catch(error => status(errorText(error)))
    } else if (message.type === "tab.accepted") {
      const tab = await api.tabs.get(message.tabId)
      attached.set(message.tabId,{tabId:message.tabId,targetId:message.targetId,url:tab.url ?? "",title:tab.title ?? "",owned:attached.get(message.tabId)?.owned ?? false})
      const item = pending.get(message.tabId)
      if (item) { clearTimeout(item.timer); pending.delete(message.tabId); item.resolve() }
      await api.action.setBadgeText({tabId:message.tabId,text:"ON"})
      await api.tabs.sendMessage(message.tabId,{action:"page-status.set",status:{state:"attached"}}).catch(()=>{})
      await status()
    } else if (message.type === "tab.rejected") {
      const item = pending.get(message.tabId)
      if (item) { clearTimeout(item.timer); pending.delete(message.tabId); item.reject(new Error(String(message.error))) }
      await detach(message.tabId); await status(String(message.error))
    } else if (message.type === "request" && typeof message.id === "number") {
      try { current.send(JSON.stringify({type:"reply",id:message.id,result:await command(message.method,message.params ?? {})})) }
      catch (error) { current.send(JSON.stringify({type:"reply",id:message.id,error:errorText(error)})) }
    }
  })().catch(error => status(errorText(error))) }
  current.onclose = () => {
    if (socket !== current) return
    ready = false
    for (const item of pending.values()) { clearTimeout(item.timer); item.reject(new Error("Disconnected; inspect the tab before retrying")) }
    pending.clear(); void status("Neoism driver disconnected")
    reconnect = setTimeout(()=>{void connect()},3000)
    void api.alarms.create("reconnect",{delayInMinutes:0.5})
  }
  current.onerror = () => { void status("Start the Neoism browser driver and reconnect") }
}
async function attach(tabId:number,owned = false):Promise<void> {
  if (!ready) throw new Error("Pair Neoism before attaching a tab")
  if (pending.has(tabId)) throw new Error("Attachment already pending")
  const tab = await api.tabs.get(tabId)
  // Chrome can report an empty URL briefly for a newly created blank tab.
  const url=tab.url || (owned && engine === "chrome" ? "about:blank":"")
  if (!url || (!/^https?:/.test(url) && !(owned && engine === "chrome" && url === "about:blank"))) throw new Error("Select an HTTP(S) page")
  let targetId = "", marker:string | undefined
  if (engine === "chrome") {
    if (!attached.has(tabId)) await chrome.debugger.attach({tabId},"1.3")
    const result = await chrome.debugger.sendCommand({tabId},"Target.getTargetInfo") as {targetInfo:{targetId:string}}
    targetId = result.targetInfo.targetId
  } else {
    marker = crypto.randomUUID()
    await api.scripting.executeScript({target:{tabId},func:(value:string)=>document.documentElement.setAttribute("data-neoism-browser-attachment",value),args:[marker]})
  }
  attached.set(tabId,{tabId,targetId,url,title:tab.title ?? "",owned})
  const completion = new Promise<void>((resolve,reject)=> {
    const timer = setTimeout(()=>{pending.delete(tabId); reject(new Error("Attachment timed out; check the browser driver"))},20000)
    pending.set(tabId,{resolve,reject,timer})
  })
  send({type:"tab.attached",...attached.get(tabId),marker})
  return completion
}
async function detach(tabId:number):Promise<void> {
  const item = pending.get(tabId)
  if (item) { clearTimeout(item.timer); pending.delete(tabId); item.reject(new Error("Tab detached")) }
  attached.delete(tabId)
  if (engine === "chrome") await chrome.debugger.detach({tabId}).catch(()=>{})
  try { send({type:"tab.detached",tabId}) } catch {}
  await api.action.setBadgeText({tabId,text:""}).catch(()=>{})
  await api.tabs.sendMessage(tabId,{action:"page-status.clear"}).catch(()=>{})
  await status()
}
async function command(method:string,params:Record<string,any>):Promise<unknown> {
  if (method === "browser.info") return {product:`Chrome/${navigator.userAgent.match(/(?:Chrome|Chromium)\/([\d.]+)/)?.[1] ?? "146.0"}`,userAgent:navigator.userAgent}
  if (method === "tabs.create") {
    const url = String(params.url ?? "about:blank")
    if (url !== "about:blank" && !/^https?:/.test(url)) throw new Error("New tabs require HTTP(S) or about:blank")
    const tab = await api.tabs.create({url,active:true})
    if (typeof tab.id !== "number") throw new Error("Browser did not return a tab ID")
    if(engine === "firefox") await new Promise<void>((resolve,reject)=> {
      const id=tab.id!
      const timer=setTimeout(()=>{api.tabs.onUpdated.removeListener(listener);reject(new Error("New Firefox tab did not finish loading"))},15000)
      const finish=()=>{clearTimeout(timer);api.tabs.onUpdated.removeListener(listener);resolve()}
      const listener=(tabId:number,change:chrome.tabs.OnUpdatedInfo,current:chrome.tabs.Tab)=>{if(tabId === id && change.status === "complete" && /^https?:/.test(current.url ?? ""))finish()}
      api.tabs.onUpdated.addListener(listener)
      void api.tabs.get(id).then(current=>{if(current.status === "complete" && /^https?:/.test(current.url ?? ""))finish()},reject)
    })
    try { await attach(tab.id,true) }
    catch(error) { const current=await api.tabs.get(tab.id);throw new Error(`Could not attach new ${engine} tab (${current.url}, private=${current.incognito}): ${errorText(error)}`) }
    return attached.get(tab.id)
  }
  const tabId = Number(params.tabId), tab = attached.get(tabId)
  if (!tab) throw new Error("This tab has not been attached to Neoism")
  if (method === "tabs.close") {
    if (!tab.owned) throw new Error("Neoism can only close its own tabs")
    await api.tabs.remove(tabId); return {ok:true}
  }
  if (method === "tabs.detach") { await detach(tabId); return {ok:true} }
  if (method === "pageStatus.set" || method === "pageStatus.clear") {
    await api.tabs.sendMessage(tabId,{action:method === "pageStatus.set" ? "page-status.set":"page-status.clear",status:params.status}); return {ok:true}
  }
  if (method === "cdp.send" && engine === "chrome") return chrome.debugger.sendCommand({tabId,...(params.sessionId ? {sessionId:String(params.sessionId)}:{})},String(params.method),params.params ?? {})
  if (method === "download") {
    const url = new URL(String(params.url))
    if (!["https:","http:"].includes(url.protocol) || url.username || url.password) throw new Error("Download requires an HTTP(S) URL")
    return {downloadId:await api.downloads.download({url:url.href,saveAs:false,...(params.filename ? {filename:String(params.filename)}:{})})}
  }
  throw new Error(`Unsupported Neoism extension method: ${method}`)
}
api.runtime.onMessage.addListener((message,sender,respond)=> {
  if (sender.id !== api.runtime.id) return
  const operation = (async()=> {
    // Configuration and attachment requests must come from our extension UI.
    if (["configure","attach.active","detach.active"].includes(message.action) && sender.url !== api.runtime.getURL("popup.html")) throw new Error("Use the Neoism toolbar popup")
    if (message.action === "configure") { await api.storage.local.set({pairingToken:message.token,bidiEndpoint:message.endpoint}); await connect(); return {ok:true} }
    if (message.action === "attach.active" || message.action === "detach.active") {
      const [tab] = await api.tabs.query({active:true,currentWindow:true})
      if (typeof tab?.id !== "number") throw new Error("No active tab")
      if (message.action === "attach.active") await attach(tab.id); else await detach(tab.id)
      return {ok:true}
    }
    if (message.action === "handoff.complete" && typeof sender.tab?.id === "number" && sender.frameId === 0 && attached.has(sender.tab.id)) {
      send({type:"handoff.complete",handoffId:message.handoffId,tabId:sender.tab.id}); return {ok:true}
    }
    throw new Error("Unknown Neoism extension request")
  })()
  void operation.then(respond,error=>respond({error:errorText(error)})); return true
})
if (engine === "chrome") {
  chrome.debugger.onEvent.addListener((source,method,params)=> {
    if (typeof source.tabId !== "number" || !attached.has(source.tabId)) return
    try { send({type:"cdp.event",tabId:source.tabId,...(source.sessionId ? {sessionId:source.sessionId}:{}),method,params:params ?? {}}) } catch {}
  })
  chrome.debugger.onDetach.addListener(source=> { if (typeof source.tabId === "number" && attached.has(source.tabId)) void detach(source.tabId) })
}
api.tabs.onRemoved.addListener(tabId=>{if (attached.has(tabId)) void detach(tabId)})
api.alarms.onAlarm.addListener(alarm=>{if (alarm.name === "reconnect" && !ready) void connect()})
void api.storage.session.get("attachedTabs").then(stored=> {
  if (Array.isArray(stored.attachedTabs)) for (const tab of stored.attachedTabs) if (Number.isSafeInteger(tab?.tabId)) attached.set(tab.tabId,tab)
  return connect()
})
