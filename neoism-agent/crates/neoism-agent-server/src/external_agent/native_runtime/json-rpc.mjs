import { spawn } from 'node:child_process';
import { createInterface } from 'node:readline';
import { isAbsolute } from 'node:path';

// Codex's stdio protocol is JSON-RPC-shaped NDJSON (no Content-Length framing).
export function spawnJsonRpc({ executable, cwd, args = ['app-server', '--stdio'] }) {
  if (!executable || !isAbsolute(executable)) throw new Error('Codex requires an absolute installed executable');
  const grouped = process.platform !== 'win32';
  const child = spawn(executable, args, { cwd, env: process.env, stdio: ['pipe', 'pipe', 'pipe'], detached: grouped });
  let sequence = 0, dead = false, closing;
  let stderr = '';
  const pending = new Map(), notifications = new Set(), failures = new Set();
  const write = value => {
    if (dead) throw new Error('Codex app-server connection closed');
    child.stdin.write(JSON.stringify(value) + '\n');
  };
  function fail(error) {
    if (dead) return;
    dead = true;
    for (const { reject, timer } of pending.values()) { clearTimeout(timer); reject(error); }
    pending.clear();
    for (const callback of failures) callback(error);
  }
  child.stderr.on('data', chunk => { stderr = (stderr + chunk).slice(-8192); });
  child.on('error', fail);
  child.stdin.on('error', fail);
  const exited = new Promise(resolve => child.once('close', (code, signal) => {
    fail(new Error(`Codex app-server exited (${signal ?? code})${stderr ? ': ' + stderr : ''}`));
    resolve();
  }));
  let handleRequest = async method => { throw Object.assign(new Error(`Unsupported Codex request: ${method}`), { code: -32601 }); };
  const lines = createInterface({ input: child.stdout, crlfDelay: Infinity });
  lines.on('line', line => {
    if (!line.trim()) return;
    let message;
    try { message = JSON.parse(line); } catch { fail(new Error('Invalid JSON from Codex app-server')); return; }
    if (message.method && Object.hasOwn(message, 'id')) {
      Promise.resolve().then(() => handleRequest(message.method, message.params ?? {}, message.id)).then(
        result => { if (!dead) write({ id: message.id, result }); },
        error => { if (!dead) write({ id: message.id, error: { code: typeof error.code === 'number' ? error.code : -32603, message: error.message ?? String(error) } }); },
      ).catch(fail);
    } else if (message.method) {
      try { for (const callback of notifications) callback(message.method, message.params ?? {}); } catch (error) { fail(error); }
    } else if (Object.hasOwn(message, 'id')) {
      const waiter = pending.get(message.id);
      if (!waiter) return;
      pending.delete(message.id); clearTimeout(waiter.timer);
      if (message.error) waiter.reject(Object.assign(new Error(message.error.message), { code: message.error.code, details: message.error.data }));
      else waiter.resolve(message.result);
    }
  });
  return {
    request(method, params = {}) {
      return new Promise((resolve, reject) => {
        const id = ++sequence;
        const timer = setTimeout(() => { pending.delete(id); reject(new Error(`Codex request timed out: ${method}`)); }, 60000);
        pending.set(id, { resolve, reject, timer });
        try { write({ id, method, params }); } catch (error) { pending.delete(id); clearTimeout(timer); reject(error); }
      });
    },
    notify(method, params) { write({ method, ...(params === undefined ? {} : { params }) }); },
    onNotification(callback) { notifications.add(callback); return () => notifications.delete(callback); },
    onFailure(callback) { failures.add(callback); return () => failures.delete(callback); },
    setRequestHandler(callback) { handleRequest = callback; },
    close() {
      if (closing) return closing;
      closing = (async () => {
        child.stdin.end();
        const kill = signal => { try { if (grouped && child.pid) process.kill(-child.pid, signal); else child.kill(signal); } catch {} };
        const term = setTimeout(() => kill('SIGTERM'), 250);
        const hard = setTimeout(() => kill('SIGKILL'), 1500);
        await exited;
        clearTimeout(term); clearTimeout(hard); lines.close();
        // A server may exit before children; terminate the owned process group too.
        if (grouped) kill('SIGKILL');
      })();
      return closing;
    },
  };
}