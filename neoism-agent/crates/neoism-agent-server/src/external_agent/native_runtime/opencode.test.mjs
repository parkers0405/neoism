import test from 'node:test';
import assert from 'node:assert/strict';
import { createServer } from 'node:http';
import { mkdtemp, writeFile, rm } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { setTimeout as delay } from 'node:timers/promises';
import { createAdapter } from './opencode.mjs';

// An actual executable/server, not fetch mocks. Startup/auth/argv/cwd/exit are
// exercised too. The control server is test-only and never exists in the adapter.
function executableMain() {
  import('node:http').then(({ createServer }) => {
    const control = process.env.NEOISM_OPENCODE_TEST_CONTROL;
    const password = process.env.OPENCODE_SERVER_PASSWORD;
    const expected = `Basic ${Buffer.from(`opencode:${password}`).toString('base64')}`;
    if (process.argv.slice(2).join(' ') !== 'serve --hostname=127.0.0.1 --port=0') process.exit(7);
    const server = createServer(async (req, res) => {
      if (req.headers.authorization !== expected) { res.writeHead(401).end(); return; }
      // Relay to the parent test's stateful native API. Secrets stay in this process.
      const { request } = await import('node:http');
      const upstream = request(control + req.url, { method: req.method,
        headers: { ...req.headers, authorization: 'test-auth-checked', 'x-test-cwd': encodeURIComponent(process.cwd()),
          'x-test-private-auth': String(password.length === 64 && password !== 'inherited-password') } }, r => {
        res.writeHead(r.statusCode, r.headers); r.pipe(res);
        res.on('close', () => r.destroy());
      });
      upstream.on('error', () => res.destroy());
      req.pipe(upstream);
      res.on('close', () => upstream.destroy());
    });
    server.listen(0, '127.0.0.1', () => {
      console.log(`opencode server listening on http://127.0.0.1:${server.address().port}`);
    });
    process.on('SIGTERM', () => { server.closeAllConnections(); server.close(() => process.exit(0)); });
  });
}

