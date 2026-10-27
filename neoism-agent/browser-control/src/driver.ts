import { randomUUID } from "node:crypto"
import type { Browser, Page, Request, Response } from "playwright-core"
import { ExtensionBridge } from "./bridge.ts"
import { connectChrome } from "./chrome.ts"
import { connectFirefox, detachFirefoxPage, hydrateFirefoxPages } from "./firefox-transport.ts"
import { pageTargetId } from "./identity.ts"
import { References } from "./snapshot.ts"
import { Recorder } from "./recording.ts"
import { safeHttpUrl, shortError, type Tab, type WireMessage } from "./protocol.ts"

interface NetworkEntry {method:string;url:string;status?:number;error?:string}
interface Session {
  id:string; state:Record<string,unknown>; page?:Page; owned:Set<Page>; queue:Promise<unknown>;
  network?:{page:Page;entries:NetworkEntry[];request:(request:Request)=>void;response:(response:Response)=>void;failed:(request:Request)=>void};
  poisoned:boolean;
  recorder?:Recorder;
}
export class Driver {
  private browser:Browser | undefined
  private connecting:Promise<Browser> | undefined
  private readonly sessions = new Map<string,Session>()
  private readonly refs = new WeakMap<Page,References>()
  private readonly handoffs = new Map<string,{tabId:number;resolve:()=>void;reject:(error:Error)=>void;timer:NodeJS.Timeout}>()
  private current = "default"
  constructor(readonly bridge:ExtensionBridge) {
    this.create("default")
    bridge.on("attach",message=>{void this.attach(message).catch(error=> {
      bridge.lastError = shortError(error)
      if (bridge.connected) bridge.send({type:"tab.rejected",tabId:message.tabId,error:shortError(error)})
    })})
    bridge.on("detach",message=>{void this.detached(Number(message.tabId))})
    bridge.on("event",message=> {
      if (message.type !== "handoff.complete") return
      const waiter = this.handoffs.get(String(message.handoffId))
      if (waiter && waiter.tabId === message.tabId) { clearTimeout(waiter.timer); this.handoffs.delete(String(message.handoffId)); waiter.resolve() }
    })
    bridge.on("disconnected",()=> {
      for (const item of this.handoffs.values()) {clearTimeout(item.timer);item.reject(new Error("Browser disconnected during handoff"))}
      this.handoffs.clear()
      void this.browser?.close().catch(()=>{}); this.browser = undefined; bridge.tabs.clear()
    })
  }
  private create(id:string):Session {
    if (!/^[\w-]{1,80}$/.test(id) || this.sessions.has(id)) throw new Error("Use a unique session name with letters, numbers, underscores or hyphens")
    const session:Session = {id,state:{},owned:new Set(),queue:Promise.resolve(),poisoned:false}
    this.sessions.set(id,session); return session
  }
  private session(id = this.current):Session { const value = this.sessions.get(id); if (!value) throw new Error(`Unknown session ${id}`); return value }
  private serial<T>(session:Session,operation:()=>Promise<T>):Promise<T> {
    const pending = session.queue.catch(()=>{}).then(operation)
    session.queue = pending.catch(()=>{}); return pending
  }
  async connectedBrowser():Promise<Browser> {
    if (this.browser?.isConnected()) return this.browser
    if (!this.bridge.connected) throw new Error("Pair and attach the Neoism browser extension first")
    if (!this.connecting) {
      this.connecting = (this.bridge.engine === "chrome" ? connectChrome(this.bridge) : connectFirefox(this.bridge.endpoint!)).then(browser=> {
        if(!this.bridge.connected) {void browser.close();throw new Error("Browser disconnected during attachment")}
        this.browser = browser
        browser.on("disconnected",()=>{if(this.browser === browser) this.browser = undefined})
        return browser
      }).finally(()=>{this.connecting = undefined})
    }
    return this.connecting
  }
  private async attach(message:WireMessage):Promise<void> {
    const tabId = Number(message.tabId)
    if (!Number.isSafeInteger(tabId) || typeof message.url !== "string") throw new Error("Invalid browser attachment")
    const tab:Tab = {tabId,url:message.url,title:String(message.title ?? ""),owned:message.owned === true,targetId:String(message.targetId ?? "")}
    if (this.bridge.engine === "firefox") {
      if (typeof message.marker !== "string") throw new Error("Firefox attachment needs its exact tab marker")
      const browser = await this.connectedBrowser()
      const pages = await hydrateFirefoxPages(browser,{url:tab.url})
      const candidates:Page[] = []
      for (const page of pages) if (await page.evaluate(()=>document.documentElement.getAttribute("data-neoism-browser-attachment")).catch(()=>null) === message.marker) candidates.push(page)
      if (candidates.length !== 1) throw new Error("Could not verify the exact Firefox tab; reload it and attach again")
      tab.targetId = await pageTargetId(candidates[0]!)
      tab.marker = message.marker
    }
    if (!tab.targetId) throw new Error("Browser did not supply a target identity")
    this.bridge.tabs.set(tabId,tab); this.bridge.emit("attached",{type:"attached",tabId})
    this.bridge.send({type:"tab.accepted",tabId,targetId:tab.targetId})
  }
  private async detached(tabId:number):Promise<void> {
    for (const session of this.sessions.values()) {
      const page = session.page
      if (!page) continue
      const id = await pageTargetId(page).catch(()=>"")
      // The bridge has already removed this tab, so an unlisted page is revoked.
      if (![...this.bridge.tabs.values()].some(tab=>tab.targetId === id)) {
        this.refs.get(page)?.clear(); this.stopNetwork(session);await session.recorder?.cancel();delete session.recorder
        if (this.bridge.engine === "firefox" && !page.isClosed()) detachFirefoxPage(page)
        delete session.page
      }
    }
    for (const [id,item] of this.handoffs) if (item.tabId === tabId) {clearTimeout(item.timer);this.handoffs.delete(id);item.reject(new Error("Tab detached during handoff"))}
  }
  private async attachedTab(page:Page):Promise<Tab> {
    const id = await pageTargetId(page)
    const tab = [...this.bridge.tabs.values()].find(tab=>tab.targetId === id)
    if (!tab || page.isClosed()) throw new Error("This page is detached; attach and adopt it before continuing")
    return tab
  }
  private reference(page:Page):References { let value = this.refs.get(page); if (!value) { value=new References(page);this.refs.set(page,value) } return value }
  private stopNetwork(session:Session):void {
    const item = session.network
    if (item) {item.page.off("request",item.request);item.page.off("response",item.response);item.page.off("requestfailed",item.failed);delete session.network}
  }
  private async reset(session:Session):Promise<void> {
    this.stopNetwork(session)
    if(session.page)this.refs.get(session.page)?.clear()
    await session.recorder?.cancel();delete session.recorder
    for (const page of session.owned) if (!page.isClosed()) await page.close().catch(()=>{})
    session.owned.clear(); delete session.page; session.state={};session.poisoned=false
  }
  async close():Promise<void> { for (const session of this.sessions.values()) await this.reset(session);await this.browser?.close().catch(()=>{});this.bridge.close() }
  status():unknown { return {engine:this.bridge.engine,connected:this.bridge.connected,browserConnected:this.browser?.isConnected() ?? false,tabs:[...this.bridge.tabs.values()],sessions:[...this.sessions.values()].map(s=>({id:s.id,current:s.id === this.current,url:s.page?.url(),needsReset:s.poisoned})),lastError:this.bridge.lastError} }
  async invoke(method:string,args:Record<string,any> = {}):Promise<unknown> {
    if (method === "status") return this.status()
    if (method === "capabilities") return {
      engine:this.bridge.engine,implementation:"neoism",protocol:1,playwright:"1.64.1",snapshot:true,stableReferences:true,frames:true,shadowDOM:"open",networkCapture:true,screenshots:true,handoff:true,directDownloads:true,
      browserExtensionRequired:true,firefoxRemoteDebuggingRequired:this.bridge.engine === "firefox",isolatedContexts:false,recording:"bounded screenshot sampling; requires ffmpeg; no audio",flightRecorder:true,arbitraryCode:"trusted local Node execution, not a sandbox",
    }
    if (method === "session_new") { const s = this.create(String(args.name ?? randomUUID()));return {id:s.id} }
    if (method === "session_list" || method === "session_current") return method === "session_current" ? {id:this.current} : [...this.sessions.keys()]
    const session = this.session(args.session)
    if (method === "session_use") {this.current=session.id;return {id:this.current}}
    return this.serial(session,async()=> {
      if (method === "session_reset") { await this.reset(session); return {id:session.id,reset:true} }
      if (method === "session_delete") { if (session.id === "default") throw new Error("Reset the default session instead");await this.reset(session);this.sessions.delete(session.id);if(this.current===session.id)this.current="default";return {deleted:true} }
      if (session.poisoned) throw new Error("A script timed out and may still have local work running. Reset the session before continuing; do not repeat uncertain actions")
      if (method === "session_adopt") {
        const browser = await this.connectedBrowser(), tab = this.bridge.tabs.get(Number(args.tabId))
        if (!tab) throw new Error("Choose a tab from status after attaching it in the Neoism popup")
        for (const other of this.sessions.values()) if (other !== session) for(const page of new Set([...other.owned,...(other.page?[other.page]:[])])) if(await pageTargetId(page).catch(()=>"") === tab.targetId) throw new Error("This tab belongs to another Neoism session")
        const pages = browser.contexts().flatMap(context=>context.pages())
        for (const page of pages) if (await pageTargetId(page).catch(()=>"") === tab.targetId) {this.stopNetwork(session);await session.recorder?.cancel();delete session.recorder;session.page=page;return {id:session.id,url:page.url()}}
        throw new Error("The attached page is not ready; inspect status before retrying")
      }
      if (method === "page_new") {
        const url = safeHttpUrl(args.url)
        const browser = await this.connectedBrowser()
        let page:Page
        if (this.bridge.engine === "chrome") page=await browser.contexts()[0]!.newPage()
        else {
          // Extension creates and explicitly marks the new tab in the existing profile.
          const tab = await this.bridge.call<Tab>("tabs.create",{url})
          const pages = browser.contexts().flatMap(context=>context.pages())
          page = (await Promise.all(pages.map(async page=>({page,id:await pageTargetId(page)})))).find(item=>item.id===tab.targetId)?.page!
          if (!page) throw new Error("New Firefox tab did not bind to Playwright")
        }
        this.stopNetwork(session);await session.recorder?.cancel();delete session.recorder
        session.page=page;session.owned.add(page)
        if (page.url() !== url) await page.goto(url,{waitUntil:"domcontentloaded",timeout:20000})
        return {url:page.url(),tab:await this.attachedTab(page)}
      }
      const page = session.page
      if (!page) throw new Error("Adopt an attached tab with session_adopt, or open a page with page_new")
      const tab = await this.attachedTab(page), refs = this.reference(page)
      if(method === "recording_start" || method === "flight_recorder_start") {
        if(session.recorder)throw new Error("Stop or cancel the current recorder first")
        const fps=Number(args.fps ?? 4),seconds=Number(args.seconds ?? 30)
        if(!Number.isFinite(fps) || fps<1 || fps>15 || !Number.isFinite(seconds) || seconds<1 || seconds>120)throw new Error("Use 1–15 FPS and a 1–120 second rolling window")
        const recorder=new Recorder(page,method === "flight_recorder_start",fps,seconds)
        await recorder.start();session.recorder=recorder;return recorder.status()
      }
      if(method === "recording_status" || method === "flight_recorder_status")return session.recorder?.status() ?? {active:false}
      if(method === "recording_cancel" || method === "flight_recorder_cancel") {await session.recorder?.cancel();delete session.recorder;return {cancelled:true}}
      if(method === "recording_stop" || method === "flight_recorder_save_last") {
        const recorder=session.recorder;if(!recorder)throw new Error("No recorder in this session")
        if(method === "recording_stop")await recorder.halt()
        const seconds=Number(args.seconds ?? 30)
        if(!Number.isFinite(seconds) || seconds<1 || seconds>120)throw new Error("Save 1–120 seconds")
        const result=await recorder.save(method === "flight_recorder_save_last"?seconds:undefined)
        if(method === "recording_stop"){await recorder.cancel();delete session.recorder}
        return result
      }
      if (method === "snapshot") return refs.snapshot()
      if (method === "screenshot") return {type:"image",mimeType:"image/png",data:(await page.screenshot({fullPage:args.fullPage === true,timeout:10000})).toString("base64")}
      if (method === "network_start") {
        this.stopNetwork(session)
        const entries:NetworkEntry[] = []
        const bounded = (entry:NetworkEntry) => {entries.push(entry);if(entries.length>100)entries.shift()}
        const clean = (url:string) => {try { const u=new URL(url);u.username="";u.password="";u.search="";u.hash="";return u.href.slice(0,1500)} catch{return "[invalid URL]"}}
        const request = (r:Request)=>bounded({method:r.method(),url:clean(r.url())})
        const response = (r:Response)=>bounded({method:r.request().method(),url:clean(r.url()),status:r.status()})
        const failed = (r:Request)=>bounded({method:r.method(),url:clean(r.url()),error:r.failure()?.errorText ?? "request failed"})
        page.on("request",request);page.on("response",response);page.on("requestfailed",failed)
        session.network={page,entries,request,response,failed};return {started:true,limit:100,queryStrings:"omitted"}
      }
      if (method === "network_status" || method === "network_stop") {const entries=session.network?.entries.slice() ?? [];if(method === "network_stop")this.stopNetwork(session);return entries}
      if (method === "download") return this.bridge.call("download",{tabId:tab.tabId,url:safeHttpUrl(args.url),...(args.filename ? {filename:String(args.filename)}:{})})
      if (method === "handoff") {
        const id=randomUUID(), timeout=Math.min(600000,Math.max(1000,Number(args.timeoutMs ?? 300000)))
        const promise=new Promise<void>((resolve,reject)=>{const timer=setTimeout(()=>{this.handoffs.delete(id);reject(new Error("Handoff timed out"))},timeout);this.handoffs.set(id,{tabId:tab.tabId,resolve,reject,timer})})
        try { await Promise.all([this.bridge.call("pageStatus.set",{tabId:tab.tabId,status:{state:"waiting",message:String(args.message ?? "Complete the task, then continue in Neoism"),handoffId:id}}),promise]);return {completed:true} }
        finally { const item=this.handoffs.get(id);if(item){clearTimeout(item.timer);this.handoffs.delete(id)}await this.bridge.call("pageStatus.clear",{tabId:tab.tabId}).catch(()=>{}) }
      }
      if (method === "execute") {
        if (typeof args.code !== "string" || args.code.length>100000) throw new Error("Provide a browser script under 100KB")
        const logs:string[]=[]
        const output={log:(...values:unknown[])=>{if(logs.join("\n").length<16000)logs.push(values.map(v=>typeof v === "string" ? v : JSON.stringify(v)).join(" "))}}
        const AsyncFunction=Object.getPrototypeOf(async function(){}).constructor
        const run=new AsyncFunction("page","state","snapshot","ref","console",args.code)
        let timer:NodeJS.Timeout | undefined
        try {
          const result = await Promise.race([
            run(page,session.state,()=>refs.snapshot(),(id:string)=>refs.get(id),output),
            new Promise<never>((_,reject)=>{timer=setTimeout(()=>{
              session.poisoned=true
              // Revoke ordinary Playwright waits so a delayed locator cannot
              // click after its tool call has timed out. Arbitrary Node work
              // remains trusted and cannot be forcibly cancelled in-process.
              if(this.bridge.engine === "firefox" && !page.isClosed())detachFirefoxPage(page)
              void this.bridge.call("tabs.detach",{tabId:tab.tabId}).catch(()=>{})
              reject(new Error("Script timed out; it may have partially run. The tab was revoked. Reset the session and attach again before continuing"))
            },Math.min(120000,Math.max(1000,Number(args.timeoutMs ?? 30000))))}),
          ])
          return {result:result === undefined ? null : JSON.parse(JSON.stringify(result)),logs:logs.join("\n").slice(0,16000)}
        } finally { if(timer)clearTimeout(timer) }
      }
      throw new Error(`Unknown Neoism browser operation ${method}`)
    })
  }
}
