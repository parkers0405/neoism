import { createAdapter as codex } from './codex.mjs';
import { createAdapter as claude } from './claude.mjs';
import { createAdapter as opencode } from './opencode.mjs';
import { createRequire } from 'node:module';
import { readFile, open, unlink, realpath, access } from 'node:fs/promises';
import { isAbsolute, join, relative, sep, resolve } from 'node:path';
import { pathToFileURL } from 'node:url';
import { spawn } from 'node:child_process';
import { createInterface } from 'node:readline';
import { setTimeout as delay } from 'node:timers/promises';
import { format } from 'node:util';

const SDK = '@anthropic-ai/claude-agent-sdk';
const SDK_VERSION = '0.2.76';
const factories = { codex, claude, opencode };
const rpcError = (message, code = -32000, details) => Object.assign(new Error(message), { code, details });
const inside = (root, path) => { const p = relative(root, path); return p !== '..' && !p.startsWith(`..${sep}`) && !isAbsolute(p); };
const object = value => value !== null && typeof value === 'object' && !Array.isArray(value);

async function installedSdk(root) {
  const packageDir = join(root, 'node_modules', '@anthropic-ai', 'claude-agent-sdk');
  try {
    const actualDir = await realpath(packageDir);
    if (!inside(root, actualDir)) throw new Error('Claude SDK escapes its private dependency cache');
    const metadata = JSON.parse(await readFile(join(packageDir, 'package.json'), 'utf8'));
    if (metadata.name !== SDK || metadata.version !== SDK_VERSION) throw new Error(`Managed Claude SDK must be ${SDK}@${SDK_VERSION}`);
    const require = createRequire(join(root, 'package.json'));
    const entry = await realpath(require.resolve(SDK));
    if (!inside(actualDir, entry)) throw new Error('Claude SDK resolved outside its managed package');
    await access(entry);
    return entry;
  } catch (error) {
    if (error.code === 'ENOENT' || error.code === 'MODULE_NOT_FOUND') return null;
    throw error;
  }
}

async function installSdk(npm, root, signal) {
  if (!isAbsolute(npm)) throw new Error('Managed npm executable must be an absolute path');
  if (signal.aborted) throw new Error('Claude SDK installation cancelled');
  await new Promise((resolveInstall, reject) => {
    const child = spawn(npm, ['ci', '--ignore-scripts', '--no-audit', '--no-fund', '--omit=dev'], {
      cwd: root, env: process.env, stdio: ['ignore', 'pipe', 'pipe'], shell: false, windowsHide: true,
    });
    let diagnostic = '', failure, hardKill;
    const collect = chunk => { diagnostic = (diagnostic + chunk.toString()).slice(-8192); };
    child.stdout.on('data', collect); child.stderr.on('data', collect);
    const stop = message => {
      failure ??= new Error(message);
      child.kill('SIGTERM');
      hardKill ??= setTimeout(() => child.kill('SIGKILL'), 2000);
    };
    const abort = () => stop('Claude SDK installation cancelled');
    const timeout = setTimeout(() => stop('Claude SDK npm ci timed out'), 120000);
    signal.addEventListener('abort', abort, { once: true });
    child.once('error', error => { failure ??= error; });
    child.once('close', code => {
      clearTimeout(timeout); clearTimeout(hardKill); signal.removeEventListener('abort', abort);
      if (failure || code !== 0) reject(new Error(`${failure?.message ?? `Claude SDK npm ci failed (${code})`}${diagnostic ? `: ${diagnostic}` : ''}`));
      else resolveInstall();
    });
    if (signal.aborted) abort();
  });
}

