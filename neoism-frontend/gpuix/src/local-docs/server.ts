import { existsSync, lstatSync, realpathSync, statSync } from "node:fs";
import { dirname, extname, join, sep } from "node:path";
import { fileURLToPath } from "node:url";
import { createServer } from "node:http";

export const DOCS_PREFIX = "/api/docs/";
export const ONLINE_DOCS_URL = "https://neoism.ai/api/docs/";
const sourceRoot = fileURLToPath(new URL("../../../../vendor/neoism-docs/dist", import.meta.url));

/** Contents of docs dist are copied verbatim into resources/docs (not dist/). */
export function docsRoot(executable = process.execPath, developmentRoot = sourceRoot): string {
  const root = dirname(realpathSync(executable));
  const candidates = [join(root, "resources/docs"), join(root, "../Resources/resources/docs"), developmentRoot];
  for (const candidate of candidates) {
    if (existsSync(join(candidate, "api/docs/index.html"))) return realpathSync(candidate);
  }
  throw new Error("Local documentation is missing. Reinstall the complete Neoism bundle, or from the repository run: git submodule update --init vendor/neoism-docs && cd vendor/neoism-docs && npm ci && npm run build. Online documentation (explicit opt-in): " + ONLINE_DOCS_URL);
}

const types: Record<string, string> = {
  ".html": "text/html; charset=utf-8", ".css": "text/css; charset=utf-8",
  ".js": "text/javascript; charset=utf-8", ".mjs": "text/javascript; charset=utf-8",
  ".json": "application/json; charset=utf-8", ".svg": "image/svg+xml", ".png": "image/png",
  ".jpg": "image/jpeg", ".jpeg": "image/jpeg", ".webp": "image/webp", ".gif": "image/gif",
  ".ico": "image/x-icon", ".woff": "font/woff", ".woff2": "font/woff2", ".ttf": "font/ttf",
  ".txt": "text/plain; charset=utf-8", ".wasm": "application/wasm",
  ".xml": "application/xml; charset=utf-8",
};
const security = {
  "Content-Security-Policy": "default-src 'self'; script-src 'self' 'unsafe-inline' 'report-sample'; style-src 'self' 'unsafe-inline'; img-src 'self' data:; font-src 'self' data:; connect-src 'self'; object-src 'none'; base-uri 'none'; frame-ancestors 'none'; form-action 'none'",
  "X-Content-Type-Options": "nosniff", "Referrer-Policy": "no-referrer",
  "Cross-Origin-Resource-Policy": "same-origin", "Cache-Control": "no-store",
};

