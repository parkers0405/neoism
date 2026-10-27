import type { Page } from "playwright-core"
const identities = new WeakMap<Page, () => Promise<string>>()
export function registerPageTargetId(page: Page, resolve: () => Promise<string>): void { identities.set(page, resolve) }
export async function pageTargetId(page: Page): Promise<string> {
  const resolve = identities.get(page)
  if (!resolve) throw new Error("This page has no Neoism browser identity")
  return resolve()
}
