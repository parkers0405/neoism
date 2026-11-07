import test from 'node:test';
import assert from 'node:assert/strict';
import { spawn } from 'node:child_process';
import { fileURLToPath } from 'node:url';
import { createHost } from './host.mjs';

const tick = () => new Promise(resolve => setImmediate(resolve));
function deferred() {
  let resolve, reject;
  const promise = new Promise((a, b) => { resolve = a; reject = b; });
  return { promise, resolve, reject };
}
function setup(options = {}) {
  const frames = [], contexts = [], starts = [], calls = [];
  const capabilities = { resume: true, history: true, list: true, modelSelection: true, steer: false, nativeChildren: 'observed' };
  const metadata = { capabilities, models: [{ id: 'native-model' }], options: [], commands: [], auth: { owner: 'native' } };
  const adapter = {
    async describe() { calls.push(['describe']); return metadata; },
    async open(params) { calls.push(['open', params]); return { sessionId: params.sessionId ?? null, capabilities }; },
    async start(params) { starts.push(params); return { finish: 'completed' }; },
    async interrupt(params) { calls.push(['interrupt', params]); return { interrupted: true }; },
    async configure(params) { calls.push(['configure', params]); return { ...params.selection, applied: 'next-turn' }; },
    async list(params) { calls.push(['list', params]); return { sessions: [] }; },
    async history(params) { calls.push(['history', params]); return { ...params, messages: [], complete: false }; },
    async close() { calls.push(['close']); },
  };
  const factory = context => { contexts.push(context); return adapter; };
  const host = createHost({ output: frame => frames.push(frame), adapters: { codex: factory, claude: factory, opencode: factory },
    loadSdk: async () => { throw new Error('Unexpected SDK loading'); }, ...options });
  let sequence = 0;
  const send = (method, params = {}, id = ++sequence) => host.receive({ jsonrpc: '2.0', id, method, params });
  const response = id => frames.find(frame => frame.id === id && !frame.method);
  const init = (params = {}) => send('runtime.initialize', { provider: 'codex', cwd: '/project', executable: '/native/agent', ...params });
  return { host, frames, adapter, metadata, capabilities, contexts, starts, calls, send, response, init };
}

test('initialize returns provider-confirmed metadata; dispatch preserves lazy null session IDs and effective selections', async t => {
  const h = setup(); t.after(() => h.host.close());
  await h.init({ policy: { permission: 'inherited' }, preview: true });
  assert.deepEqual(h.response(1).result, h.metadata);
  assert.equal(h.contexts[0].cwd, '/project');
  assert.deepEqual(h.contexts[0].policy, { permission: 'inherited' });
  assert.equal(h.contexts[0].preview, true);
  await h.send('runtime.describe', {}, 2);
  await h.send('conversation.open', {}, 3);
  assert.equal(h.response(3).result.sessionId, null);
  await h.send('selection.apply', { sessionId: 'real', selection: { model: 'native-model' } }, 4);
  assert.equal(h.response(4).result.applied, 'next-turn');
  await h.send('conversation.list', { cwd: '/confirmed' }, 5);
  await h.send('conversation.history', { sessionId: 'real', cwd: '/confirmed' }, 6);
  assert.equal(h.response(6).result.complete, false);
  assert.deepEqual(h.calls.find(c => c[0] === 'history')[1], { sessionId: 'real', cwd: '/confirmed' });
  await h.send('runtime.initialize', { provider: 'codex', cwd: '/project', executable: '/native/agent' }, 7);
  assert.match(h.response(7).error.message, /already initialized/);
});

test('only Claude initialization resolves SDK; injected SDK bypasses installation entirely', async t => {
  let loads = 0;
  const sdk = { query() {} };
  const h = setup({ loadSdk: async (params, signal) => { loads++; assert.equal(params.dependencyRoot, '/private'); assert.equal(signal.aborted, false); return sdk; } });
  t.after(() => h.host.close());
  await h.init({ provider: 'claude', executable: null, dependencyRoot: '/private' });
  assert.equal(loads, 1);
  assert.equal(h.contexts[0].sdk, sdk);
  for (const provider of ['codex', 'opencode']) {
    const other = setup({ loadSdk: async () => { throw new Error('Non-Claude attempted SDK install'); } });
    t.after(() => other.host.close());
    await other.init({ provider, dependencyRoot: '/unused', npm: '/unused/npm' });
    assert.equal(other.response(1).error, undefined);
    assert.equal(other.contexts[0].sdk, undefined);
  }
  const injected = setup({ sdk }); t.after(() => injected.host.close());
  await injected.init({ provider: 'claude', executable: null });
  assert.equal(injected.contexts[0].sdk, sdk);
});