async function fixture(t, { scenario = 'normal' } = {}) {
  const dir = await mkdtemp(join(tmpdir(), 'neoism opencode '));
  const executable = join(dir, 'fake-opencode');
  await writeFile(executable, `#!${process.execPath}\n(${executableMain.toString()})();\n`, { mode: 0o700 });
  const requests = [], sessions = new Map(), histories = new Map(), streams = new Set();
  let seq = 0, prompt, current, status = {}, aborted = false, permit, question;
  const json = (res, value, code = 200) => { res.writeHead(code, { 'Content-Type': 'application/json' }); res.end(JSON.stringify(value)); };
  const send = (type, properties, directory, eventId) => {
    const event = { id: eventId ?? `evt_${++seq}`, type, properties };
    for (const s of streams) s.write(`data: ${JSON.stringify(directory ? { directory, payload: event } : event)}\r\n\r\n`);
  };
  const makeInfo = (sid = current, id = `msg_answer_${seq}`) => ({ id, role: 'assistant', sessionID: sid,
    parentID: prompt?.messageID, providerID: 'host', modelID: 'default', time: { created: 1 },
    tokens: { input: 3, output: 2, reasoning: 1, cache: { read: 4, write: 0 } } });
  const complete = async () => {
    const info = makeInfo(current, 'msg_answer');
    const text = { id: 'prt_text', sessionID: current, messageID: info.id, type: 'text', text: 'Hello world', time: { end: 2 } };
    const reasoning = { id: 'prt_reason', sessionID: current, messageID: info.id, type: 'reasoning', text: 'Think', time: { end: 2 } };
    const tool = { id: 'prt_tool', callID: 'call_tool', sessionID: current, messageID: info.id, type: 'tool', tool: 'shell',
      state: { status: 'completed', input: { command: 'pwd' }, output: 'initial\nlast\n', metadata: { exit: 0 } } };
    histories.set(current, [...histories.get(current), { info: { ...info, time: { created: 1, completed: 2 } }, parts: [text, reasoning, tool] }]);
    send('message.part.updated', { part: text });
    send('message.part.updated', { part: tool });
    // Duplicate terminal snapshots must not resurrect tools or repeat output.
    send('message.part.updated', { part: tool });
    send('message.updated', { info: { id: info.id, role: 'assistant', sessionID: current,
      parentID: prompt.messageID, time: { created: 1, completed: 2 } } });
    status = {}; send('session.status', { sessionID: current, status: { type: 'idle' } });
  };
  const begin = async () => {
    status[current] = { type: 'busy' };
    send('session.status', { sessionID: current, status: { type: 'busy' } });
    send('message.updated', { info: { id: prompt.messageID, role: 'user', sessionID: current } });
    send('message.part.updated', { part: { id: 'prt_user', sessionID: current, messageID: prompt.messageID, type: 'text', text: 'MUST NOT ECHO' } });
    const info = makeInfo(current, 'msg_answer');
    send('message.updated', { info });
    send('message.part.updated', { part: { id: 'prt_text', sessionID: current, messageID: info.id, type: 'text', text: 'Hello' } });
    send('message.part.delta', { sessionID: current, messageID: info.id, partID: 'prt_text', field: 'text', delta: ' world' }, undefined, 'evt_delta');
    send('message.part.delta', { sessionID: current, messageID: info.id, partID: 'prt_text', field: 'text', delta: ' world' }, undefined, 'evt_delta');
    // Older partial snapshot arrives after the delta.
    send('message.part.updated', { part: { id: 'prt_text', sessionID: current, messageID: info.id, type: 'text', text: 'Hello' } });
    send('message.part.delta', { sessionID: current, messageID: info.id, partID: 'prt_reason', field: 'text', delta: 'Think' });
    send('message.part.updated', { part: { id: 'prt_reason', sessionID: current, messageID: info.id, type: 'reasoning', text: '' } });
    send('message.part.updated', { part: { id: 'prt_tool', callID: 'call_tool', sessionID: current, messageID: info.id, type: 'tool', tool: 'shell',
      state: { status: 'running', input: { command: 'pwd' }, metadata: { output: 'initial\n' } } } });
    send('todo.updated', { sessionID: current, todos: [{ content: 'Verify', status: 'in_progress', priority: 'high' }] });
    send('message.updated', { info: makeInfo('ses_unrelated', 'msg_other') });
    send('message.part.updated', { part: { id: 'prt_other', sessionID: 'ses_unrelated', messageID: 'msg_other', type: 'text', text: 'WRONG SESSION' } });
    send('message.part.updated', { part: { id: 'prt_dir', sessionID: current, messageID: info.id, type: 'text', text: 'WRONG DIR' } }, '/elsewhere');
    if (scenario === 'interactions') {
      send('permission.asked', { id: 'per_native', sessionID: current, permission: 'bash', patterns: ['pwd'], metadata: { command: 'pwd' }, tool: { callID: 'call_tool' } });
      send('question.asked', { id: 'que_native', sessionID: current, questions: [{ question: 'Which?', header: 'Choice', options: [{ label: 'A', description: 'First' }], multiple: true }] });
    }
    if (scenario === 'children') {
      const s = { id: 'ses_child', directory: dir, parentID: current, title: 'Native child' };
      sessions.set(s.id, s); histories.set(s.id, []);
      send('session.created', { info: s });
      const ci = makeInfo(s.id, 'msg_child');
      send('message.updated', { info: ci });
      const cp = { id: 'prt_child', sessionID: s.id, messageID: ci.id, type: 'text', text: 'child output' };
      send('message.part.updated', { part: cp });
      histories.set(s.id, [{ info: ci, parts: [cp] }]);
      send('session.status', { sessionID: s.id, status: { type: 'idle' } });
    }
  };
  const server = createServer(async (req, res) => {
    const url = new URL(req.url, 'http://localhost');
    let body = ''; for await (const b of req) body += b;
    body = body ? JSON.parse(body) : undefined;
    requests.push({ method: req.method, path: url.pathname, directory: url.searchParams.get('directory'), body });
    assert.equal(req.headers.authorization, 'test-auth-checked');
    assert.equal(req.headers['x-test-private-auth'], 'true');
    assert.equal(decodeURIComponent(req.headers['x-test-cwd']), dir);
    if (url.pathname === '/global/health') return json(res, { healthy: true, version: '1.18.35' });
    if (url.pathname === '/config/providers') return json(res, { providers: [{ id: 'host', models: {
      default: { id: 'default', name: 'Host model', limit: { context: 12345 }, variants: { deep: {} } },
      alternate: { id: 'alternate', name: 'Other' } } }], default: { host: 'default' } });
    if (url.pathname === '/config') return json(res, { model: 'host/default', default_agent: 'build', secret: 'never expose config' });
    if (url.pathname === '/agent') return json(res, [{ name: 'build', mode: 'primary' }, { name: 'explore', mode: 'subagent' }]);
    if (url.pathname === '/command') return json(res, [{ name: 'review', description: 'Review code' }]);
    if (url.pathname === '/event') {
      res.writeHead(200, { 'Content-Type': 'text/event-stream' }); res.write(': subscribed\n\n'); streams.add(res);
      res.on('close', () => streams.delete(res)); return;
    }
    if (url.pathname === '/session' && req.method === 'POST') {
      assert.deepEqual(body, { permission: [{ permission: '*', pattern: '*', action: 'ask' }] });
      const s = { id: `ses_${sessions.size + 1}`, directory: dir, title: 'Native', time: { updated: 123 } };
      sessions.set(s.id, s); histories.set(s.id, []); return json(res, s);
    }
    if (url.pathname === '/session' && req.method === 'GET') return json(res, [...sessions.values(), { id: 'ses_wrong', directory: '/wrong' }, { id: 'ses_unknown' }]);
    if (url.pathname === '/session/status') return json(res, status);
    const match = url.pathname.match(/^\/session\/([^/]+)(?:\/(.*))?$/);
    if (match) {
      const [, sid, op] = match;
      if (!sessions.has(sid)) return json(res, { error: 'not found' }, 404);
      if (!op) {
        if (req.method === 'PATCH') {
          assert.deepEqual(body, { permission: [{ permission: '*', pattern: '*', action: 'ask' }] });
          sessions.get(sid).permission = body.permission;
        }
        return json(res, sessions.get(sid));
      }
      if (op === 'message') return json(res, histories.get(sid));
      if (op === 'prompt_async') {
        assert.ok(streams.size > 0, 'subscription exists before prompt');
        current = sid; prompt = body;
        if (scenario === 'disconnect') { res.writeHead(204).end(); for (const s of streams) s.end(); return; }
        await begin(); res.writeHead(204).end(); return;
      }
      if (op === 'abort') {
        aborted = true;
        if (scenario !== 'abort-unconfirmed') { status = {}; send('session.status', { sessionID: current, status: { type: 'idle' } }); }
        return json(res, true);
      }
    }
    if (url.pathname === '/permission/per_native/reply') { permit = body; return json(res, true); }
    if (url.pathname === '/question/que_native/reply' || url.pathname === '/question/que_native/reject') { question = { path: url.pathname, ...body }; return json(res, true); }
    return json(res, { error: 'unexpected endpoint' }, 404);
  });
  await new Promise(res => server.listen(0, '127.0.0.1', res));
  const previous = process.env.NEOISM_OPENCODE_TEST_CONTROL;
  process.env.NEOISM_OPENCODE_TEST_CONTROL = `http://127.0.0.1:${server.address().port}`;
  const adapter = createAdapter({ cwd: dir, executable });
  t.after(async () => {
    await adapter.close(); server.closeAllConnections(); await new Promise(res => server.close(res));
    if (previous === undefined) delete process.env.NEOISM_OPENCODE_TEST_CONTROL; else process.env.NEOISM_OPENCODE_TEST_CONTROL = previous;
    await rm(dir, { recursive: true, force: true });
  });
  return { adapter, dir, requests, sessions, histories, streams, complete, send,
    get prompt() { return prompt; }, get aborted() { return aborted; }, get permit() { return permit; }, get question() { return question; } };
}

