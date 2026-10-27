import type { ElementHandle, Page, Frame } from "playwright-core"
interface Entry {handle:ElementHandle<Element>; signature:string; frame:Frame}
export class References {
  private readonly entries = new Map<string,Entry>()
  private sequence = 0
  constructor(readonly page:Page) {
    page.on("framenavigated",()=>this.clear())
    page.on("close",()=>this.clear())
  }
  clear():void { for (const value of this.entries.values()) void value.handle.dispose().catch(()=>{}); this.entries.clear() }
  async get(id:string):Promise<ElementHandle<Element>> {
    const entry = this.entries.get(id)
    if (!entry) throw new Error(`Unknown or expired reference ${id}; take another snapshot`)
    const signature = await entry.handle.evaluate(element=> {
      if (!element.isConnected) return null
      return [element.tagName,element.getAttribute("role"),element.getAttribute("type"),element.getAttribute("aria-label"),(element as HTMLInputElement).labels?.[0]?.textContent,element.textContent?.trim().slice(0,100)].join("|")
    }).catch(()=>null)
    if (signature !== entry.signature) { this.entries.delete(id); await entry.handle.dispose().catch(()=>{}); throw new Error(`Reference ${id} changed; take another snapshot`) }
    return entry.handle
  }
  async snapshot():Promise<string> {
    const lines = [`${this.page.url()} — ${await this.page.title()}`]
    const found = new Set<string>()
    let count = 0
    for (const frame of this.page.frames().slice(0,16)) {
      if (count >= 150) break
      const collection = await frame.evaluateHandle((remaining:number)=> {
        const result:Element[] = []
        const roots:(Document|ShadowRoot)[] = [document]
        let scanned=0
        while(roots.length && result.length < remaining && scanned<6000) {
          const root=roots.pop()!
          const walker = document.createTreeWalker(root,NodeFilter.SHOW_ELEMENT)
          let node:Node|null
          while ((node=walker.nextNode()) && result.length < remaining && ++scanned<6000) {
            const el = node as HTMLElement
            if (el.shadowRoot) roots.push(el.shadowRoot)
            if (el.matches("button,a[href],input:not([type=hidden]),textarea,select,[contenteditable=true],[role=button],[role=link],[role=checkbox],[role=textbox],[role=combobox],[role=tab],[role=menuitem]")) {
              const rect = el.getBoundingClientRect(), style = getComputedStyle(el)
              if (rect.width && rect.height && style.visibility !== "hidden" && style.display !== "none" && !el.closest("[data-neoism-browser-status]")) result.push(el)
            }
          }
        }
        return result
      },150-count)
      try {
        const handles = [...(await collection.getProperties()).values()].map(value=>value.asElement()).filter((value):value is ElementHandle<Element>=>value !== null) as ElementHandle<Element>[]
        const previous = [...this.entries.entries()].filter(([,entry])=>entry.frame === frame)
        const metadata = await frame.evaluate(({elements,old})=>elements.map(el=>({
          old:old.findIndex(item=>item === el),
          signature:[el.tagName,el.getAttribute("role"),el.getAttribute("type"),el.getAttribute("aria-label"),(el as HTMLInputElement).labels?.[0]?.textContent,el.textContent?.trim().slice(0,100)].join("|"),
          role:el.getAttribute("role") ?? el.tagName.toLowerCase(),
          label:(el.getAttribute("aria-label") || (el as HTMLInputElement).labels?.[0]?.textContent || el.textContent || el.getAttribute("placeholder") || "").trim().replace(/\s+/g," ").slice(0,140),
          disabled:(el as HTMLInputElement).disabled || el.getAttribute("aria-disabled") === "true",
          value:(el as HTMLInputElement).type === "password" ? "[redacted]" : ((el as HTMLInputElement).value ?? "").slice(0,100),
        })),{elements:handles,old:previous.map(([,entry])=>entry.handle)})
        for (let i=0;i<handles.length;i++) {
          const handle = handles[i]!, meta = metadata[i]!, before = previous[meta.old]
          const unchanged=before && before[1].signature === meta.signature
          const id = unchanged ? before[0] : `r${++this.sequence}`
          if (unchanged) await handle.dispose()
          else this.entries.set(id,{handle,signature:meta.signature,frame})
          found.add(id); count++
          lines.push(`[${id}] ${meta.role} ${JSON.stringify(meta.label)}${meta.disabled ? " disabled":""}${meta.value ? ` value=${JSON.stringify(meta.value)}`:""}${frame === this.page.mainFrame() ? "":` frame=${frame.url()}`}`)
        }
      } finally { await collection.dispose() }
    }
    for (const [id,entry] of this.entries) if (!found.has(id)) { this.entries.delete(id); await entry.handle.dispose().catch(()=>{}) }
    return lines.join("\n").slice(0,16000)
  }
}