test('turn callback IDs stay bound after start resolves and across later turns; queued events precede result', async t => {
  const frames = [];
  const h = setup({ output: async frame => { await tick(); frames.push(frame); } });
  t.after(() => h.host.close());
  await h.init();
  h.adapter.start = async params => {
    h.starts.push(params);
    params.emit({ type: 'text.delta', id: 'text', text: 'hello' });
    params.emit({ type: 'text.completed', id: 'text', text: 'hello' });
    return { finish: 'completed' };
  };
  await h.send('turn.start', { text: 'first' }, 10);
  await h.send('turn.start', { text: 'second' }, 11);
  h.starts[0].emit({ type: 'child.updated', scope: { id: 'native-child' }, status: 'completed' });
  h.starts[1].emit({ type: 'runtime.status', status: 'ready' });
  await h.host.drain();
  const firstEvents = frames.filter(frame => frame.method === 'runtime.event' && frame.params.operationId === '10');
  assert.equal(firstEvents.length, 3);
  const firstResult = frames.findIndex(frame => frame.id === 10);
  assert.ok(frames.indexOf(firstEvents[1]) < firstResult);
  assert.ok(frames.indexOf(firstEvents[2]) > firstResult);
  assert.equal(firstEvents[2].params.event.scope.id, 'native-child');
  assert.equal(frames.at(-1).params.operationId, '11');
});

test('permission and question requests have distinct string IDs, route responses concurrently, and retain turn identity', async t => {
  const h = setup(); t.after(() => h.host.close());
  await h.init();
  h.adapter.start = async params => {
    h.starts.push(params);
    const permission = await params.request('permission.request', { id: 'tool', toolCallId: 'tool', toolName: 'Bash', input: { command: 'pwd' } });
    assert.equal(permission.decision, 'allow_once');
    const answer = await params.request('question.request', { id: 'question', questions: [{ id: 'q', question: 'Which?' }] });
    assert.deepEqual(answer.answers.q, ['Rust']);
    params.emit({ type: 'tool.completed', id: 'tool', output: 'done' });
    return { finish: 'completed' };
  };
  const running = h.send('turn.start', { text: 'work' }, 20);
  await tick(); await h.host.drain();
  const permission = h.frames.find(f => f.method === 'permission.request');
  assert.equal(typeof permission.id, 'string');
  assert.equal(permission.params.operationId, '20');
  // A numeric Rust response must not accidentally resolve the string-keyed request.
  await h.host.receive({ jsonrpc: '2.0', id: 1, result: { decision: 'deny' } });
  assert.equal(h.response(20), undefined);
  await h.send('turn.interrupt', { sessionId: 'real' }, 21);
  assert.deepEqual(h.response(21).result, { interrupted: true });
  await h.host.receive({ jsonrpc: '2.0', id: permission.id, result: { decision: 'allow_once' } });
  await tick(); await h.host.drain();
  const question = h.frames.find(f => f.method === 'question.request');
  assert.notEqual(question.id, permission.id);
  assert.equal(question.params.operationId, '20');
  await h.host.receive({ jsonrpc: '2.0', id: question.id, result: { answers: { q: ['Rust'] } } });
  await running;
  assert.equal(h.response(20).result.finish, 'completed');
  const late = h.starts[0].request('permission.request', { toolName: 'Read', input: {} });
  await h.host.drain();
  const lastRequest = h.frames.at(-1);
  assert.equal(lastRequest.params.operationId, '20');
  await h.host.receive({ jsonrpc: '2.0', id: lastRequest.id, result: { decision: 'deny' } });
  assert.equal((await late).decision, 'deny');
});

test('close rejects outstanding interactions and pending turns and closes the adapter exactly once', async () => {
  const h = setup(); await h.init();
  let interactionError;
  h.adapter.start = async params => {
    try { await params.request('permission.request', { toolName: 'Write', input: {} }); }
    catch (error) { interactionError = error; throw error; }
  };
  const running = h.send('turn.start', {}, 30); await tick();
  await h.send('runtime.close', {}, 31); await running;
  assert.match(interactionError.message, /closed/);
  assert.match(h.response(30).error.message, /closed/);
  assert.deepEqual(h.response(31).result, { closed: true });
  await h.host.close();
  assert.equal(h.calls.filter(c => c[0] === 'close').length, 1);
  await h.send('conversation.open', {}, 32);
  assert.match(h.response(32).error.message, /closed/);
});