// Rust supplies this trusted private cache, including its manifest and lockfile.
// Resolution is deliberately anchored here, never at cwd, NODE_PATH, or this host.
async function managedSdk({ dependencyRoot, npm }, signal) {
  if (!dependencyRoot || !isAbsolute(dependencyRoot)) throw new Error('Claude requires an absolute private dependencyRoot');
  const root = await realpath(dependencyRoot);
  const manifest = JSON.parse(await readFile(join(root, 'package.json'), 'utf8'));
  const lock = JSON.parse(await readFile(join(root, 'package-lock.json'), 'utf8'));
  if (manifest.private !== true || manifest.dependencies?.[SDK] !== SDK_VERSION ||
      lock.packages?.['node_modules/' + SDK]?.version !== SDK_VERSION || lock.packages?.['']?.dependencies?.[SDK] !== SDK_VERSION) {
    throw new Error(`Private runtime manifest and lock must pin ${SDK}@${SDK_VERSION}`);
  }
  // Even readers take the lease: a partially extracted package is not an
  // installed SDK while another host's npm ci is still replacing node_modules.
  const lockPath = join(root, '.claude-sdk-install.lock');
  const deadline = Date.now() + 180000;
  let lease;
  while (!lease) {
    if (signal.aborted) throw new Error('Claude SDK installation cancelled');
    try { lease = await open(lockPath, 'wx', 0o600); }
    catch (error) {
      if (error.code !== 'EEXIST') throw error;
      if (Date.now() >= deadline) throw new Error('Timed out waiting for private Claude SDK install lock; check for an interrupted installer');
      await delay(100, undefined, { signal });
    }
  }
  try {
    let entry = await installedSdk(root);
    if (!entry) {
      if (!npm) throw new Error(`Managed ${SDK}@${SDK_VERSION} is missing; provide an absolute npm executable to install it`);
      await installSdk(npm, root, signal);
      entry = await installedSdk(root);
    }
    if (!entry) throw new Error('npm ci completed without installing the pinned Claude SDK');
    if (signal.aborted) throw new Error('Claude initialization cancelled');
    return await import(pathToFileURL(entry).href);
  } finally { await lease.close(); await unlink(lockPath); }
}

/** Object output is useful for embedders/tests; write receives serialized NDJSON.
 * receive/handleLine run independently: callers must not serialize incoming RPCs.
 * drain waits for queued output, NOT running turns or warm background work.
 */
