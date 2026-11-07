// Claude Agent SDK 0.2.76. The host injects the managed SDK; no provider credentials
// are read here. A fresh session ID is null until Claude emits its first message.
import { resolve } from 'node:path';
function deferred() {
  let resolve, reject;
  const promise = new Promise((a, b) => { resolve = a; reject = b; });
  return { promise, resolve, reject };
}

function inbox() {
  const values = [];
  let wake, ended = false;
  return {
    push(value) { values.push(value); wake?.(); },
    end() { ended = true; wake?.(); },
    async *[Symbol.asyncIterator]() {
      while (!ended) {
        if (values.length) yield values.shift();
        else await new Promise(resolve => { wake = resolve; });
      }
    },
  };
}

const outputText = content => typeof content === 'string' ? content :
  (content ?? []).map(block => block.type === 'text' ? block.text : JSON.stringify(block)).join('\n');
const usageOf = u => ({ input: u?.input_tokens ?? 0, output: u?.output_tokens ?? 0,
  cacheRead: u?.cache_read_input_tokens ?? 0, cacheWrite: u?.cache_creation_input_tokens ?? 0 });

// This projector is used for both live SDK messages and native transcript import.
function projector(emit, sessionId) {
  const streams = new Map(), blocks = new Map(), tools = new Map(), children = new Map();
  const send = (event, scope) => emit({ ...event, sessionId, ...(scope ? { scope } : {}) });
  function child(id, parentToolCallId) {
    if (!children.has(id)) children.set(id, { id, ...(parentToolCallId ? { parentToolCallId } : {}) });
    return children.get(id);
  }
  function startTool(block, scope) {
    if (!tools.has(block.id)) {
      tools.set(block.id, { terminal: false, scope });
      send({ type: 'tool.started', id: block.id, name: block.name, input: block.input ?? {} }, scope);
    }
    return tools.get(block.id);
  }
  return message => {
    sessionId = message.session_id ?? sessionId;
    const scope = message.parent_tool_use_id ? child(message.parent_tool_use_id, message.parent_tool_use_id) : undefined;
    const streamKey = message.parent_tool_use_id ?? 'root';
    if (message.type === 'stream_event') {
      const e = message.event;
      if (e.type === 'message_start') streams.set(streamKey, e.message.id);
      const mid = streams.get(streamKey);
      const key = `${mid}:${e.index}`;
      if (e.type === 'content_block_start') {
        const b = { ...e.content_block, id: e.content_block.id ?? key, text: e.content_block.text ?? e.content_block.thinking ?? '', json: '', scope };
        blocks.set(key, b);
        if (b.type === 'tool_use') startTool(b, scope);
        else if (b.text && ['text', 'thinking'].includes(b.type)) send({ type: b.type === 'text' ? 'text.delta' : 'reasoning.delta', id: b.id, text: b.text }, scope);
      } else if (e.type === 'content_block_delta') {
        const b = blocks.get(key);
        if (!b) return;
        if (e.delta.type === 'input_json_delta') {
          b.json += e.delta.partial_json;
          let input;
          try { input = JSON.parse(b.json); } catch { /* sparse JSON is metadata, not fabricated input */ }
          send({ type: 'tool.updated', id: b.id, ...(input === undefined ? {} : { input }), metadata: { inputJson: b.json, inputJsonDelta: e.delta.partial_json } }, scope);
        } else if (['text_delta', 'thinking_delta'].includes(e.delta.type)) {
          const text = e.delta.text ?? e.delta.thinking;
          b.text += text;
          send({ type: b.type === 'thinking' ? 'reasoning.delta' : 'text.delta', id: b.id, text }, scope);
        }
      } else if (e.type === 'content_block_stop') {
        const b = blocks.get(key);
        if (b && !b.done && ['text', 'thinking'].includes(b.type)) {
          b.done = true;
          send({ type: b.type === 'text' ? 'text.completed' : 'reasoning.completed', id: b.id, text: b.text }, scope);
        }
      }
    } else if (message.type === 'assistant' || message.type === 'user') {
      const content = message.message?.content;
      if (typeof content === 'string') {
        send({ type: 'text.completed', id: message.uuid, text: content }, scope);
        return;
      }
      for (const [index, b] of (content ?? []).entries()) {
        const key = `${message.message.id ?? message.uuid}:${index}`;
        const prior = blocks.get(key);
        if (b.type === 'text' || b.type === 'thinking') {
          if (!prior?.done) send({ type: b.type === 'text' ? 'text.completed' : 'reasoning.completed', id: prior?.id ?? key, text: b.text ?? b.thinking }, scope);
        } else if (b.type === 'tool_use') {
          const tool = startTool(b, scope);
          if (!tool.terminal) send({ type: 'tool.updated', id: b.id, input: b.input }, scope);
        } else if (b.type === 'tool_result') {
          // Native history may omit the assistant tool-use record. Do not invent a name.
          const tool = tools.get(b.tool_use_id);
          if (tool?.terminal) continue;
          const text = outputText(b.content);
          const toolScope = tool?.scope ?? scope;
          if (!tool) startTool({ id: b.tool_use_id, name: 'unknown', input: {} }, toolScope);
          tools.get(b.tool_use_id).terminal = true;
          send({ type: 'tool.output', id: b.tool_use_id, text, mode: 'replace' }, toolScope);
          send(b.is_error ? { type: 'tool.failed', id: b.tool_use_id, error: text, output: text } :
            { type: 'tool.completed', id: b.tool_use_id, output: text, outputMode: 'replace' }, toolScope);
        }
      }
    } else if (message.type === 'tool_progress') {
      const tool = tools.get(message.tool_use_id);
      if (!tool?.terminal) {
        startTool({ id: message.tool_use_id, name: message.tool_name }, scope);
        send({ type: 'tool.updated', id: message.tool_use_id, metadata: { elapsedSeconds: message.elapsed_time_seconds } }, scope);
      }
    } else if (message.type === 'system') {
      if (message.subtype === 'init') send({ type: 'model.updated', model: message.model });
      else if (message.subtype.startsWith('task_')) {
        const s = child(message.task_id, message.tool_use_id);
        send({ type: 'child.updated', scope: s, status: message.status === 'failed' ? 'error' : message.status === 'stopped' ? 'interrupted' : message.status === 'completed' ? 'completed' : 'running', summary: message.summary ?? message.description }, s);
      } else if (message.subtype === 'status') send({ type: 'runtime.status', status: message.status ?? 'ready' });
    } else if (message.type === 'result') send({ type: 'usage.updated', usage: usageOf(message.usage) });
  };
}

