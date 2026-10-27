import { EventEmitter } from "node:events"
import { WebSocket } from "ws"
import { timingSafeEqual, randomBytes } from "node:crypto"
import fs from "node:fs/promises"
import os from "node:os"
import path from "node:path"
import type { Engine, Tab, WireMessage } from "./protocol.ts"

export function stateDirectory(): string { return process.env.NEOISM_BROWSER_STATE_DIR ?? path.join(os.homedir(), ".neoism-browser-control") }
export async function pairingToken(engine: Engine): Promise<string> {
  const directory = stateDirectory()
  await fs.mkdir(directory, {recursive:true, mode:0o700})
  const file = path.join(directory, `${engine}-pairing-token`)
  try { await fs.writeFile(file, randomBytes(32).toString("hex"), {flag:"wx", mode:0o600}) }
  catch (error) { if ((error as NodeJS.ErrnoException).code !== "EEXIST") throw error }
  await fs.chmod(file, 0o600)
  const token = (await fs.readFile(file, "utf8")).trim()
  if (!/^[a-f0-9]{64}$/.test(token)) throw new Error("Invalid local browser pairing token")
  return token
}

/** Neoism's extension transport. A failed operation is never resubmitted. */
export class ExtensionBridge extends EventEmitter {
  private socket: WebSocket | undefined
  private next = 0
  private readonly waiting = new Map<number, {resolve:(value:any)=>void; reject:(error:Error)=>void; timer:NodeJS.Timeout}>()
  readonly tabs = new Map<number, Tab>()
  endpoint: string | undefined
  lastError: string | undefined
  constructor(readonly engine: Engine, private readonly token: string) { super() }
  get connected(): boolean { return this.socket?.readyState === WebSocket.OPEN }
  accept(socket: WebSocket): void {
    let paired = false
    const deadline = setTimeout(() => socket.close(4003, "Pairing required"), 3000)
    socket.on("error", () => {})
    socket.on("message", raw => {
      try {
        const message = JSON.parse(raw.toString()) as WireMessage
        if (!paired) {
          if (message.type !== "hello" || message.protocol !== 1 || message.browser !== this.engine || typeof message.token !== "string" || message.token.length !== this.token.length
              || !timingSafeEqual(Buffer.from(message.token), Buffer.from(this.token))) throw new Error("Browser pairing rejected")
          if (this.engine === "firefox") {
            const endpoint = new URL(String(message.endpoint))
            if (endpoint.protocol !== "ws:" || !["127.0.0.1", "[::1]"].includes(endpoint.hostname) || endpoint.pathname !== "/session" || endpoint.username || endpoint.password || endpoint.search || endpoint.hash) throw new Error("Firefox needs a loopback BiDi /session endpoint")
            this.endpoint = endpoint.href
          }
          paired = true; clearTimeout(deadline)
          const previous = this.socket
          this.socket = socket
          if(previous) { previous.close(4001, "New paired connection");this.emit("disconnected") }
          this.rejectWaiting("Browser connection replaced; no operations were replayed")
          socket.send(JSON.stringify({type:"ready", protocol:1}))
          this.emit("connected"); return
        }
        if (socket !== this.socket) return
        if (message.type === "reply" && typeof message.id === "number") {
          const waiter = this.waiting.get(message.id)
          if (!waiter) return
          this.waiting.delete(message.id); clearTimeout(waiter.timer)
          message.error ? waiter.reject(new Error(message.error)) : waiter.resolve(message.result)
        } else if (message.type === "tab.attached") this.emit("attach", message)
        else if (message.type === "tab.detached") {
          if (typeof message.tabId === "number") this.tabs.delete(message.tabId)
          this.emit("detach", message)
        } else this.emit("event", message)
      } catch (error) {
        const message = error instanceof Error ? error.message : "Invalid browser message"
        if (paired) this.lastError = message
        else { clearTimeout(deadline); socket.close(4003, "Pairing rejected") }
      }
    })
    socket.on("close", () => {
      clearTimeout(deadline)
      if (socket !== this.socket) return
      this.socket = undefined
      this.rejectWaiting("Browser extension disconnected; an operation may already have run")
      this.emit("disconnected")
    })
  }
  send(message: WireMessage): void {
    if (!this.connected) throw new Error("Pair the Neoism browser extension first")
    this.socket!.send(JSON.stringify(message))
  }
  call<T = any>(method: string, params: Record<string, unknown> = {}, timeout = 15000): Promise<T> {
    if (!this.connected) return Promise.reject(new Error("Pair the Neoism browser extension first"))
    const id = ++this.next
    return new Promise((resolve, reject) => {
      const timer = setTimeout(() => { this.waiting.delete(id); reject(new Error(`${method} timed out; observe the result before retrying`)) }, timeout)
      this.waiting.set(id, {resolve, reject, timer})
      try { this.send({type:"request", id, method, params}) }
      catch (error) { clearTimeout(timer); this.waiting.delete(id); reject(error as Error) }
    })
  }
  private rejectWaiting(message: string): void {
    for (const waiter of this.waiting.values()) { clearTimeout(waiter.timer); waiter.reject(new Error(message)) }
    this.waiting.clear()
  }
  close(): void { this.rejectWaiting("Neoism browser driver stopped"); this.socket?.close(); this.socket = undefined }
}
