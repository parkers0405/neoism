export type Engine = "chrome" | "firefox"
export interface Tab {
  tabId: number
  targetId: string
  url: string
  title: string
  owned: boolean
  marker?: string
}
export interface WireMessage {
  type: string
  id?: number
  method?: string
  params?: Record<string, unknown>
  result?: unknown
  error?: string
  [key: string]: unknown
}
export const enginePort = (engine: Engine): number => engine === "chrome" ? 19990 : 19991
export function selectedEngine(): Engine { return process.env.NEOISM_BROWSER_ENGINE === "firefox" ? "firefox" : "chrome" }
export function safeHttpUrl(value: unknown): string {
  if (typeof value !== "string") throw new Error("An exact URL is required")
  const url = new URL(value)
  if (!["http:", "https:"].includes(url.protocol) || url.username || url.password) throw new Error("Use an HTTP(S) URL without embedded credentials")
  return url.href
}
export function shortError(value: unknown): string { return (value instanceof Error ? value.message : String(value)).slice(0, 2000) }
