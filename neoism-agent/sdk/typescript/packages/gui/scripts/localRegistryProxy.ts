import { randomBytes } from 'node:crypto';
import { request as httpRequest, type IncomingMessage, type ServerResponse } from 'node:http';
import type { Plugin } from 'vite';
const PATH = '/__neoism/gui/servers';
const loopback = (host: string) => ['127.0.0.1', 'localhost', '[::1]', '::1', '::ffff:127.0.0.1'].includes(host);

// Native HTTP preserves Fetch Metadata set by this local Node process. Browser
// fetch cannot set those reserved headers; don't use an upstream redirecting client.
async function localRequest(url: URL, method: string, headers: Record<string, string>, body?: string): Promise<Response> {
    return new Promise((resolve, reject) => {
        const request = httpRequest(url, { method, headers }, response => {
            const chunks: Buffer[] = []; let length = 0;
            response.on('data', chunk => { length += chunk.length; if (length > 1024 * 1024) { request.destroy(new Error('response too large')); return; } chunks.push(chunk); });
            response.on('end', () => {
                const headers = new Headers();
                for (const [key, value] of Object.entries(response.headers)) if (value) headers.set(key, Array.isArray(value) ? value.join(', ') : value);
                resolve(new Response(Buffer.concat(chunks), { status: response.statusCode || 502, headers }));
            });
        });
        request.setTimeout(5000, () => request.destroy(new Error('local GUI unavailable')));
        request.on('error', reject); request.end(body);
    });
}
type Options = { target: string; port: () => number; agentToken?: string; upstream?: typeof localRequest };
export function localRegistryHandler({ target, port, agentToken, upstream = localRequest }: Options) {
    const browserSession = randomBytes(32).toString('hex');
    let issuedAt = 0;
    let upstreamCookie = '';
    return async (req: IncomingMessage, res: ServerResponse, next: () => void) => {
        const path = req.url?.split('?')[0] || '/';
        const host = req.headers.host || '';
        let origin = '';
        try {
            const parsed = new URL(`http://${host}`);
            if (loopback(parsed.hostname) && Number(parsed.port || 80) === port() && parsed.pathname === '/' && !parsed.username && !parsed.password) origin = parsed.origin;
        } catch { /* Reject invalid Host. */ }
        const local = !!origin && loopback(req.socket.remoteAddress || '') && !['forwarded', 'x-forwarded-for', 'x-forwarded-host', 'authorization'].some(key => req.headers[key]) && (!req.headers.origin || req.headers.origin === origin);
        const hasSession = req.headers.cookie?.split(';').some(c => c.trim() === `neoism_vite_gui=${browserSession}`) && Date.now() - issuedAt < 12 * 3600 * 1000;
        // Protect the dev GUI against a cross-site iframe manufacturing a
        // same-origin fetch context. Only a direct/user-initiated document boots it.
        res.setHeader('X-Frame-Options', 'DENY'); res.setHeader('Content-Security-Policy', "frame-ancestors 'none'");
        if (path !== PATH) {
            if (local && req.method === 'GET' && req.headers['sec-fetch-mode'] === 'navigate' && req.headers['sec-fetch-dest'] === 'document'
                && (req.headers['sec-fetch-site'] === 'none' || (req.headers['sec-fetch-site'] === 'same-origin' && hasSession))) {
                issuedAt = Date.now();
                res.setHeader('Set-Cookie', `neoism_vite_gui=${browserSession}; HttpOnly; SameSite=Strict; Path=/__neoism/gui; Max-Age=43200`);
                res.setHeader('Cache-Control', 'no-store');
            }
            next(); return;
        }
        res.setHeader('Cache-Control', 'no-store');
        if (!local || !hasSession || req.headers['sec-fetch-site'] !== 'same-origin' || req.headers['x-neoism-gui'] !== '1') { res.statusCode = 403; res.end(); return; }
        if (!['GET', 'POST', 'DELETE'].includes(req.method || '')) { res.statusCode = 405; res.end(); return; }
        try {
            const base = new URL(target);
            // A development registry always belongs to the local user. Never
            // bootstrap operator access on an arbitrary remote Agent target.
            if (base.protocol !== 'http:' || !['127.0.0.1', 'localhost', '[::1]'].includes(base.hostname) || base.username || base.password || base.pathname !== '/' || base.search || base.hash) throw new Error('nonlocal target');
            let body = '';
            for await (const chunk of req) { body += chunk.toString(); if (Buffer.byteLength(body) > 65536) { res.statusCode = 413; res.end(); return; } }
            const bootstrap = async () => {
                const response = await upstream(base, 'GET', { 'sec-fetch-site': 'none', 'sec-fetch-mode': 'navigate', 'sec-fetch-dest': 'document', accept: 'text/html', ...(agentToken ? { authorization: `Bearer ${agentToken}` } : {}) });
                if (!response.ok || response.headers.get('x-neoism-agent-gui') !== '1') throw new Error('old backend');
                const cookie = response.headers.get('set-cookie')?.match(/(?:^|,\s*)(neoism_local_gui=[a-f0-9]{64})(?:;|$)/)?.[1];
                if (!cookie) throw new Error('no local launch session');
                upstreamCookie = cookie;
            };
            if (!upstreamCookie) await bootstrap();
            const call = () => upstream(new URL(PATH, base), req.method || 'GET', { cookie: upstreamCookie, origin: base.origin, 'sec-fetch-site': 'same-origin', 'x-neoism-gui': '1', 'content-type': 'application/json' }, body || undefined);
            let response = await call();
            if (response.status === 403) { upstreamCookie = ''; await bootstrap(); response = await call(); }
            res.statusCode = response.status;
            res.setHeader('Content-Type', response.headers.get('content-type') || 'application/json');
            // No upstream cookies, credentials, or CORS headers enter the browser.
            res.end(await response.text());
        } catch { res.statusCode = 404; res.end(); }
    };
}
export function localRegistryProxy(): Plugin {
    return {
        name: 'neoism-local-registry',
        configureServer(server) {
            const handler = localRegistryHandler({
                agentToken: process.env.NEOISM_AGENT_TOKEN,
                target: process.env.NEOISM_AGENT_URL || process.env.VITE_NEOISM_AGENT_URL || 'http://127.0.0.1:4096',
                port: () => { const address = server.httpServer?.address(); return address && typeof address !== 'string' ? address.port : server.config.server.port || 5174; },
            });
            server.middlewares.use((req, res, next) => { void handler(req, res, next); });
        },
    };
}