export function createHost({ output, write, adapters = factories, sdk, loadSdk = managedSdk, requestTimeoutMs = 120000 } = {}) {
  if (!output && !write) throw new Error('Native host requires an output or NDJSON write callback');
  const sink = output ?? (message => write(JSON.stringify(message) + '\n'));
  let tail = Promise.resolve(), outputError, adapter, metadata, initializing, closePromise;
  let initialized = false, closed = false, sequence = 0;
  const pending = new Map(), inbound = new Set(), abort = new AbortController();
  let rejectClosed;
  const stopped = new Promise((_, reject) => { rejectClosed = reject; });
  stopped.catch(() => {});
  function send(message) {
    tail = tail.then(() => {
      if (outputError) throw outputError;
      return sink(message);
    }).catch(error => { outputError ??= error; });
    return tail;
  }
  async function drain() { await tail; if (outputError) throw outputError; }
  function notify(event, operationId) {
    if (closed) return;
    send({ jsonrpc: '2.0', method: 'runtime.event', params: { ...(operationId === undefined ? {} : { operationId }), event } });
  }
  function request(method, params, operationId) {
    if (closed) return Promise.reject(rpcError('Native runtime is closed'));
    if (!['permission.request', 'question.request'].includes(method)) return Promise.reject(rpcError(`Unsupported native interaction: ${method}`, -32601));
    if (operationId === undefined) return Promise.reject(rpcError('Native interaction requires a turn-bound operationId'));
    return new Promise((resolveReply, reject) => {
      const id = `neoism-host:interaction:${++sequence}`;
      const timer = setTimeout(() => {
        pending.delete(id); reject(rpcError(`Native interaction timed out: ${method}`, -32001));
      }, requestTimeoutMs);
      pending.set(id, { resolve: resolveReply, reject, timer });
      send({ jsonrpc: '2.0', id, method, params: { ...params, operationId } }).then(() => {
        if (outputError && pending.has(id)) {
          pending.delete(id); clearTimeout(timer); reject(outputError);
        }
      });
    });
  }
  async function close() {
    if (closePromise) return closePromise;
    closed = true;
    const error = rpcError('Native runtime closed');
    abort.abort(); rejectClosed(error);
    for (const waiter of pending.values()) { clearTimeout(waiter.timer); waiter.reject(error); }
    pending.clear();
    closePromise = (async () => {
      if (initializing) await initializing.catch(() => {});
      if (adapter) await adapter.close();
    })();
    return closePromise;
  }
  async function initialize(params, id) {
    if (initialized || initializing) throw rpcError('Native runtime is already initialized');
    if (closed) throw rpcError('Native runtime is closed');
    const factory = Object.hasOwn(adapters, params.provider) ? adapters[params.provider] : undefined;
    if (typeof factory !== 'function') throw rpcError(`Unsupported native provider: ${params.provider}`, -32602);
    if (typeof params.cwd !== 'string' || !isAbsolute(params.cwd)) throw rpcError('Native cwd must be absolute', -32602);
    if (params.executable != null && (typeof params.executable !== 'string' || !isAbsolute(params.executable))) throw rpcError('Native executable must be absolute', -32602);
    if (params.provider !== 'claude' && !params.executable) throw rpcError('Installed native executable is required', -32602);
    initializing = (async () => {
      const injected = params.provider === 'claude' ? sdk ?? await Promise.race([loadSdk(params, abort.signal), stopped]) : undefined;
      if (closed) throw rpcError('Native runtime is closed');
      adapter = await factory({ cwd: params.cwd, executable: params.executable ?? null, sdk: injected,
        policy: params.policy, preview: params.preview,
        emit: event => notify(event, id === undefined ? undefined : String(id)), request: (method, values) => request(method, values) });
      if (closed) { await adapter.close(); adapter = undefined; throw rpcError('Native runtime is closed'); }
      metadata = await Promise.race([adapter.describe(), stopped]);
      initialized = true;
      return metadata;
    })();
    try { return await initializing; }
    catch (error) {
      if (adapter && !closed) { await adapter.close().catch(() => {}); adapter = undefined; }
      throw error;
    } finally { initializing = undefined; }
  }
  async function dispatch(method, params, id) {
    if (method === 'runtime.initialize') return initialize(params, id);
    if (method === 'runtime.close') { await close(); return { closed: true }; }
    if (closed) throw rpcError('Native runtime is closed');
    if (initializing) await initializing;
    if (!initialized || !adapter) throw rpcError('Native runtime is not initialized');
    const unsupported = name => { throw rpcError(`Native adapter does not support ${name}`, -32601); };
    const call = (name, value) => {
      if (typeof adapter[name] !== 'function') return unsupported(name);
      return Promise.race([adapter[name](value), stopped]);
    };
    const capabilities = metadata?.capabilities ?? {};
    switch (method) {
      case 'runtime.describe': metadata = await call('describe', params); return metadata;
      case 'conversation.open': {
        if (params.sessionId && !capabilities.resume) unsupported('resume');
        const result = await call('open', params);
        if (params.sessionId && result.sessionId !== params.sessionId) throw rpcError('Native continuation returned a different session identity');
        if (result.capabilities) metadata = { ...metadata, capabilities: { ...capabilities, ...result.capabilities } };
        return result;
      }
      case 'turn.start': {
        if (id === undefined) throw rpcError('turn.start requires a request ID', -32600);
        const operationId = String(id);
        return call('start', { ...params, emit: event => notify(event, operationId),
          request: (name, values) => request(name, values, operationId) });
      }
      case 'turn.interrupt': return call('interrupt', params);
      case 'turn.steer': if (!capabilities.steer) unsupported('steer'); return call('steer', params);
      case 'selection.apply':
        if (params.selection?.model && !capabilities.modelSelection) unsupported('model selection');
        return call('configure', params);
      case 'conversation.list': if (!capabilities.list) unsupported('list'); return call('list', params);
      case 'conversation.history': if (!capabilities.history) unsupported('history'); return call('history', params);
      default: throw rpcError(`Unknown native runtime method: ${method}`, -32601);
    }
  }
  async function receive(message) {
    const validId = value => typeof value === 'string' || (typeof value === 'number' && Number.isFinite(value));
    if (!object(message) || message.jsonrpc !== '2.0') {
      send({ jsonrpc: '2.0', id: null, error: { code: -32600, message: 'Invalid JSON-RPC 2.0 message' } });
      return drain();
    }
    if (!Object.hasOwn(message, 'method')) {
      const waiter = pending.get(message.id);
      if (!waiter) return;
      pending.delete(message.id); clearTimeout(waiter.timer);
      if (object(message.error)) waiter.reject(rpcError(message.error.message ?? 'Native interaction rejected', message.error.code, message.error.data));
      else if (Object.hasOwn(message, 'result')) waiter.resolve(message.result);
      else waiter.reject(rpcError('Invalid native interaction response', -32600));
      return;
    }
    const hasId = Object.hasOwn(message, 'id');
    if (typeof message.method !== 'string' || (hasId && !validId(message.id)) ||
        (message.params !== undefined && !object(message.params))) {
      send({ jsonrpc: '2.0', id: validId(message.id) ? message.id : null, error: { code: -32600, message: 'Invalid native runtime request' } });
      return drain();
    }
    if (hasId && inbound.has(message.id)) {
      send({ jsonrpc: '2.0', id: message.id, error: { code: -32600, message: 'Duplicate active request ID' } });
      return drain();
    }
    if (hasId) inbound.add(message.id);
    try {
      const result = await dispatch(message.method, message.params ?? {}, hasId ? message.id : undefined);
      if (hasId) send({ jsonrpc: '2.0', id: message.id, result: result ?? null });
    } catch (error) {
      if (hasId) send({ jsonrpc: '2.0', id: message.id, error: {
        code: typeof error.code === 'number' ? error.code : -32000,
        message: error.message ?? String(error),
        ...(error.details === undefined && typeof error.code !== 'string' ? {} : { data: { ...(error.details === undefined ? {} : { details: error.details }), ...(typeof error.code === 'string' ? { nativeCode: error.code } : {}) } }),
      } });
    } finally { if (hasId) inbound.delete(message.id); }
    await drain();
  }
  async function handleLine(line) {
    if (!line.trim()) return;
    let message;
    try { message = JSON.parse(line); }
    catch { send({ jsonrpc: '2.0', id: null, error: { code: -32700, message: 'Invalid JSON' } }); return drain(); }
    return receive(message);
  }
  return { receive, handleLine, close, drain, get closed() { return closed; } };
}

