import { randomUUID } from "node:crypto"
import { chromium, type Browser, type ConnectOverCDPTransport } from "playwright-core"
import { ExtensionBridge } from "./bridge.ts"
import { registerPageTargetId } from "./identity.ts"
import type { Tab, WireMessage } from "./protocol.ts"

interface CdpMessage { id?:number; method:string; params?:Record<string, any>; sessionId?:string }
interface Route { tabId:number; targetId:string; nativeSession?:string }

/** Map Chrome's extension debugger to Playwright's public CDP transport API. */
export class ChromeConnection implements ConnectOverCDPTransport {
  onmessage?: (message:object)=>void
  onclose?: (reason?:string)=>void
  private readonly routes = new Map<string, Route>()
  private readonly browserRoutes = new Set<string>()
  private readonly contexts = new Map<number, Map<number, unknown>>()
  private automatic = false
  private closed = false
  private readonly events: (message:WireMessage)=>void
  private readonly attached: (message:WireMessage)=>void
  private readonly detached: (message:WireMessage)=>void
  private readonly disconnected: ()=>void
  constructor(readonly bridge: ExtensionBridge) {
    this.events = message => this.event(message)
    this.attached = message => { const tab = this.bridge.tabs.get(Number(message.tabId)); if (tab) this.announce(tab) }
    this.detached = message => this.forget(Number(message.tabId))
    this.disconnected = () => { this.close() }
    bridge.on("event", this.events); bridge.on("attached", this.attached); bridge.on("detach", this.detached); bridge.on("disconnected", this.disconnected)
  }
  private emit(message:object): void { if (!this.closed) queueMicrotask(() => this.onmessage?.(message)) }
  private info(tab:Tab): object { return {targetId:tab.targetId,type:"page",title:tab.title,url:tab.url,attached:true,canAccessOpener:false,browserContextId:"neoism-default-profile"} }
  private announce(tab:Tab): void {
    this.emit({method:"Target.targetCreated",params:{targetInfo:this.info(tab)}})
    if (this.automatic && !Array.from(this.routes.values()).some(route => route.tabId === tab.tabId && !route.nativeSession)) {
      const sessionId = randomUUID(); this.routes.set(sessionId,{tabId:tab.tabId,targetId:tab.targetId})
      this.emit({method:"Target.attachedToTarget",params:{sessionId,targetInfo:this.info(tab),waitingForDebugger:false}})
    }
  }
  private forget(tabId:number): void {
    for (const [id,route] of this.routes) if (route.tabId === tabId) { this.routes.delete(id); this.emit({method:"Target.detachedFromTarget",params:{sessionId:id,targetId:route.targetId}}) }
    this.contexts.delete(tabId)
  }
  private event(message:WireMessage): void {
    if (message.type !== "cdp.event" || typeof message.method !== "string") return
    const tabId = Number(message.tabId), params = (message.params ?? {}) as Record<string,any>
    if (message.method === "Runtime.executionContextCreated") {
      const values = this.contexts.get(tabId) ?? new Map(); values.set(params.context.id, params.context); this.contexts.set(tabId,values)
    } else if (message.method === "Runtime.executionContextsCleared") this.contexts.delete(tabId)
    else if (message.method === "Runtime.executionContextDestroyed") this.contexts.get(tabId)?.delete(params.executionContextId)
    const nativeSession = typeof message.sessionId === "string" ? message.sessionId : undefined
    if (message.method === "Target.attachedToTarget" && typeof params.sessionId === "string") this.routes.set(params.sessionId,{tabId,targetId:params.targetInfo.targetId,nativeSession:params.sessionId})
    for (const [sessionId,route] of this.routes) if (route.tabId === tabId && route.nativeSession === nativeSession) {
      this.emit({method:message.method, params, sessionId})
    }
    if (message.method === "Page.frameNavigated" && !params.frame.parentId) {
      const tab = this.bridge.tabs.get(tabId); if (tab) tab.url = params.frame.url
    }
  }
  send(raw:object): void {
    const message = raw as CdpMessage
    if(process.env.NEOISM_BROWSER_DEBUG === "1")process.stderr.write(`Neoism CDP ${message.id}: ${message.method}\n`)
    void this.dispatch(message).then(result => this.emit({id:message.id,result,...(message.sessionId ? {sessionId:message.sessionId}: {})}), error => this.emit({id:message.id,error:{code:-32000,message:error instanceof Error ? error.message : String(error)},...(message.sessionId ? {sessionId:message.sessionId}: {})}))
  }
  private async dispatch(message:CdpMessage): Promise<any> {
    const params = message.params ?? {}, route = message.sessionId ? this.routes.get(message.sessionId) : undefined
    if (!message.sessionId || this.browserRoutes.has(message.sessionId) || message.method.startsWith("Target.")) {
      switch (message.method) {
        case "Target.attachToBrowserTarget": {const sessionId=randomUUID();this.browserRoutes.add(sessionId);return {sessionId}}
        case "Browser.getVersion": {
          const browser = await this.bridge.call("browser.info")
          return {protocolVersion:"1.3",product:browser.product,userAgent:browser.userAgent,revision:"neoism",jsVersion:"0"}
        }
        case "Target.getBrowserContexts": return {browserContextIds:[]}
        case "Target.getTargets": return {targetInfos:Array.from(this.bridge.tabs.values(),tab=>this.info(tab))}
        case "Target.setDiscoverTargets": for (const tab of this.bridge.tabs.values()) this.announce(tab); return {}
        case "Target.setAutoAttach":
          if (!message.sessionId) { this.automatic = params.autoAttach === true; for (const tab of this.bridge.tabs.values()) this.announce(tab); return {} }
          break
        case "Target.createTarget": {
          if (params.browserContextId) throw new Error("Neoism controls the paired browser profile; isolated contexts are unavailable")
          const tab = await this.bridge.call<Tab>("tabs.create",{url:params.url ?? "about:blank"})
          this.bridge.tabs.set(tab.tabId,tab); this.announce(tab); return {targetId:tab.targetId}
        }
        case "Target.getTargetInfo": {
          if((!message.sessionId || this.browserRoutes.has(message.sessionId)) && !params.targetId) return {targetInfo:{targetId:"neoism-browser",type:"browser",title:"Neoism",url:"",attached:true}}
          const tab = Array.from(this.bridge.tabs.values()).find(tab=>tab.targetId===params.targetId || (!params.targetId && tab.tabId===route?.tabId))
          if (!tab) throw new Error("The requested Chrome target is not attached")
          return {targetInfo:this.info(tab)}
        }
        case "Target.attachToTarget": {
          const tab = Array.from(this.bridge.tabs.values()).find(tab=>tab.targetId===params.targetId)
          if (!tab) throw new Error("Attach the Chrome tab before using it")
          const sessionId = randomUUID(); this.routes.set(sessionId,{tabId:tab.tabId,targetId:tab.targetId}); return {sessionId}
        }
        case "Target.detachFromTarget": {
          if (typeof params.sessionId === "string" && this.browserRoutes.delete(params.sessionId)) return {}
          if (typeof params.sessionId === "string" && this.routes.delete(params.sessionId)) return {}
          break
        }
        case "Target.closeTarget": {
          const tab = Array.from(this.bridge.tabs.values()).find(tab=>tab.targetId===params.targetId)
          if (!tab?.owned) throw new Error("Neoism only closes tabs created by its driver")
          await this.bridge.call("tabs.close",{tabId:tab.tabId}); return {success:true}
        }
        case "Browser.setDownloadBehavior": return {}
        case "Browser.close": throw new Error("Closing the user's browser is not a Neoism operation")
      }
    }
    if (!route) throw new Error(`No attached Chrome route for ${message.method}`)
    if (!this.bridge.tabs.has(route.tabId)) throw new Error("Chrome tab detached; operation was not replayed")
    const result = await this.bridge.call("cdp.send", {tabId:route.tabId,...(route.nativeSession ? {sessionId:route.nativeSession}:{}),method:message.method,params}, 30000)
    if (message.method === "Runtime.enable") for (const context of this.contexts.get(route.tabId)?.values() ?? []) this.emit({method:"Runtime.executionContextCreated",params:{context},sessionId:message.sessionId})
    return result
  }
  close(): void {
    if (this.closed) return; this.closed = true
    this.bridge.off("event",this.events); this.bridge.off("attached",this.attached); this.bridge.off("detach",this.detached); this.bridge.off("disconnected",this.disconnected)
    this.routes.clear(); this.contexts.clear();this.browserRoutes.clear()
    this.onclose?.("Neoism Chrome transport closed")
  }
}

export async function connectChrome(bridge:ExtensionBridge): Promise<Browser> {
  const transport = new ChromeConnection(bridge)
  try {
    const browser = await chromium.connectOverCDP(transport,{timeout:15000,noDefaults:true})
    const bind = (page:import("playwright-core").Page) => {
      let identity:Promise<string>|undefined
      registerPageTargetId(page, () => identity ??= (async()=> {
        const session = await page.context().newCDPSession(page)
        try { return (await session.send("Target.getTargetInfo")).targetInfo.targetId }
        finally { await session.detach() }
      })())
    }
    for (const context of browser.contexts()) { context.pages().forEach(bind); context.on("page",bind) }
    return browser
  } catch (error) { transport.close(); throw error }
}
