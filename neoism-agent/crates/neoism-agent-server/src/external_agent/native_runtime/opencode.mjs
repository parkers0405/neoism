import { spawn } from 'node:child_process';
import { randomBytes } from 'node:crypto';
import { isAbsolute, resolve } from 'node:path';
import { setTimeout as delay } from 'node:timers/promises';

// Native OpenCode 1.x HTTP API (verified against 1.18.35 /doc). No ACP or SDK.
const capabilities = Object.freeze({ resume: true, history: true, list: true,
  modelSelection: true, steer: false, toolStreaming: true, reasoning: true, nativeChildren: 'observed' });
const enc = encodeURIComponent;
// Rust is the permission authority, including for sessions whose previous UI
// granted broader native permissions. This is session-local, never a config edit.
const permission = [{ permission: '*', pattern: '*', action: 'ask' }];
const usageOf = t => ({ input: t.input ?? 0, output: t.output ?? 0,
  reasoning: t.reasoning ?? 0, cacheRead: t.cache?.read ?? 0, cacheWrite: t.cache?.write ?? 0,
  ...(t.total === undefined ? {} : { total: t.total }) });

// State is local to a turn/history import, never redirected to another operation.
function projector(emit) {
  const parts = new Map(), tools = new Map(), messages = new Map(), pending = new Map();
  let model;
  const send = (event, scope, sessionId) => emit({ ...event, sessionId, ...(scope ? { scope } : {}) });
  function part(p, scope, complete = false) {
    const sid = p.sessionID;
    if (p.type === 'text' || p.type === 'reasoning') {
      const key = `${sid}:${p.id}`;
      const prev = parts.get(key) ?? { text: '', done: false };
      if (prev.done) return;
      const text = p.text ?? '';
      // Late snapshots may be prefixes of bytes already emitted by delta events.
      if (text.startsWith(prev.text)) {
        const suffix = text.slice(prev.text.length);
        if (suffix) send({ type: `${p.type}.delta`, id: p.id, text: suffix }, scope, sid);
        prev.text = text;
      } else if (!prev.text.startsWith(text)) {
        throw new Error(`OpenCode changed already streamed ${p.type} part ${p.id}`);
      }
      Object.assign(prev, { p: { ...p, text: prev.text }, scope });
      parts.set(key, prev);
      const deltas = pending.get(key);
      if (deltas) {
        pending.delete(key);
        // These deltas arrived before the type snapshot; the snapshot is authoritative.
        const joined = deltas.join('');
        if (joined.startsWith(prev.text)) part({ ...p, text: joined }, scope);
      }
      if (complete || p.time?.end !== undefined) {
        send({ type: `${p.type}.completed`, id: p.id }, scope, sid);
        prev.done = true;
      }
    } else if (p.type === 'tool') {
      const id = p.callID ?? p.id, key = `${sid}:${id}`, s = p.state ?? {};
      const prev = tools.get(key) ?? { output: undefined, done: false };
      if (prev.done) return;
      const fields = { ...prev.fields };
      for (const [key, value] of Object.entries({ name: p.tool, title: s.title, input: s.input, metadata: s.metadata })) {
        if (value !== undefined) fields[key] = value;
      }
      const signature = JSON.stringify(fields);
      if (!tools.has(key)) send({ type: 'tool.started', id, ...fields, input: s.input ?? {} }, scope, sid);
      else if (signature !== prev.signature) send({ type: 'tool.updated', id, ...fields }, scope, sid);
      prev.signature = signature;
      prev.fields = fields;
      // Running bash output is a metadata snapshot in the native 1.x API.
      const output = s.output ?? s.metadata?.output;
      if (typeof output === 'string' && output !== prev.output) {
        send({ type: 'tool.output', id, text: output, mode: 'replace' }, scope, sid);
        prev.output = output;
      }
      if (s.status === 'completed' || s.status === 'error') {
        send(s.status === 'error'
          ? { type: 'tool.failed', id, error: s.error ?? 'OpenCode tool failed', metadata: s.metadata }
          : { type: 'tool.completed', id, input: fields.input, metadata: fields.metadata,
            ...(typeof fields.metadata?.exit === 'number' ? { exitCode: fields.metadata.exit } : {}) }, scope, sid);
        prev.done = true;
      }
      tools.set(key, prev);
    }
  }
  function delta(p, scope) {
    if (p.field !== 'text' || typeof p.delta !== 'string') return;
    const key = `${p.sessionID}:${p.partID}`, prev = parts.get(key);
    if (!prev) { const list = pending.get(key) ?? []; list.push(p.delta); pending.set(key, list); return; }
    if (!prev.done) part({ ...prev.p, text: prev.text + p.delta }, scope);
  }
  function message(info, scope) {
    if (info.role !== 'assistant') return;
    messages.set(info.id, { ...messages.get(info.id), ...info });
    if (info.modelID && info.providerID) {
      const next = `${info.providerID}/${info.modelID}`;
      if (next !== model) { model = next; send({ type: 'model.updated', model }, scope, info.sessionID); }
    }
    if (info.tokens) send({ type: 'usage.updated', usage: usage() }, scope, info.sessionID);
  }
  function usage() {
    const sum = {};
    for (const m of messages.values()) for (const [k, v] of Object.entries(usageOf(m.tokens ?? {}))) sum[k] = (sum[k] ?? 0) + v;
    return sum;
  }
  return { part, delta, message, usage, get model() { return model; },
    finish() { for (const v of parts.values()) if (!v.done) part(v.p, v.scope, true); } };
}

