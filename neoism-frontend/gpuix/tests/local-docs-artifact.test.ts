import { expect, test } from "bun:test";
import { createRequire } from "node:module";
import { createHash } from "node:crypto";
import { fileURLToPath } from "node:url";
import { startDocsServer } from "../src/local-docs/server";

// Optional integration: reuse a checkout with Playwright installed rather than
// adding browser tooling to the native app. Never builds or changes the dist.
// NEOISM_DOCS_PLAYWRIGHT_PACKAGE=/path/to/website/package.json bun test tests/local-docs-artifact.test.ts
test.skipIf(!process.env.NEOISM_DOCS_PLAYWRIGHT_PACKAGE)("real shared artifact: all files, handbook navigation/search and mounted Scalar stay offline under native CSP", async () => {
  const root = fileURLToPath(new URL("../../../vendor/neoism-docs/dist/", import.meta.url));
  const artifact = await Bun.file(root + "artifact.json").json() as { schemaVersion: number; version: string; files: Record<string, { bytes: number; sha256: string }> };
  expect(artifact.schemaVersion).toBe(1);
  const server = await startDocsServer(root);
  const origin = new URL(server.url).origin;
  let browser: any;
  try {
    // Every inventoried file must be available unmodified through our allowlist,
    // including lazy JS chunks, fonts and extensionless third-party notices.
    for (const [path, entry] of Object.entries(artifact.files)) {
      const response = await fetch(origin + "/" + path.split("/").map(encodeURIComponent).join("/"));
      expect(response.status, path).toBe(200);
      const bytes = new Uint8Array(await response.arrayBuffer());
      expect(bytes.byteLength, path).toBe(entry.bytes);
      expect(createHash("sha256").update(bytes).digest("hex"), path).toBe(entry.sha256);
    }
    const require = createRequire(process.env.NEOISM_DOCS_PLAYWRIGHT_PACKAGE!);
    const { chromium } = require("@playwright/test");
    browser = await chromium.launch({ headless: true, ...(process.env.NEOISM_DOCS_CHROMIUM ? { executablePath: process.env.NEOISM_DOCS_CHROMIUM } : {}) });
    const context = await browser.newContext();
    const external: string[] = [], failures: string[] = [], errors: string[] = [];
    await context.route("**/*", async (route: any) => {
      const url = route.request().url();
      if (new URL(url).origin !== origin) { external.push(url); await route.abort(); }
      else await route.continue();
    });
    const page = await context.newPage();
    page.on("pageerror", (error: Error) => errors.push(error.message));
    page.on("console", (message: any) => { if (message.type() === "error") errors.push(message.text()); });
    page.on("response", (response: any) => { if (response.status() >= 400) failures.push(`${response.status()} ${response.url()}`); });
    page.on("requestfailed", (request: any) => failures.push(`${request.failure()?.errorText} ${request.url()}`));
    await page.addInitScript(() => {
      (window as any).__docsCspViolations = [];
      document.addEventListener("securitypolicyviolation", event => {
        (window as any).__docsCspViolations.push(`${event.violatedDirective}: ${event.blockedURI} at ${event.sourceFile}:${event.lineNumber}:${event.columnNumber} ${event.sample}`);
      });
    });
    const csp: string[] = [];
    const manifest = await (await fetch(origin + "/manifest.json")).json() as { pages: { path: string; title: string }[] };
    const inspect = async () => { csp.push(...await page.evaluate(() => (window as any).__docsCspViolations)); };
    await page.goto(server.url, { waitUntil: "networkidle" });
    await page.locator(".guide-no-results").waitFor({ state: "attached" }); // enhancement loaded
    expect(await page.locator(".guide-main h1").innerText()).toContain(manifest.pages[0]!.title);
    const links = page.locator(".guide-navigation a[data-guide-path]");
    const count = await links.count();
    expect(count).toBeGreaterThan(10);
    await page.locator(".guide-search").fill("no-such-guide-neoism-test-999");
    expect(await page.locator(".guide-navigation a[data-guide-path]:not([hidden])").count()).toBe(0);
    await page.locator(".guide-no-results").waitFor({ state: "visible" });
    await page.locator(".guide-search").fill("JSON Render");
    const found = page.locator(".guide-navigation a[data-guide-path]:not([hidden])");
    expect(await found.count()).toBeGreaterThan(0);
    expect(await found.count()).toBeLessThan(count);
    const guide = page.locator('.guide-navigation a[data-guide-path="Neoism Agent/JSON Render.md"]');
    await inspect();
    await guide.click();
    await page.waitForURL("**/api/docs/guides/neoism-agent/json-render/");
    await page.waitForLoadState("networkidle");
    expect(await page.locator(".guide-main h1").innerText()).toContain(manifest.pages.find(page => page.path === "Neoism Agent/JSON Render.md")!.title);
    await inspect();
    await page.locator("#reference-tab").click();
    await page.locator('.scalar-scope[data-ready="true"]').waitFor({ timeout: 60000 });
    await page.waitForLoadState("networkidle");
    expect(await page.locator(".reference-fallback").isHidden()).toBe(true);
    expect(await page.locator(".scalar-scope").innerText()).toContain("Neoism Agent API");
    // Assert a real versioned operation appears, not just an empty mount/title.
    const spec = await (await fetch(origin + "/reference/openapi.json")).json() as { paths: Record<string, any> };
    const endpoint = Object.keys(spec.paths).find(path => path === "/session") || Object.keys(spec.paths)[0]!;
    expect(await page.locator(".scalar-scope").innerText()).toContain(endpoint);
    await inspect();
    expect(external, "external runtime requests").toEqual([]);
    expect(failures, "failed runtime requests").toEqual([]);
    expect(errors, "console/page errors").toEqual([]);
    expect(csp, "CSP violations").toEqual([]);
    console.log(`Shared docs ${artifact.version}: ${Object.keys(artifact.files).length} inventoried files verified; handbook/search/Scalar passed with no external requests or CSP errors.`);
  } finally { await browser?.close(); server.close(); }
}, 120000);