import { createRequire } from "node:module"
import os from "node:os"
import path from "node:path"
import fs from "node:fs/promises"
import type { Browser, Page } from "playwright-core"
import { registerPageTargetId } from "./identity.ts"

const require = createRequire(import.meta.url)
const inputPolicySetters = new WeakMap<Page, (guard: () => void) => void>()
type ExistingPageSelection = {url?:string; ids?:ReadonlySet<string>}
const pageHydrators = new WeakMap<Browser, (selection:ExistingPageSelection)=>Promise<Page[]>>()
const pageDetachers = new WeakMap<Page, () => void>()

export function detachFirefoxPage(page: Page): void {
  const detach = pageDetachers.get(page)
  if (!detach) throw new Error("Firefox page has no Neoism attachment binding")
  detach()
}

export async function hydrateFirefoxPages(browser: Browser, selection:ExistingPageSelection): Promise<Page[]> {
  const hydrate = pageHydrators.get(browser)
  if (!hydrate) throw new Error("Firefox page attachment is unavailable")
  return hydrate(selection)
}

export function registerFirefoxInputGuard(page: Page, guard: () => void): void {
  const install = inputPolicySetters.get(page)
  if (!install) throw new Error("Firefox page has no Neoism input-policy binding")
  install(guard)
}

export function validateFirefoxEndpoint(value: string): string {
  const url = new URL(value)
  if (url.protocol !== "ws:" || !["127.0.0.1", "[::1]"].includes(url.hostname)
      || url.pathname !== "/session" || url.username || url.password || url.search || url.hash) {
    throw new Error("Firefox requires a literal loopback WebDriver BiDi endpoint: ws://127.0.0.1:9223/session")
  }
  return url.href
}

/**
 * Isolate the version-pinned Playwright BiDi integration in one module. Public
 * Firefox.connect speaks Playwright's wire protocol, not Firefox's BiDi wire
 * protocol. The in-process adapter retains real Playwright Pages and Locators;
 * no DOM-only approximation is presented as Playwright.
 */