async function until(fn) {
  for (let i = 0; i < 300; i++) { if (fn()) return; await delay(10); }
  assert.fail('timed out waiting for native flow');
}

test('native discovery, exact resume, per-turn selection, streaming and late snapshots', async t => {
  const f = await fixture(t), a = f.adapter;
  const d = await a.describe();
  assert.equal(d.capabilities.nativeChildren, 'observed'); assert.equal(d.capabilities.steer, false);
  assert.equal(d.models[0].contextWindow, 12345);
  assert.deepEqual(d.options.find(o => o.id === 'variant').options, [{ value: 'deep', name: 'deep' }]);
  assert.ok(!JSON.stringify(d).includes('never expose config'));
  const opened = await a.open();
  const count = f.sessions.size;
  await assert.rejects(a.open({ sessionId: 'ses_missing' }), /HTTP 404/);
  assert.equal(f.sessions.size, count);
  assert.equal((await a.open({ sessionId: opened.sessionId })).sessionId, opened.sessionId);
  await assert.rejects(a.configure({ sessionId: opened.sessionId, selection: { model: 'unknown/model' } }), /Unsupported/);
  await assert.rejects(a.configure({ sessionId: opened.sessionId, selection: { options: { unsafe: true } } }), /Unsupported/);
  await assert.rejects(a.configure({ sessionId: opened.sessionId, selection: { options: { variant: 'unknown' } } }), /Unsupported/);
  assert.deepEqual(await a.configure({ sessionId: opened.sessionId, selection: { model: 'host/alternate', options: { agent: 'build' } } }),
    { applied: 'next-turn', selection: { model: 'host/alternate', options: { agent: 'build' } } });
  const events = []; let finished = false;
  const running = a.start({ sessionId: opened.sessionId, text: 'literal ; $(not-a-shell)', emit: e => events.push(e), request: () => assert.fail('unexpected interaction') }).then(r => { finished = true; return r; });
  await until(() => events.some(e => e.type === 'text.delta' && e.text === ' world'));
  assert.equal(finished, false, 'tokens arrive before completion');
  assert.deepEqual(f.prompt.model, { providerID: 'host', modelID: 'alternate' });
  assert.equal(f.prompt.agent, 'build');
  assert.equal(f.prompt.parts[0].text, 'literal ; $(not-a-shell)');
  await f.complete(); const result = await running;
  assert.equal(result.finish, 'stop');
  assert.equal(events.filter(e => e.type === 'text.delta').map(e => e.text).join(''), 'Hello world');
  assert.equal(events.filter(e => e.type === 'reasoning.delta').map(e => e.text).join(''), 'Think');
  assert.equal(events.filter(e => e.type === 'text.completed').length, 1);
  assert.equal(events.filter(e => e.type === 'tool.started').length, 1);
  assert.deepEqual(events.filter(e => e.type === 'tool.output').map(e => [e.mode, e.text]), [['replace', 'initial\n'], ['replace', 'initial\nlast\n']]);
  assert.equal(events.filter(e => e.type === 'tool.completed').length, 1);
  assert.ok(events.some(e => e.type === 'plan.updated'));
  assert.equal(result.usage.input, 3); assert.equal(result.usage.cacheRead, 4);
  assert.ok(f.requests.every(r => r.directory === f.dir));
  await until(() => f.streams.size === 0);
});

