import { afterEach, expect, test } from "bun:test";
import { mkdtempSync, mkdirSync, writeFileSync, symlinkSync, rmSync, realpathSync } from "node:fs";
import { join } from "node:path";
import { tmpdir } from "node:os";
import { connect } from "node:net";
import { docsRoot, startDocsServer } from "../src/local-docs/server";
import { docsArguments, runDocs } from "../src/local-docs/cli";

const cleanups: (() => void)[] = [];
afterEach(() => { for (const cleanup of cleanups.splice(0).reverse()) cleanup(); });
async function fixture() {
  const root = mkdtempSync(join(tmpdir(), "neoism-docs-"));
  cleanups.push(() => rmSync(root, { recursive: true, force: true }));
  mkdirSync(join(root, "api/docs/reference"), { recursive: true });
  mkdirSync(join(root, "reference")); mkdirSync(join(root, "docs-assets")); mkdirSync(join(root, "legal"));
  writeFileSync(join(root, "api/docs/index.html"), "<h1>Handbook</h1>");
  writeFileSync(join(root, "index.html"), '<a href="/api/docs/">Documentation</a>');
  writeFileSync(join(root, "api/docs/reference/index.html"), "<h1>Scalar</h1>");
  writeFileSync(join(root, "reference/openapi.json"), '{"openapi":"3.1.0"}');
  writeFileSync(join(root, "content.json"), '{"pages":[]}');
  writeFileSync(join(root, "manifest.json"), '{"schemaVersion":1}');
  writeFileSync(join(root, "artifact.json"), '{"schemaVersion":1,"files":{}}');
  writeFileSync(join(root, "docs-assets/app.js"), "console.log('local')");
  writeFileSync(join(root, "docs-assets/font.woff2"), "font fixture");
  writeFileSync(join(root, "legal/neoism-LICENSE"), "MIT fixture");
  writeFileSync(join(root, "LICENSE"), "MIT fixture");
  writeFileSync(join(root, "NOTICE"), "Notice fixture");
  writeFileSync(join(root, ".secret"), "secret");
  const server = await startDocsServer(root); cleanups.push(() => server.close());
  return { root, server, origin: new URL(server.url).origin };
}
// Raw TCP avoids normalization in both fetch and Bun's node:http client.
function raw(origin: string, path: string, method = "GET", headers: Record<string,string> = {}) {
  return new Promise<{ status: number; body: string }>((resolve, reject) => {
    const url = new URL(origin);
    const socket = connect(Number(url.port), url.hostname, () => {
      const fields = { Host: url.host, Connection: "close", ...headers };
      socket.write(`${method} ${path} HTTP/1.1\r\n${Object.entries(fields).map(([name,value]) => `${name}: ${value}`).join("\r\n")}\r\n\r\n`);
    });
    let response = "";
    socket.on("data", chunk => { response += chunk; }); socket.on("error", reject);
    socket.on("end", () => resolve({ status: Number(response.split(" ")[1]), body: response.split("\r\n\r\n").slice(1).join("\r\n\r\n") }));
  });
}
test("handbook, Scalar, machine content, assets, HEAD and redirects use real loopback HTTP", async () => {
  const { server, origin } = await fixture();
  expect(new URL(server.url).hostname).toBe("127.0.0.1");
  expect(await (await fetch(server.url)).text()).toContain("Handbook");
  expect(await (await fetch(server.url + "reference/")).text()).toContain("Scalar");
  expect(await (await fetch(origin + "/content.json")).json()).toEqual({ pages: [] });
  const asset = await fetch(origin + "/docs-assets/app.js");
  expect(asset.headers.get("content-type")).toContain("text/javascript");
  expect(asset.headers.get("content-security-policy")).toContain("connect-src 'self'");
  expect(asset.headers.get("access-control-allow-origin")).toBeNull();
  const head = await fetch(server.url, { method: "HEAD" });
  expect(head.status).toBe(200); expect(await head.text()).toBe("");
  expect(head.headers.get("content-length")).toBe("17");
  expect((await raw(origin, "/api/docs")).status).toBe(308);
  expect((await raw(origin, "/api/docs/reference")).status).toBe(308);
});
test("bounded static-only access, methods, Host and Origin guards", async () => {
  const { origin } = await fixture();
  for (const path of ["/api/docs/../content.json", "/api/docs/%2e%2e/content.json", "/api/docs/%252e%252e/content.json", "/api/docs/assets%5c..%5c.secret", "/api/docs/%00", "/api/docs/.secret"]) {
    expect((await raw(origin, path)).status).not.toBe(200);
  }
  expect((await raw(origin, "/api/docs/assets/../index.html")).status).not.toBe(200);
  expect((await raw(origin, "/api/docs/assets/%2e%2e/index.html")).status).not.toBe(200);
  for (const path of ["/session", "/api/auth", "/api/docs/missing", "/api/docs/assets/"]) expect((await raw(origin, path)).status).toBe(404);
  for (const method of ["POST", "PUT", "DELETE", "OPTIONS"]) expect((await raw(origin, "/api/docs/", method)).status).toBe(405);
  expect((await raw(origin, "/api/docs/", "GET", { Host: "evil.example" })).status).toBe(403);
  expect((await raw(origin, "/api/docs/", "GET", { Origin: "https://evil.example" })).status).toBe(403);
});
test("contract shared assets, original OpenAPI, metadata and local legal notices are readable", async () => {
  const { origin } = await fixture();
  for (const path of ["/docs-assets/font.woff2", "/reference/openapi.json", "/content.json", "/manifest.json", "/artifact.json", "/LICENSE", "/NOTICE", "/legal/neoism-LICENSE"]) expect((await fetch(origin + path)).status).toBe(200);
  expect((await fetch(origin + "/legal/neoism-LICENSE")).headers.get("content-type")).toContain("text/plain");
  for (const path of ["/assets/app.js", "/reference/index.html", "/reference/other.json", "/content/Start Here.md"]) expect((await fetch(origin + path)).status).toBe(404);
  expect((await fetch(origin + "/")).status).toBe(200);
});
test("rejects symlinks to outside files and directories", () => {
  if (process.platform === "win32") return; // Requires symlink privileges on Windows.
  return (async () => {
    const { root, origin } = await fixture();
    symlinkSync(join(root, "content.json"), join(root, "api/docs/alias.json"));
    symlinkSync(tmpdir(), join(root, "api/docs/escape"));
    expect((await raw(origin, "/api/docs/alias.json")).status).toBe(403);
    expect((await raw(origin, "/api/docs/escape/")).status).toBe(403);
  })();
});
test("root lookup uses installed resources before developer fallback, with actionable missing error", async () => {
  const { root } = await fixture();
  const executable = join(root, "neoism"); writeFileSync(executable, "");
  // docsRoot returns canonical paths (on macOS /tmp can resolve to /private/tmp).
  expect(docsRoot(executable, root)).toBe(realpathSync(root));
  mkdirSync(join(root, "resources/docs/api/docs"), { recursive: true });
  writeFileSync(join(root, "resources/docs/api/docs/index.html"), "installed");
  expect(docsRoot(executable, root)).toBe(realpathSync(join(root, "resources/docs")));
  rmSync(join(root, "resources"), { recursive: true });
  mkdirSync(join(root, "resources/docs"), { recursive: true });
  writeFileSync(join(root, "resources/docs/index.html"), "old speculative layout");
  expect(docsRoot(executable, root)).toBe(realpathSync(root));
  expect(() => docsRoot(executable, "/missing")).toThrow("npm run build");
});
test("root lookup canonicalizes symlinked developer paths and macOS bundle executables", async () => {
  if (process.platform === "win32") return; // Requires symlink privileges on Windows.
  const { root } = await fixture();
  const alias = join(root, "developer-docs");
  symlinkSync(root, alias);
  const executable = join(root, "Neoism.app/Contents/MacOS/neoism");
  mkdirSync(join(root, "Neoism.app/Contents/MacOS"), { recursive: true });
  writeFileSync(executable, "");
  const launcher = join(root, "neoism");
  symlinkSync(executable, launcher);
  expect(docsRoot(launcher, alias)).toBe(realpathSync(root));
  const installed = join(root, "Neoism.app/Contents/Resources/resources/docs");
  mkdirSync(join(installed, "api/docs"), { recursive: true });
  writeFileSync(join(installed, "api/docs/index.html"), "installed bundle");
  expect(docsRoot(launcher, alias)).toBe(realpathSync(installed));
});
test("docs routing is first-argument only, without consuming legacy options", async () => {
  expect(docsArguments(["docs"])).toEqual([]);
  expect(docsArguments(["docs", "--help"])).toEqual(["--help"]);
  for (const args of [["--workspace", "docs"], ["update"], ["--help"], []]) expect(docsArguments(args)).toBeUndefined();
  // Entrypoint help must never load GPUIX/native or launch a browser/window.
  const child = Bun.spawn([process.execPath, "src/cli.ts", "docs", "--help"], { stdout: "pipe", stderr: "pipe" });
  expect(await child.exited).toBe(0);
  expect(await new Response(child.stdout).text()).toContain("Ctrl+C");
  const invalid = Bun.spawn([process.execPath, "src/cli.ts", "docs", "--invalid"], { stdout: "pipe", stderr: "pipe" });
  expect(await invalid.exited).toBe(1);
  expect(await new Response(invalid.stderr).text()).toContain("Usage: neoism docs");
});
test("CLI stays alive after browser opens until Ctrl+C and closes on browser failure", async () => {
  const { root } = await fixture();
  let opened = "", closed = false;
  const start = async () => {
    const server = await startDocsServer(root);
    cleanups.push(() => server.close());
    return { ...server, close: () => { closed = true; return server.close(); } };
  };
  const running = runDocs([], async url => { opened = url; }, start);
  await Bun.sleep(5);
  expect(closed).toBe(false);
  expect((await fetch(opened)).status).toBe(200);
  process.emit("SIGINT");
  await running;
  expect(closed).toBe(true);
  closed = false;
  await expect(runDocs([], async () => { throw Error("browser failed"); }, start)).rejects.toThrow("browser failed");
  expect(closed).toBe(true);
});