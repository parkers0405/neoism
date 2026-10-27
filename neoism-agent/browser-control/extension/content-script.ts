export {}
const firefox = (globalThis as unknown as {browser?: typeof chrome}).browser ?? chrome
// Firefox's status UI changes only Neoism's own DOM. Other extensions' panels,
// password managers, and browser-owned UI are never removed or rewritten.
let host: HTMLElement | undefined
let activeHandoff: string | undefined
function clear(): void { host?.remove(); host = undefined; activeHandoff = undefined }
function render(value: unknown): void {
  if (!value || typeof value !== "object") return
  const status = value as {state?:unknown; message?:unknown; handoffId?:unknown}
  if (!["attached", "running", "waiting"].includes(String(status.state))) return
  clear()
  host = document.createElement("div")
  host.setAttribute("data-neoism-browser-status", "")
  host.style.cssText = "all:initial;position:fixed;z-index:2147483647;bottom:18px;right:18px;max-width:360px"
  const root = host.attachShadow({mode:"closed"})
  const card = document.createElement("div")
  card.style.cssText = "font:13px system-ui;color:#fff;background:#242a38;border:1px solid #5c74ab;border-radius:10px;padding:12px 16px;box-shadow:0 4px 18px #0005"
  card.setAttribute("role", "status")
  card.textContent = typeof status.message === "string" ? status.message.slice(0,1000) : status.state === "running" ? "Neoism is using this tab" : "Attached to Neoism"
  if (status.state === "waiting" && typeof status.handoffId === "string") {
    activeHandoff = status.handoffId
    const button = document.createElement("button")
    button.textContent = "Continue in Neoism"
    button.style.cssText = "display:block;margin-top:10px;padding:7px 12px;border:0;border-radius:5px;background:#526fae;color:white;cursor:pointer"
    button.addEventListener("click", () => {
      const id = activeHandoff
      if (!id) return
      button.disabled = true
      void firefox.runtime.sendMessage({action:"handoff.complete", handoffId:id}).then(result => {
        if (result?.error) throw new Error(result.error)
        clear()
      }).catch(() => { button.disabled = false })
    })
    card.appendChild(button)
  }
  root.appendChild(card)
  document.documentElement.appendChild(host)
}
firefox.runtime.onMessage.addListener((message, sender) => {
  if (sender.id !== firefox.runtime.id) return
  if (message.action === "page-status.set") render(message.status)
  else if (message.action === "page-status.clear") clear()
})