/** No daemon routes, credentials, directory listings, remote binds, or writes. */
export async function startDocsServer(directory = docsRoot()) {
  const root = realpathSync(directory);
  let origin = "";
  const handle = async (request: { method: string; headers: Headers; url: string }) => {
      const reply = (status: number, text = "", extra: Record<string, string> = {}) => new Response(request.method === "HEAD" ? null : text, { status, headers: { ...security, ...extra } });
      // Exact authority prevents DNS rebinding; this server advertises IPv4 only.
      if (request.headers.get("host") !== origin.slice("http://".length)) return reply(403);
      if (request.headers.has("origin") && request.headers.get("origin") !== origin) return reply(403);
      if (request.method !== "GET" && request.method !== "HEAD") return reply(405, "Method not allowed", { Allow: "GET, HEAD" });
      let path: string;
      // Do not use URL.pathname here: WHATWG URL parsing normalizes dot
      // segments before we can reject encoded or literal traversal attempts.
      try { path = decodeURIComponent(request.url.replace(/^https?:\/\/[^/]+/, "").split(/[?#]/, 1)[0]!); } catch { return reply(400); }
      if (path.includes("\\") || path.includes("\0") || path.includes("%") || path.split("/").some(part => part === "." || part === "..")) return reply(403);
      if (path === "/api/docs") return reply(308, "", { Location: DOCS_PREFIX });
      const docsPath = path.startsWith(DOCS_PREFIX);
      // vendor/neoism-docs/CONTRACT.md: the dedicated artifact root, never the
      // marketing site or a daemon/web-client root. Do not guess legacy layouts.
      const sharedAsset = path.startsWith("/docs-assets/") || path.startsWith("/legal/") ||
        ["/", "/index.html", "/reference/openapi.json", "/content.json", "/manifest.json", "/artifact.json", "/LICENSE", "/NOTICE", "/favicon.svg", "/geist-LICENSE.txt", "/sitemap.xml"].includes(path);
      if (!docsPath && !sharedAsset) return reply(404);
      const relative = path.slice(1);
      const parts = relative.split("/").filter(Boolean);
      if (parts.some(part => part.startsWith("."))) return reply(403);
      try {
        let file = root;
        // Reject symlinks even when their target happens to remain within root.
        for (const part of parts) {
          file = join(file, part);
          if (lstatSync(file).isSymbolicLink()) return reply(403);
        }
        if (statSync(file).isDirectory()) {
          if (!path.endsWith("/")) return reply(308, "", { Location: path + "/" });
          file = join(file, "index.html");
          if (lstatSync(file).isSymbolicLink()) return reply(403);
        }
        if (!realpathSync(file).startsWith(root + sep) || !statSync(file).isFile()) return reply(403);
        const extension = extname(file).toLowerCase();
        const type = types[extension] || (!extension && (path.startsWith("/legal/") || path === "/LICENSE" || path === "/NOTICE") ? "text/plain; charset=utf-8" : undefined);
        if (!type) return reply(404);
        const asset = Bun.file(file);
        return new Response(request.method === "HEAD" ? null : asset, { headers: { ...security, "Content-Type": type, "Content-Length": String(asset.size) } });
      } catch { return reply(404); }
  };
  // IncomingMessage.url retains raw dot segments. Bun.serve/Request normalizes
  // them before dispatch, which prevents an explicit traversal rejection.
  const server = createServer(async (incoming, outgoing) => {
    try {
      const headers = new Headers();
      for (const [name, value] of Object.entries(incoming.headers)) if (value !== undefined) headers.set(name, Array.isArray(value) ? value.join(",") : value);
      const response = await handle({ method: incoming.method || "", headers, url: incoming.url || "/" });
      const responseHeaders: Record<string, string> = {};
      response.headers.forEach((value, name) => { responseHeaders[name] = value; });
      outgoing.writeHead(response.status, responseHeaders);
      if (response.body) {
        const reader = response.body.getReader();
        try {
          while (!outgoing.destroyed) {
            const chunk = await reader.read();
            if (chunk.done) break;
            if (!outgoing.write(chunk.value)) await new Promise<void>(resolve => {
              const ready = () => { outgoing.off("drain", ready); outgoing.off("close", ready); resolve(); };
              outgoing.once("drain", ready); outgoing.once("close", ready);
            });
          }
        } finally { await reader.cancel(); }
      }
      outgoing.end();
    } catch { outgoing.destroy(); }
  });
  await new Promise<void>((resolve, reject) => {
    server.once("error", reject);
    server.listen(0, "127.0.0.1", () => { server.off("error", reject); resolve(); });
  });
  const address = server.address();
  if (!address || typeof address === "string") throw new Error("Could not bind local documentation server");
  origin = `http://127.0.0.1:${address.port}`;
  return { url: `${origin}${DOCS_PREFIX}`, close: () => { server.closeAllConnections(); server.close(); }, unref: () => server.unref() };
}

type DocsServer = Awaited<ReturnType<typeof startDocsServer>>;
const key = Symbol.for("neoism/local-docs-server");
const globals = globalThis as typeof globalThis & { [key]?: Promise<DocsServer> };
/** Survives UI/window remounts; unref means it cannot hold the app open. */
export async function localDocsUrl() {
  if (!globals[key]) {
    globals[key] = startDocsServer().then(instance => {
      instance.unref();
      process.once("exit", () => instance.close());
      return instance;
    }).catch(error => { delete globals[key]; throw error; });
  }
  return (await globals[key]!).url;
}