export function createAdapter(context) {
  const sdk = context.sdk;
  if (typeof sdk?.query !== 'function') throw new Error('Claude Agent SDK module with query() is required');
  let runtime, active, sessionId = null, expectedSession, model, models = [], commands = [], account;
  let selection = { options: {} }, closed = false, opening;
  const toolOwners = new Map(), messageOwners = new Map(), taskOwners = new Map(), streamOwners = new Map();
  const capabilities = () => ({ resume: typeof sdk.getSessionInfo === 'function',
    history: typeof sdk.getSessionMessages === 'function' && typeof sdk.getSessionInfo === 'function',
    completeHistory: false,
    list: typeof sdk.listSessions === 'function', modelSelection: true, steer: false,
    toolStreaming: true, reasoning: true, nativeChildren: 'observed' });
  const check = () => { if (closed) throw new Error('Claude adapter is closed'); };
  function ownerFor(m) {
    const mid = m.type === 'stream_event' ? m.event.message?.id : m.message?.id;
    const streamKey = m.parent_tool_use_id ?? 'root';
    if (m.type === 'stream_event' && m.event.type === 'message_start') streamOwners.set(streamKey, active);
    const owner = toolOwners.get(m.parent_tool_use_id ?? m.tool_use_id) ?? taskOwners.get(m.task_id) ?? messageOwners.get(mid) ??
      (m.type === 'stream_event' ? streamOwners.get(streamKey) : undefined) ?? active;
    if (owner && mid) messageOwners.set(mid, owner);
    if (owner && m.task_id) taskOwners.set(m.task_id, owner);
    if (owner && m.parent_tool_use_id) toolOwners.set(m.parent_tool_use_id, owner);
    if (owner && m.tool_use_id) toolOwners.set(m.tool_use_id, owner);
    if (owner && m.type === 'stream_event' && m.event.content_block?.type === 'tool_use') toolOwners.set(m.event.content_block.id, owner);
    if (owner && m.type === 'assistant') for (const b of m.message.content ?? []) if (b.type === 'tool_use') toolOwners.set(b.id, owner);
    return owner;
  }
  async function canUseTool(name, input, options) {
    const turn = toolOwners.get(options.toolUseID) ?? active;
    if (!turn || turn.done) return { behavior: 'deny', message: 'No active Rust-owned turn' };
    toolOwners.set(options.toolUseID, turn);
    const signal = options.signal;
    const invoke = async () => {
      if (name === 'AskUserQuestion') {
        const questions = (input.questions ?? []).map((q, i) => ({ id: String(i), question: q.question, header: q.header,
          options: q.options, multiple: q.multiSelect ?? false, custom: true }));
        const response = await turn.request('question.request', { id: options.toolUseID, toolCallId: options.toolUseID, questions });
        const answers = {};
        for (const q of questions) {
          if (!Array.isArray(response.answers?.[q.id])) throw new Error(`Missing answer for ${q.id}`);
          answers[q.question] = response.answers[q.id].join(', ');
        }
        return { behavior: 'allow', updatedInput: { ...input, answers } };
      }
      const response = await turn.request('permission.request', { id: options.toolUseID, toolCallId: options.toolUseID,
        toolName: name, title: options.decisionReason, input });
      // Rust owns session grants. Never install SDK-wide allow rules or change mode.
      return ['allow_once', 'allow_session'].includes(response.decision) ? { behavior: 'allow', updatedInput: input } :
        { behavior: 'deny', message: 'Denied by inherited Neoism policy' };
    };
    let onAbort;
    try {
      if (signal?.aborted) throw new Error('Tool request cancelled');
      const cancelled = new Promise((_, reject) => {
        onAbort = () => reject(new Error('Tool request cancelled'));
        signal?.addEventListener('abort', onAbort, { once: true });
      });
      const result = await Promise.race([invoke(), cancelled]);
      if (turn.done || signal?.aborted) return { behavior: 'deny', message: 'Tool request cancelled' };
      return result;
    } catch (error) { return { behavior: 'deny', message: error.message }; }
    finally { signal?.removeEventListener('abort', onAbort); }
  }
  async function boot(resume) {
    const queue = inbox(), controller = new AbortController();
    const options = { cwd: context.cwd, includePartialMessages: true, abortController: controller,
      permissionMode: 'default', canUseTool,
      // Force a Rust policy check even for tools native permission rules auto-allow.
      hooks: { PreToolUse: [{ hooks: [async () => ({ hookSpecificOutput: { hookEventName: 'PreToolUse', permissionDecision: 'ask' } })] }] },
      ...(context.executable ? { pathToClaudeCodeExecutable: context.executable } : {}),
      ...(resume ? { resume } : {}), ...(selection.model ? { model: selection.model } : {}),
      ...(selection.options.effort ? { effort: selection.options.effort } : {}) };
    const query = sdk.query({ prompt: queue, options });
    const state = { query, queue, controller, stopping: false };
    runtime = state;
    state.pump = (async () => {
      try {
        for await (const m of query) {
          if (m.session_id && !m.parent_tool_use_id) {
            if (expectedSession && expectedSession !== m.session_id) throw new Error(`Claude resumed a different session: ${m.session_id}`);
            sessionId = m.session_id;
            expectedSession = sessionId;
          }
          const turn = ownerFor(m);
          if (!turn) continue;
          if (sessionId && !m.parent_tool_use_id && !turn.identityAnnounced) {
            turn.identityAnnounced = true;
            turn.emit({ type: 'conversation.updated', sessionId,
              ...(m.type === 'system' && m.subtype === 'init' ? { model: m.model } : {}) });
          }
          turn.project(m);
          if (m.type === 'system' && m.subtype === 'init') model = m.model;
          if (m.type === 'assistant' && m.error) throw new Error(`Claude Code ${m.error}; authentication remains native-owned`);
          if (m.type === 'auth_status' && m.error) throw new Error(`Claude Code authentication: ${m.error}`);
          if (m.type === 'result' && !m.parent_tool_use_id && !turn.done) {
            turn.done = true;
            if (active === turn) active = null;
            if (m.is_error || m.subtype !== 'success') turn.reject(new Error(`Claude Code ${m.subtype}: ${(m.errors ?? []).join('; ')}`));
            else Promise.resolve(turn.interruption).then(() => turn.resolve({ finish: turn.interrupted ? 'interrupted' : 'completed', model, usage: usageOf(m.usage) }), turn.reject);
          }
        }
        if (!state.stopping) throw new Error('Claude SDK stream ended before adapter close');
      } catch (error) {
        state.error = error;
        if (active) { active.done = true; active.reject(error); active = null; }
        state.queue.end();
        state.controller.abort();
        state.query.close();
      }
    })();
    try {
      const init = await query.initializationResult();
      models = init.models ?? [];
      commands = init.commands ?? [];
      account = init.account;
      if (state.error) throw state.error;
    } catch (error) { await shutdown(); throw error; }
  }
  async function shutdown() {
    const state = runtime;
    if (!state) return;
    runtime = undefined;
    state.stopping = true;
    state.queue.end();
    state.controller.abort();
    try { await state.query.close(); } finally { await state.pump; }
  }
  async function ensure() {
    check();
    if (opening) await opening;
    if (!runtime) {
      opening ??= boot(expectedSession).finally(() => { opening = undefined; });
      await opening;
    }
    check();
    if (runtime?.error) throw runtime.error;
  }
  async function info(id, cwd = context.cwd) {
    if (typeof sdk.getSessionInfo !== 'function') throw new Error('Native session validation is unavailable in this SDK');
    const value = await sdk.getSessionInfo(id, { dir: cwd });
    if (!value || value.sessionId !== id) throw new Error(`Claude session not found: ${id}`);
    if (typeof value.cwd !== 'string' || resolve(value.cwd) !== resolve(cwd)) throw new Error('Claude native session cwd mismatch or unavailable');
    return value;
  }
  async function configure({ sessionId: id, selection: next = {} } = {}) {
    await ensure();
    if (active) throw new Error('Cannot configure Claude during an active turn');
    if (id && id !== sessionId && id !== expectedSession) throw new Error('Claude session mismatch');
    for (const key of Object.keys(next.options ?? {})) if (key !== 'effort') throw new Error(`Unsupported Claude option: ${key}`);
    const nextModel = next.model ?? selection.model;
    const entry = models.find(m => m.value === (nextModel ?? 'default'));
    if (nextModel && !entry) throw new Error(`Unsupported Claude model: ${nextModel}`);
    const effort = next.options?.effort ?? selection.options.effort;
    if (effort && !entry?.supportedEffortLevels?.includes(effort)) throw new Error(`Unsupported Claude effort ${effort} for ${nextModel ?? 'default'}`);
    if (nextModel !== selection.model) await runtime.query.setModel(nextModel);
    const changedEffort = effort !== selection.options.effort;
    const previous = selection;
    selection = { ...(nextModel ? { model: nextModel } : {}), options: effort ? { effort } : {} };
    if (changedEffort) {
      // SDK has no live setEffort control. Reinitialize at the native persisted session.
      await shutdown();
      try { await boot(sessionId ?? expectedSession); } catch (error) { selection = previous; throw error; }
    }
    return { ...selection, options: { ...selection.options } };
  }
  return {
    async describe({ selection: requested = selection } = {}) {
      await ensure();
      const entry = models.find(m => m.value === (requested.model ?? 'default'));
      if (requested.model && !entry) throw new Error(`Unsupported Claude model: ${requested.model}`);
      return { capabilities: capabilities(), models: models.map(m => ({ id: m.value, name: m.displayName })),
        options: entry?.supportedEffortLevels?.length ? [{ id: 'effort', name: 'Effort', type: 'select', currentValue: selection.options.effort ?? 'high',
          options: entry.supportedEffortLevels.map(value => ({ value, name: value })) }] : [],
        commands, auth: { owner: 'native', ...(account ? { account } : {}) } };
    },
    async open({ sessionId: id, selection: next } = {}) {
      check();
      if (active) throw new Error('Claude turn is active');
      if (id && id !== sessionId) {
        await info(id);
        await shutdown();
        expectedSession = id;
        sessionId = id;
      }
      await ensure();
      if (next) await configure({ selection: next });
      return { sessionId, ...(model ? { model } : {}), options: { ...selection.options }, capabilities: capabilities() };
    },
    async start({ sessionId: id, text, selection: next, emit, request }) {
      await ensure();
      if (active) throw new Error('Claude turn is already active');
      if (id && id !== sessionId) throw new Error('Open the requested Claude session before starting');
      if (next) await configure({ sessionId: id, selection: next });
      if (typeof emit !== 'function' || typeof request !== 'function') throw new Error('Turn-bound emit and request callbacks are required');
      const done = deferred();
      const turn = { ...done, emit, request, done: false, interrupted: false, project: projector(emit, sessionId) };
      active = turn;
      runtime.queue.push({ type: 'user', session_id: sessionId ?? '', parent_tool_use_id: null, message: { role: 'user', content: text } });
      return done.promise;
    },
    async interrupt({ sessionId: id } = {}) {
      check();
      if (id && id !== sessionId) throw new Error('Claude session mismatch');
      const turn = active;
      if (!turn) return { interrupted: false };
      turn.interruption = runtime.query.interrupt().then(() => { turn.interrupted = true; });
      await turn.interruption;
      return { interrupted: true };
    },
    configure,
    async list({ cwd = context.cwd, cursor } = {}) {
      check();
      if (typeof sdk.listSessions !== 'function') throw new Error('Claude session listing is unsupported');
      const offset = cursor === undefined ? 0 : Number(cursor);
      if (!Number.isSafeInteger(offset) || offset < 0) throw new Error('Invalid Claude session cursor');
      const rows = await sdk.listSessions({ dir: cwd, includeWorktrees: false, offset, limit: 100 });
      return { sessions: rows.map(s => ({ sessionId: s.sessionId, ...(s.cwd ? { cwd: s.cwd } : {}), title: s.customTitle ?? s.summary, updatedAt: s.lastModified })),
        ...(rows.length === 100 ? { nextCursor: String(offset + rows.length) } : {}) };
    },
    async history({ sessionId: id, cwd = context.cwd }) {
      check();
      if (!capabilities().history) throw new Error('Claude native history is unsupported');
      const metadata = await info(id, cwd);
      const messages = [];
      let events;
      const project = projector(e => events.push(e), id);
      const rows = await sdk.getSessionMessages(id, { dir: cwd });
      for (const row of rows) {
        events = [];
        project(row);
        messages.push({ id: row.uuid, role: row.type, events });
      }
      // SDK history exposes user/assistant transcript only, not every system/task event.
      return { sessionId: id, ...(metadata.cwd ? { cwd: metadata.cwd } : {}), messages, complete: false };
    },
    async close() {
      if (closed) return;
      closed = true;
      if (active) { active.done = true; active.reject(new Error('Claude adapter closed')); active = null; }
      await shutdown();
      toolOwners.clear(); messageOwners.clear(); taskOwners.clear(); streamOwners.clear();
    },
  };
}