async function main() {
  // Provider/SDK chatter must never corrupt the NDJSON protocol. Do not modify
  // the embedding application's console when this module is merely imported.
  console.log = (...args) => process.stderr.write(format(...args).slice(0, 8192) + '\n');
  const host = createHost({ write: line => new Promise((resolveWrite, reject) => {
    process.stdout.write(line, error => error ? reject(error) : resolveWrite());
  }) });
  const lines = createInterface({ input: process.stdin, crlfDelay: Infinity });
  let closing;
  const stop = () => closing ??= host.close().then(() => host.drain()).catch(error => {
    process.stderr.write(String(error.message).slice(0, 8192) + '\n'); process.exitCode = 1;
  });
  lines.on('line', line => { void host.handleLine(line).then(() => {
    if (host.closed) { process.stdin.pause(); lines.close(); }
  }).catch(error => {
    process.stderr.write(String(error.message).slice(0, 8192) + '\n'); void stop();
  }); });
  lines.on('close', () => { void stop(); });
  process.once('SIGTERM', () => { lines.close(); void stop(); });
  process.once('SIGINT', () => { lines.close(); void stop(); });
  process.stdout.on('error', () => { lines.close(); void stop(); });
}

if (process.argv[1] && pathToFileURL(resolve(process.argv[1])).href === import.meta.url) {
  if (Number(process.versions.node.split('.')[0]) < 20) {
    process.stderr.write('Native agent runtime requires Node.js 20 or newer\n'); process.exitCode = 1;
  } else await main();
}