test('default model is omitted from prompt; native permission/question callbacks and HTTP replies', async t => {
  const f = await fixture(t, { scenario: 'interactions' });
  const { sessionId } = await f.adapter.open();
  const calls = [], events = [];
  const running = f.adapter.start({ sessionId, text: 'ask', emit: e => events.push(e), request: async (method, params) => {
    calls.push([method, params]);
    return method === 'permission.request' ? { decision: 'allow_once' } : { answers: { 0: ['A', 'custom answer'] } };
  } });
  await until(() => f.permit && f.question);
  assert.equal(f.prompt.model, undefined);
  assert.deepEqual(f.permit, { reply: 'once' });
  assert.deepEqual(f.question.answers, [['A', 'custom answer']]);
  assert.equal(calls[0][1].toolCallId, 'call_tool');
  assert.equal(calls[1][1].questions[0].multiple, true);
  await f.complete(); await running;
});

test('interaction cancellation denies/rejects without auto-answer; pending replies cannot escape closed turn', async t => {
  const f = await fixture(t, { scenario: 'interactions' });
  const { sessionId } = await f.adapter.open();
  const run = f.adapter.start({ sessionId, text: 'ask', emit() {}, request: async () => { throw new Error('cancelled'); } });
  await until(() => f.permit && f.question);
  assert.deepEqual(f.permit, { reply: 'reject' });
  assert.equal(f.question.path, '/question/que_native/reject');
  await f.complete(); await run;
});