export async function connectFirefox(endpoint: string, timeout = 15_000): Promise<Browser> {
  validateFirefoxEndpoint(endpoint)
  const core = require("playwright-core/lib/coreBundle")
  const version = require("playwright-core/package.json").version
  if (version !== "1.64.1") throw new Error(`Firefox adapter requires Playwright 1.64.1; found ${version}`)
  const playwright = core.inprocess.createInProcessPlaywright()
  const client = playwright.firefox
  const implementation = playwright._connection.toImpl(client)
  if (!implementation._bidiFirefox?.connectToTransport) throw new Error("Playwright BiDi integration changed")
  const artifactsDir = await fs.mkdtemp(path.join(os.tmpdir(), "neoism-firefox-"))
  const inputPolicies = new Map<string, () => void>()
  implementation.connectOverCDP = async (progress: unknown) => {
    const transport = await core.server.WebSocketTransport.connect(progress, endpoint)
    let sessionRequest: number | undefined
    let ownsSession = false
    let onmessage = transport.onmessage
    Object.defineProperty(transport, "onmessage", {configurable:true,
      get:() => onmessage,
      set:(handler: ((message:any)=>void) | undefined) => {
        onmessage = handler ? (message:any) => {
          if (message.id === sessionRequest && message.type === "success" && message.result?.sessionId) ownsSession = true
          handler(message)
        } : undefined
      },
    })
    const send = transport.send.bind(transport)
    transport.send = (message: {id?:number; method:string; params?:{context?:string}}) => {
      if (message.method === "session.new") sessionRequest = message.id
      if (message.method.startsWith("input.") && message.method !== "input.releaseActions" && message.params?.context) inputPolicies.get(message.params.context)?.()
      send(message)
    }
    let closed = false
    const disconnect = async () => {
      if (closed) return
      closed = true
      if (!ownsSession) { transport.close(); await fs.rm(artifactsDir, {recursive:true, force:true}); return }
      // End only our automation session. Never issue browser.close against a
      // user's Firefox. Closing the socket alone can orphan the BiDi session.
      await new Promise<void>((resolve) => {
        const previous = transport.onmessage
        const timer = setTimeout(resolve, 1500)
        transport.onmessage = (message: { id?: number }) => {
          if (message.id === 2_147_483_647) { clearTimeout(timer); resolve() }
          else previous?.(message)
        }
        try { transport.send({id:2_147_483_647, method:"session.end", params:{}}) }
        catch { clearTimeout(timer); resolve() }
      })
      transport.close()
      await fs.rm(artifactsDir, {recursive:true, force:true})
    }
    try {
      return await implementation._bidiFirefox.connectToTransport(transport, {
        name:"firefox", browserType:"firefox", isChromium:false, slowMo:0, headful:true,
        persistent:{noDefaultViewport:true, acceptDownloads:"accept"},
        artifactsDir, downloadsPath:artifactsDir, tracesDir:artifactsDir,
        originalLaunchOptions:{},
        browserProcess:{close:disconnect, kill:disconnect},
        browserLogsCollector:{recentLogs:()=>[]},
        protocolLogger:()=>{},
      })
    } catch (error) { await disconnect(); throw error }
  }
  try {
    const result = await client._channel.connectOverCDP({endpointURL:endpoint}, timeout)
    const browser: Browser = await client._browserFromConnectResult(result)
    const bindPage = (page: Page) => {
      const impl = playwright._connection.toImpl(page)
      const id = impl?.delegate?._session?.sessionId
      if (typeof id !== "string") throw new Error("Firefox browsing context identity unavailable")
      inputPolicySetters.set(page, guard => inputPolicies.set(id, guard))
      pageDetachers.set(page, () => {
        const bidiPage = server._bidiPages.get(id)
        if (!bidiPage) return
        server._bidiPages.delete(id)
        inputPolicies.set(id, () => { throw new Error("Firefox tab was detached; input is revoked") })
        // Dispose the automation page/session only. This cancels locator
        // auto-waits without issuing browsingContext.close against the tab.
        bidiPage.didClose()
      })
      const keyboard = impl.delegate.rawKeyboard
      const sendText = keyboard.sendText.bind(keyboard)
      keyboard.sendText = (...args: unknown[]) => { inputPolicies.get(id)?.(); return sendText(...args) }
      page.on("close", () => inputPolicies.delete(id))
      registerPageTargetId(page, async () => {
        return id
      })
    }
    for (const context of browser.contexts()) {
      context.pages().forEach(bindPage)
      context.on("page", bindPage)
    }
    const newContext = browser.newContext.bind(browser)
    browser.newContext = async options => {
      const context = await newContext(options)
      context.on("page", bindPage)
      return context
    }
    const server = playwright._connection.toImpl(browser)
    pageHydrators.set(browser, async selection => {
      // Playwright's launch path assumes that pages/realms are created after it
      // subscribes. Attaching to an existing Firefox requires a bounded replay
      // for the explicitly selected pages, not a reload of the user's tabs.
      const tree = await server._browserSession.send("browsingContext.getTree", {})
      const roots = tree.contexts.filter((info:any) => selection.ids?.has(info.context) || (selection.url !== undefined && info.url === selection.url))
      const contexts = new Set<string>()
      const replay = (info:any, parent:string | null) => {
        contexts.add(info.context)
        if (!server._findPageForFrame(info.context)) server._onBrowsingContextCreated({...info, parent})
        for (const child of info.children ?? []) replay(child, info.context)
      }
      for (const info of roots) replay(info, null)
      const {realms} = await server._browserSession.send("script.getRealms", {})
      for (const realm of realms) if (contexts.has(realm.context)) {
        server._connection._dispatchMessage({type:"event", method:"script.realmCreated", params:realm})
      }
      for (const info of roots) {
        const page = await server._bidiPages.get(info.context)?._page.waitForInitializedOrError()
        if (page instanceof Error) throw page
      }
      await new Promise(resolve => setImmediate(resolve))
      return browser.contexts().flatMap(context => context.pages()).filter(page => {
        const impl = playwright._connection.toImpl(page)
        return contexts.has(impl.delegate._session.sessionId)
      })
    })
    return browser
  } catch (error) {
    await fs.rm(artifactsDir, {recursive:true, force:true})
    throw error
  }
}