test('interaction timeout and Rust error replies preserve explicit failures', async t => {
  const h = setup({ requestTimeoutMs: 10 }); t.after(() => h.host.close());
  await h.init();
  h.adapter.start = async params => params.request('question.request', { questions: [] });
  await h.send('turn.start', {}, 40);
  assert.match(h.response(40).error.message, /timed out/);
  assert.equal(h.response(40).error.code, -32001);
  const running = h.send('turn.start', {}, 41); await tick(); await h.host.drain();
  const question = h.frames.filter(f => f.method === 'question.request').at(-1);
  await h.host.receive({ jsonrpc: '2.0', id: question.id, error: { code: -32009, message: 'User cancelled question', data: { reason: 'cancelled' } } });
  await running;
  assert.equal(h.response(41).error.code, -32009);
  assert.equal(h.response(41).error.message, 'User cancelled question');
  assert.deepEqual(h.response(41).error.data.details, { reason: 'cancelled' });
});

test('unsupported capabilities fail rather than forwarding or silently substituting a new session', async t => {
  const h = setup(); t.after(() => h.host.close()); await h.init();
  h.capabilities.history = false; h.capabilities.list = false; h.capabilities.resume = false; h.capabilities.modelSelection = false;
  for (const [method, params, id] of [
    ['conversation.history', {}, 50], ['conversation.list', {}, 51], ['turn.steer', { text: 'change' }, 52],
    ['conversation.open', { sessionId: 'requested' }, 53], ['selection.apply', { selection: { model: 'other' } }, 54],
  ]) { await h.send(method, params, id); assert.equal(h.response(id).error.code, -32601); }
  assert.equal(h.calls.some(c => c[0] === 'open'), false);
  h.capabilities.resume = true;
  h.adapter.open = async () => ({ sessionId: 'different' });
  await h.send('conversation.open', { sessionId: 'requested' }, 55);
  assert.match(h.response(55).error.message, /different session identity/);
});

test('malformed NDJSON, unknown methods, missing initialization and duplicate request IDs produce observable RPC errors', async t => {
  const h = setup(); t.after(() => h.host.close());
  await h.host.handleLine('{oops');
  assert.equal(h.frames.at(-1).error.code, -32700);
  await h.host.receive({ id: 1, method: 'runtime.describe' });
  assert.equal(h.frames.at(-1).error.code, -32600);
  await h.send('runtime.describe', {}, 60);
  assert.match(h.response(60).error.message, /not initialized/);
  await h.init();
  await h.send('no.such.method', {}, 61);
  assert.equal(h.response(61).error.code, -32601);
  const finish = deferred(); h.adapter.start = async () => finish.promise;
  const running = h.send('turn.start', {}, 62); await tick();
  await h.send('turn.start', {}, 62);
  assert.match(h.response(62).error.message, /Duplicate/);
  finish.resolve({ finish: 'completed' }); await running;
});

test('initialization rejects invalid providers/paths and native failures without fallback; close cancels SDK loading', async t => {
  const h = setup(); t.after(() => h.host.close());
  await h.init({ provider: 'acp' }); assert.match(h.response(1).error.message, /Unsupported native provider/);
  await h.init({ cwd: 'relative' }); assert.match(h.response(2).error.message, /absolute/);
  await h.init({ executable: 'on-path' }); assert.match(h.response(3).error.message, /absolute/);
  await h.init({ executable: null }); assert.match(h.response(4).error.message, /required/);
  const waiting = setup({ loadSdk: async (params, signal) => new Promise((_, reject) => signal.addEventListener('abort', () => reject(new Error('Cancelled SDK load')), { once: true })) });
  const initializing = waiting.init({ provider: 'claude', executable: null }); await tick();
  await waiting.send('runtime.close', {}, 70); await initializing;
  assert.ok(waiting.response(1).error);
  assert.equal(waiting.contexts.length, 0);
  assert.deepEqual(waiting.response(70).result, { closed: true });
});

