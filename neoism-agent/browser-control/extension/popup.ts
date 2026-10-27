export {}
declare const NEOISM_ENGINE:"chrome"|"firefox"
const firefox = (globalThis as unknown as {browser?: typeof chrome}).browser ?? chrome
const status = document.querySelector<HTMLElement>("#status")!
const token = document.querySelector<HTMLInputElement>("#token")!
const endpoint = document.querySelector<HTMLInputElement>("#endpoint")!
if (NEOISM_ENGINE === "chrome") { endpoint.closest("label")!.remove(); document.querySelector("#firefox-note")!.remove() }
const stored = await firefox.storage.local.get(["pairingToken", "bidiEndpoint", "connection"])
token.value = typeof stored.pairingToken === "string" ? stored.pairingToken : ""
endpoint.value = typeof stored.bidiEndpoint === "string" ? stored.bidiEndpoint : "ws://127.0.0.1:9223/session"
const render = (value: unknown) => {
  const connection = value && typeof value === "object" ? value as {connected?:boolean; error?:string} : undefined
  status.textContent = connection?.error ?? (connection?.connected ? "Connected to Neoism" : "Neoism is disconnected")
}
render(stored.connection)
firefox.storage.onChanged.addListener(changes => { if (changes.connection) render(changes.connection.newValue) })
async function invoke(message: unknown): Promise<void> {
  status.textContent = "Working…"
  try { const result = await firefox.runtime.sendMessage(message); if (result?.error) throw new Error(result.error); render((await firefox.storage.local.get("connection")).connection) }
  catch (error) { status.textContent = error instanceof Error ? error.message : String(error) }
}
document.querySelector("#connect")!.addEventListener("click", () => { void invoke({action:"configure", token:token.value.trim(), endpoint:endpoint.value.trim()}) })
document.querySelector("#attach")!.addEventListener("click", () => { void invoke({action:"attach.active"}) })
document.querySelector("#detach")!.addEventListener("click", () => { void invoke({action:"detach.active"}) })