export function createAdapter(context) {
  const cwd = resolve(context.cwd);
  let child, base, startup, closed = false, active, discovery, starting = false;
  const password = randomBytes(32).toString('hex');
  const authorization = `Basic ${Buffer.from(`opencode:${password}`).toString('base64')}`;
  const selections = new Map();
  const lifetime = new AbortController();
  const timeoutMs = context.startupTimeoutMs ?? 15000;
  async function ready() {
    if (closed) throw new Error('OpenCode runtime closed');
    if (!startup) startup = (async () => {
      if (!isAbsolute(context.executable ?? '')) throw new Error('OpenCode requires an absolute executable');
      child = spawn(context.executable, ['serve', '--hostname=127.0.0.1', '--port=0'], {
        cwd, shell: false, detached: process.platform !== 'win32',
        env: { ...process.env, OPENCODE_SERVER_USERNAME: 'opencode', OPENCODE_SERVER_PASSWORD: password,
          // Newer installations also recognize this alias; never inherit an overriding password.
          OPENCODE_PASSWORD: password }, stdio: ['ignore', 'pipe', 'pipe'] });
      try {
        base = await new Promise((res, rej) => {
          let buffer = '';
          const timer = setTimeout(() => done(new Error('OpenCode server readiness timed out')), timeoutMs);
          const onAbort = () => done(new Error('OpenCode runtime closed'));
          const onExit = () => done(new Error('OpenCode server exited before readiness'));
          const onError = () => done(new Error('Unable to launch OpenCode server'));
          const onData = b => {
            buffer = (buffer + b.toString()).slice(-8192);
            const match = buffer.match(/http:\/\/127\.0\.0\.1:(\d+)/);
            if (match && Number(match[1]) > 0) done(null, match[0]);
          };
          function done(err, value) {
            clearTimeout(timer); lifetime.signal.removeEventListener('abort', onAbort);
            child.off('exit', onExit); child.off('error', onError); child.stdout.off('data', onData);
            err ? rej(err) : res(value);
          }
          lifetime.signal.addEventListener('abort', onAbort, { once: true });
          child.once('error', onError); child.once('exit', onExit); child.stdout.on('data', onData);
          // Drain without retaining/logging provider diagnostics (which may contain credentials).
          child.stderr.resume();
        });
        child.stdout.resume();
        child.on('error', () => {});
        child.on('exit', () => active?.fail(new Error('OpenCode server exited')));
        const health = await http('/global/health');
        if (!health?.healthy) throw new Error('OpenCode server health check failed');
      } catch (error) { await kill(); throw error; }
    })();
    return startup;
  }
  async function http(path, { method = 'GET', body, signal, directory = cwd, stream = false } = {}) {
    const url = new URL(path, base); url.searchParams.set('directory', directory);
    const r = await fetch(url, { method, headers: { Authorization: authorization,
      ...(body === undefined ? {} : { 'Content-Type': 'application/json' }),
      ...(stream ? { Accept: 'text/event-stream' } : {}) },
    body: body === undefined ? undefined : JSON.stringify(body),
    signal: AbortSignal.any([lifetime.signal, ...(signal ? [signal] : []), ...(stream ? [] : [AbortSignal.timeout(15000)])]) });
    if (!r.ok) { await r.body?.cancel(); const e = new Error(`OpenCode ${method} ${url.pathname}: HTTP ${r.status}`); e.code = r.status; throw e; }
    if (stream) {
      if (!r.headers.get('content-type')?.includes('text/event-stream')) { await r.body?.cancel(); throw new Error('OpenCode did not return SSE'); }
      return r;
    }
    return r.status === 204 ? undefined : r.json();
  }
  async function verify(sessionId, directory = cwd) {
    const s = await http(`/session/${enc(sessionId)}`, { directory });
    if (s.id !== sessionId || !s.directory || resolve(s.directory) !== resolve(directory)) throw new Error('OpenCode session directory/identity mismatch');
    return s;
  }
  async function describe({ selection = {} } = {}) {
    await ready();
    const [providers, config, agents, commands] = await Promise.all([
      http('/config/providers'), http('/config'), http('/agent'), http('/command')]);
    const models = providers.providers.flatMap(p => Object.values(p.models ?? {}).map(m => ({
      id: `${p.id}/${m.id}`, name: m.name, contextWindow: m.limit?.context })));
    discovery = { providers, config, agents, models };
    const selected = await selectionFor(selection);
    const modelId = selected.model ?? config.model;
    const split = modelId?.indexOf('/');
    const variants = new Set(split > 0 ? Object.keys(providers.providers.find(p => p.id === modelId.slice(0, split))?.models?.[modelId.slice(split + 1)]?.variants ?? {}) : []);
    return { capabilities, models, options: [{ id: 'agent', name: 'Agent', type: 'select',
      currentValue: config.default_agent ?? null, options: agents.filter(a => a.mode !== 'subagent' && !a.hidden).map(a => ({ value: a.name, name: a.name })) },
    ...(variants.size ? [{ id: 'variant', name: 'Model variant (model-specific)', type: 'select', currentValue: null,
      options: [...variants].map(v => ({ value: v, name: v })) }] : [])],
    commands: commands.map(c => ({ name: c.name, description: c.description })), auth: { managedBy: 'host' } };
  }
  async function selectionFor(selection = {}, previous = {}) {
    if (!discovery) await describe();
    const result = { ...previous, ...selection, options: { ...previous.options, ...selection.options } };
    if (result.model && !discovery.models.some(m => m.id === result.model)) throw new Error(`Unsupported OpenCode model: ${result.model}`);
    for (const [key, value] of Object.entries(result.options)) {
      if (key === 'agent' && discovery.agents.some(a => a.name === value && a.mode !== 'subagent' && !a.hidden)) continue;
      const modelId = result.model ?? discovery.config.model;
      if (key === 'variant' && modelId) {
        const split = modelId.indexOf('/'), provider = modelId.slice(0, split), model = modelId.slice(split + 1);
        if (discovery.providers.providers.find(p => p.id === provider)?.models?.[model]?.variants?.[value]) continue;
      }
      throw new Error(`Unsupported OpenCode option: ${key}=${value}`);
    }
    return result;
  }
  async function reconcile(turn) {
    for (const [sid, scope] of turn.sessions) {
      const history = await http(`/session/${enc(sid)}/message`);
      for (const m of history) {
        if (turn.old.has(m.info.id) || m.info.role !== 'assistant') continue;
        if (sid === turn.sessionId && m.info.parentID !== turn.promptID) continue;
        turn.project.message(m.info, scope);
        if (m.info.error) turn.error = m.info.error;
        for (const p of m.parts) turn.project.part(p, scope, true);
      }
    }
  }
  async function start({ sessionId, text, selection, emit, request }) {
    await ready();
    if (active) throw new Error('OpenCode turn already active');
    await verify(sessionId);
    const effective = await selectionFor(selection, selections.get(sessionId));
    const old = new Set((await http(`/session/${enc(sessionId)}/message`)).map(m => m.info.id));
    const streamAbort = new AbortController();
    let settle, reject;
    const done = new Promise((res, rej) => { settle = res; reject = rej; });
    // Avoid unhandled rejection while prompt admission is in flight.
    done.catch(() => {});
    const turn = { sessionId, promptID: `msg_${Date.now().toString(16)}${randomBytes(12).toString('hex')}`,
      old, sessions: new Map([[sessionId, undefined]]), roles: new Map(), accepted: false, admitted: false,
      project: projector(emit), interactions: new Set(), seenEvents: new Set(), finished: false,
      fail(error) { if (!turn.finished) { turn.finished = true; reject(error); streamAbort.abort(); } } };
    active = turn;
    async function finish(interrupted = false) {
      if (turn.finished || turn.finishing) return;
      turn.finishing = true;
      try {
        await reconcile(turn);
        if (turn.finished) return;
        turn.project.finish();
        if (turn.error && turn.error.name !== 'MessageAbortedError') throw new Error(`OpenCode: ${turn.error.data?.message ?? turn.error.name ?? 'session error'}`);
      const cancelled = interrupted || turn.error?.name === 'MessageAbortedError';
        emit({ type: 'runtime.status', sessionId, status: cancelled ? 'interrupted' : 'idle' });
        turn.finished = true;
        settle({ finish: cancelled ? 'interrupted' : 'stop', model: turn.project.model, usage: turn.project.usage() });
        streamAbort.abort();
      } catch (e) { turn.fail(e); }
    }
    turn.finish = finish;
    function interaction(type, p) {
      if (turn.interactions.has(p.id)) return;
      turn.interactions.add(p.id);
      const alive = () => !turn.finished && !streamAbort.signal.aborted;
      (async () => {
        if (type === 'permission.asked') {
          let decision = 'deny';
          try { ({ decision } = await request('permission.request', { id: p.id, toolCallId: p.tool?.callID,
            toolName: p.permission, title: p.patterns?.join(', '), input: p.metadata })); } catch { /* cancellation denies */ }
          if (alive()) await http(`/permission/${enc(p.id)}/reply`, { method: 'POST', body: {
            reply: decision === 'allow_once' ? 'once' : decision === 'allow_session' ? 'always' : 'reject' } });
        } else {
          const questions = p.questions.map((q, i) => ({ ...q, id: String(i), custom: q.custom !== false }));
          let reply;
          try { reply = await request('question.request', { id: p.id, toolCallId: p.tool?.callID, questions }); }
          catch { if (alive()) await http(`/question/${enc(p.id)}/reject`, { method: 'POST', body: {} }); return; }
          if (alive()) await http(`/question/${enc(p.id)}/reply`, { method: 'POST', body: { answers: questions.map(q => reply.answers[q.id] ?? []) } });
        }
      })().catch(e => turn.fail(e));
    }
    const waiting = new Map();
    async function event(raw) {
      if (turn.finished) return;
      if (raw.directory !== undefined && resolve(raw.directory) !== cwd) return;
      const e = raw.payload ?? raw, p = e.properties ?? {};
      if (e.type === 'session.created' || e.type === 'session.updated') {
        const s = p.info;
        if (s?.directory && resolve(s.directory) === cwd && turn.sessions.has(s.parentID) && !turn.sessions.has(s.id)) {
          const scope = { id: s.id, sessionId: s.id, parentId: s.parentID, title: s.title };
          turn.sessions.set(s.id, scope);
          emit({ type: 'child.updated', sessionId: s.id, scope, status: 'running' });
        }
        return;
      }
      const sid = p.sessionID ?? p.info?.sessionID ?? p.part?.sessionID;
      if (!turn.sessions.has(sid)) return;
      const scope = turn.sessions.get(sid);
      const mid = p.messageID ?? p.info?.id ?? p.part?.messageID;
      if (mid && old.has(mid)) return;
      if (e.type === 'message.updated') {
        const m = p.info;
        turn.roles.set(m.id, m.role);
        if (m.role === 'user') { if (m.id === turn.promptID) turn.admitted = true; return; }
        if (sid === sessionId && m.parentID !== turn.promptID) { old.add(m.id); waiting.delete(m.id); return; }
        turn.admitted = true;
        turn.project.message(m, scope);
        if (m.error) turn.error = m.error;
        const buffered = waiting.get(m.id) ?? [];
        waiting.delete(m.id);
        for (const item of buffered) await event(item);
      } else if (e.type === 'message.part.updated') {
        if (turn.roles.get(mid) === 'user' || mid === turn.promptID) return;
        if (!turn.roles.has(mid)) { const q = waiting.get(mid) ?? []; q.push(raw); waiting.set(mid, q); return; }
        turn.project.part(p.part, scope);
      } else if (e.type === 'message.part.delta') {
        if (!turn.roles.has(mid)) { const q = waiting.get(mid) ?? []; q.push(raw); waiting.set(mid, q); return; }
        if (turn.roles.get(mid) !== 'user' && mid !== turn.promptID) turn.project.delta(p, scope);
      } else if (e.type === 'permission.asked' || e.type === 'question.asked') interaction(e.type, p);
      else if (e.type === 'todo.updated') emit({ type: 'plan.updated', sessionId: sid, scope,
        entries: p.todos.map(t => ({ content: t.content, status: t.status, priority: t.priority })) });
      else if (e.type === 'session.error') { turn.error = p.error; turn.admitted = true; }
      else if (e.type === 'session.status' || e.type === 'session.idle') {
        const status = p.status?.type ?? 'idle';
        if (scope) emit({ type: 'child.updated', sessionId: sid, scope, status: status === 'idle' ? 'completed' : 'running' });
        else {
          emit({ type: 'runtime.status', sessionId, status, message: p.status?.message });
          if (status === 'idle' && turn.interrupting) turn.idle = true;
          else if (status === 'idle' && turn.accepted && turn.admitted) await finish();
          else if (status === 'idle') turn.idle = true;
          else turn.idle = false;
        }
      }
    }
    try {
      // Await response headers before POST; no token can precede our subscription.
      const stream = await http('/event', { stream: true, signal: streamAbort.signal });
      const pump = (async () => {
        const decoder = new TextDecoder(); let buffer = '';
        for await (const chunk of stream.body) {
          buffer += decoder.decode(chunk, { stream: true });
          buffer = buffer.replace(/\r\n/g, '\n');
          let end;
          while ((end = buffer.indexOf('\n\n')) >= 0) {
            const frame = buffer.slice(0, end); buffer = buffer.slice(end + 2);
            const data = frame.split('\n').filter(l => l.startsWith('data:')).map(l => l.slice(5).trimStart()).join('\n');
            if (data) {
              const raw = JSON.parse(data), id = (raw.payload ?? raw).id;
              if (id && turn.seenEvents.has(id)) continue;
              if (id) turn.seenEvents.add(id);
              await event(raw);
            }
          }
          if (buffer.length > 8 * 1024 * 1024) throw new Error('OpenCode SSE frame too large');
        }
        if (!turn.finished) throw new Error('OpenCode SSE disconnected before terminal status');
      })().catch(e => { if (!turn.finished) turn.fail(e); });
      const payload = { messageID: turn.promptID, parts: [{ type: 'text', text }] };
      if (effective.model) { const i = effective.model.indexOf('/'); payload.model = { providerID: effective.model.slice(0, i), modelID: effective.model.slice(i + 1) }; }
      Object.assign(payload, effective.options);
      await http(`/session/${enc(sessionId)}/prompt_async`, { method: 'POST', body: payload, signal: streamAbort.signal });
      turn.accepted = true;
      selections.set(sessionId, effective);
      emit({ type: 'conversation.updated', sessionId, selection: effective });
      if (turn.idle && turn.admitted) await finish();
      const result = await done;
      await pump;
      return result;
    } finally { streamAbort.abort(); if (active === turn) active = undefined; }
  }
  async function kill() {
    if (!child || child.exitCode !== null || child.signalCode !== null) return;
    const signal = s => { try { if (process.platform !== 'win32') process.kill(-child.pid, s); else child.kill(s); } catch {} };
    const exited = new Promise(res => child.once('exit', res));
    signal('SIGTERM');
    let timer;
    await Promise.race([exited, new Promise(res => { timer = setTimeout(res, 1000); })]);
    clearTimeout(timer);
    if (child.exitCode === null && child.signalCode === null) { signal('SIGKILL'); await Promise.race([exited, delay(1000)]); }
  }
  return {
    describe,
    async open({ sessionId, selection } = {}) {
      await ready();
      const effective = await selectionFor(selection);
      const s = sessionId ? await verify(sessionId) : await http('/session', { method: 'POST', body: { permission } });
      if (!s.id || !s.directory || resolve(s.directory) !== cwd) throw new Error('OpenCode did not confirm session directory');
      if (sessionId) {
        const updated = await http(`/session/${enc(sessionId)}`, { method: 'PATCH', body: { permission } });
        if (updated.id !== sessionId || !updated.directory || resolve(updated.directory) !== cwd) throw new Error('OpenCode did not confirm session policy');
      }
      selections.set(s.id, effective);
      return { sessionId: s.id, capabilities, ...(effective.model || Object.keys(effective.options ?? {}).length ? { pendingSelection: effective } : {}) };
    },
    async start(params) {
      if (starting || active) throw new Error('OpenCode turn already active');
      starting = true;
      try { return await start(params); } finally { starting = false; }
    },
    async configure({ sessionId, selection }) {
      await ready(); await verify(sessionId);
      if (active || starting) throw new Error('Cannot configure during an OpenCode turn');
      const effective = await selectionFor(selection, selections.get(sessionId));
      // Native prompt API applies these per-turn; never mutate host config/defaults.
      selections.set(sessionId, effective); return { applied: 'next-turn', selection: effective };
    },
    async interrupt({ sessionId }) {
      await ready(); await verify(sessionId);
      const turn = active;
      if (!turn || turn.sessionId !== sessionId) throw new Error('No active OpenCode turn for session');
      turn.interrupting = true;
      try {
        const ok = await http(`/session/${enc(sessionId)}/abort`, { method: 'POST', body: {} });
        if (ok !== true) throw new Error('OpenCode did not acknowledge abort');
        const deadline = Date.now() + 5000;
        while (Date.now() < deadline) {
          const statuses = await http('/session/status');
          if (!statuses[sessionId] || statuses[sessionId].type === 'idle') {
            await turn.finish(true); return { interrupted: true };
          }
          await delay(50);
        }
        throw new Error('OpenCode abort not confirmed idle');
      } finally {
        turn.interrupting = false;
        if (turn.idle && turn.accepted && turn.admitted && !turn.finished) await turn.finish();
      }
    },
    async list({ cwd: directory = cwd, cursor } = {}) {
      await ready(); if (cursor) throw new Error('OpenCode session list has no cursor support');
      const sessions = await http('/session', { directory });
      return { sessions: sessions.filter(s => s.directory && resolve(s.directory) === resolve(directory)).map(s => ({
        sessionId: s.id, cwd: s.directory, title: s.title, updatedAt: s.time?.updated })) };
    },
    async history({ sessionId, cwd: directory = cwd }) {
      await ready(); const s = await verify(sessionId, directory);
      const rows = await http(`/session/${enc(sessionId)}/message`, { directory });
      const messages = rows.map(m => {
        const events = [], p = projector(e => events.push(e));
        p.message(m.info);
        for (const part of m.parts) p.part(part, undefined, true);
        p.finish(); return { id: m.info.id, role: m.info.role, events };
      });
      return { sessionId, cwd: s.directory, messages, complete: true };
    },
    async close() {
      if (closed) return;
      closed = true; active?.fail(new Error('OpenCode runtime closed')); lifetime.abort(); await kill();
    },
  };
}