test('NDJSON output is ordered and import leaves embedding console untouched', async () => {
  const originalLog = console.log;
  await import('./host.mjs'); assert.equal(console.log, originalLog);
  const lines = [];
  const host = createHost({ write: async line => { lines.push(line); } });
  await host.handleLine(JSON.stringify({ jsonrpc: '2.0', id: 80, method: 'runtime.close' }));
  assert.equal(lines.length, 1);
  assert.equal(lines[0].endsWith('\n'), true);
  assert.deepEqual(JSON.parse(lines[0]), { jsonrpc: '2.0', id: 80, result: { closed: true } });
});

test('CLI speaks only JSON on stdout and exits cleanly after runtime.close without any provider startup', async () => {
  const child = spawn(process.execPath, [fileURLToPath(new URL('./host.mjs', import.meta.url))], { stdio: ['pipe', 'pipe', 'pipe'] });
  let stdout = '', stderr = '';
  child.stdout.on('data', chunk => { stdout += chunk; }); child.stderr.on('data', chunk => { stderr += chunk; });
  const timer = setTimeout(() => child.kill('SIGKILL'), 5000);
  const exited = new Promise((resolve, reject) => { child.once('error', reject); child.once('close', resolve); });
  child.stdin.write('{bad}\n' + JSON.stringify({ jsonrpc: '2.0', id: 90, method: 'runtime.close' }) + '\n');
  const code = await exited; clearTimeout(timer);
  assert.equal(code, 0, stderr);
  const messages = stdout.trim().split('\n').map(line => JSON.parse(line));
  assert.equal(messages[0].error.code, -32700);
  assert.deepEqual(messages[1], { jsonrpc: '2.0', id: 90, result: { closed: true } });
  assert.equal(stderr, '');
});

test('real Claude adapter through the host publishes confirmed identity before content after a lazy open', async t => {
  const frames = [], nativeMessages = [], readers = [];
  let ended = false, wakeInput;
  const sdk = {
    query({ prompt }) {
      void (async () => {
        for await (const input of prompt) {
          wakeInput?.(input);
          const messages = [
            { type: 'system', subtype: 'init', model: 'claude-sonnet-4-6', session_id: 'native-session', parent_tool_use_id: null },
            { type: 'assistant', uuid: 'native-uuid', session_id: 'native-session', parent_tool_use_id: null,
              message: { id: 'native-message', content: [{ type: 'text', text: 'Actual SDK shape' }] } },
            { type: 'result', subtype: 'success', is_error: false, session_id: 'native-session', parent_tool_use_id: null,
              usage: { input_tokens: 3, output_tokens: 2 } },
          ];
          for (const message of messages) {
            if (readers.length) readers.shift()({ done: false, value: message });
            else nativeMessages.push(message);
          }
        }
      })();
      return {
        initializationResult: async () => ({ models: [{ value: 'default', displayName: 'Native default' }], commands: [], account: { tokenSource: 'none' } }),
        [Symbol.asyncIterator]() { return this; },
        next() {
          if (nativeMessages.length) return Promise.resolve({ done: false, value: nativeMessages.shift() });
          if (ended) return Promise.resolve({ done: true });
          return new Promise(resolve => readers.push(resolve));
        },
        close() { ended = true; for (const reader of readers.splice(0)) reader({ done: true }); },
      };
    },
  };
  const host = createHost({ output: frame => frames.push(frame), sdk }); t.after(() => host.close());
  const send = (id, method, params = {}) => host.receive({ jsonrpc: '2.0', id, method, params });
  await send(100, 'runtime.initialize', { provider: 'claude', cwd: '/project' });
  await send(101, 'conversation.open');
  assert.equal(frames.find(f => f.id === 101).result.sessionId, null);
  const input = new Promise(resolve => { wakeInput = resolve; });
  const running = send(102, 'turn.start', { sessionId: null, text: 'Hello' });
  assert.equal((await input).message.content, 'Hello');
  await running;
  const events = frames.filter(f => f.method === 'runtime.event');
  assert.equal(events[0].params.event.type, 'conversation.updated');
  assert.equal(events[0].params.event.sessionId, 'native-session');
  assert.equal(events.find(f => f.params.event.type === 'text.completed').params.event.text, 'Actual SDK shape');
  assert.equal(events.every(f => f.params.operationId === '102'), true);
  assert.ok(frames.indexOf(events.at(-1)) < frames.findIndex(f => f.id === 102));
});