test('confirmed abort, isolated callbacks on next turn, and idempotent cleanup', async t => {
  const f = await fixture(t);
  const { sessionId } = await f.adapter.open();
  const first = [], second = [];
  const run = f.adapter.start({ sessionId, text: 'first', emit: e => first.push(e), request() {} });
  await until(() => first.some(e => e.type === 'text.delta'));
  assert.deepEqual(await f.adapter.interrupt({ sessionId }), { interrupted: true });
  assert.equal((await run).finish, 'interrupted'); assert.ok(f.aborted);
  const firstCount = first.length;
  const next = f.adapter.start({ sessionId, text: 'second', emit: e => second.push(e), request() {} });
  await until(() => second.some(e => e.type === 'text.delta'));
  await f.complete(); await next;
  assert.equal(first.length, firstCount);
  await f.adapter.close(); await f.adapter.close();
  await assert.rejects(f.adapter.describe(), /closed/);
  await until(() => f.streams.size === 0);
});

test('observed native child IDs and scopes; native list/history are read-only', async t => {
  const f = await fixture(t, { scenario: 'children' });
  const { sessionId } = await f.adapter.open(); const events = [];
  const run = f.adapter.start({ sessionId, text: 'delegate', emit: e => events.push(e), request() {} });
  await until(() => events.some(e => e.type === 'child.updated' && e.status === 'completed'));
  await f.complete(); await run;
  const childText = events.find(e => e.type === 'text.delta' && e.scope);
  assert.equal(childText.scope.id, 'ses_child'); assert.equal(childText.scope.parentId, sessionId);
  const listed = await f.adapter.list({ cwd: f.dir });
  assert.deepEqual(listed.sessions.map(s => s.sessionId), [sessionId, 'ses_child']);
  const promptsBefore = f.requests.filter(r => r.path.endsWith('/prompt_async')).length;
  const history = await f.adapter.history({ sessionId, cwd: f.dir });
  assert.equal(history.complete, true); assert.equal(history.cwd, f.dir);
  assert.ok(history.messages[0].events.some(e => e.type === 'tool.completed'));
  assert.equal(f.requests.filter(r => r.path.endsWith('/prompt_async')).length, promptsBefore);
  f.sessions.get(sessionId).directory = '/wrong';
  await assert.rejects(f.adapter.history({ sessionId, cwd: f.dir }), /mismatch/);
});

test('SSE disconnect fails rather than claiming completion', async t => {
  const f = await fixture(t, { scenario: 'disconnect' });
  const { sessionId } = await f.adapter.open();
  await assert.rejects(f.adapter.start({ sessionId, text: 'fail', emit() {}, request() {} }), /SSE disconnected/);
});

test('abort acknowledgement alone never reports an unconfirmed cancellation', async t => {
  const f = await fixture(t, { scenario: 'abort-unconfirmed' });
  const { sessionId } = await f.adapter.open(); const events = [];
  const run = f.adapter.start({ sessionId, text: 'busy', emit: e => events.push(e), request() {} });
  const closed = assert.rejects(run, /closed/);
  await until(() => events.some(e => e.type === 'text.delta'));
  await assert.rejects(f.adapter.interrupt({ sessionId }), /not confirmed idle/);
  assert.ok(!events.some(e => e.status === 'interrupted'));
  await f.adapter.close(); await closed;
});

test('close rejects an active turn and suppresses late interaction replies', async t => {
  const f = await fixture(t, { scenario: 'interactions' });
  const { sessionId } = await f.adapter.open();
  const replies = [];
  const run = f.adapter.start({ sessionId, text: 'pending', emit() {}, request: () => new Promise(resolve => replies.push(resolve)) });
  const rejected = assert.rejects(run, /closed/);
  await until(() => replies.length === 2);
  await f.adapter.close(); await rejected;
  replies[0]({ decision: 'allow_session' }); replies[1]({ answers: { 0: ['A'] } });
  await delay(30);
  assert.equal(f.permit, undefined); assert.equal(f.question, undefined);
});

test('startup timeout is bounded and failed runtime cannot silently fall back', async t => {
  const dir = await mkdtemp(join(tmpdir(), 'neoism-opencode-timeout-'));
  const executable = join(dir, 'silent-server');
  await writeFile(executable, `#!${process.execPath}\nsetInterval(() => {}, 1000);\n`, { mode: 0o700 });
  const adapter = createAdapter({ cwd: dir, executable, startupTimeoutMs: 60 });
  t.after(async () => { await adapter.close(); await rm(dir, { recursive: true, force: true }); });
  await assert.rejects(adapter.open(), /readiness timed out/);
  await assert.rejects(adapter.open(), /readiness timed out/);
});