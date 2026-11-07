import test from 'node:test';
import assert from 'node:assert/strict';
import { createAdapter } from './claude.mjs';

const sid = '5c16dc19-336b-4494-8fa9-9d002f60e125';
const tick = () => new Promise(resolve => setImmediate(resolve));
function channel() {
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
function harness(overrides = {}) {
  const queries = [], interactions = [], events = [], historyCalls = [];
  const sdk = {
    query(args) {
      const output = channel();
      const q = {
        args, output, inputs: [], closeCount: 0, interruptCount: 0, modelChanges: [],
        initializationResult: async () => ({ models: [
          { value: 'default', displayName: 'Default', supportedEffortLevels: ['low', 'medium', 'high'] },
          { value: 'opus', displayName: 'Opus', supportedEffortLevels: ['low', 'medium', 'high', 'max'] },
          { value: 'haiku', displayName: 'Haiku' },
        ], commands: [{ name: 'compact', description: 'Compact' }], account: { tokenSource: 'none' } }),
        async setModel(value) { q.modelChanges.push(value); },
        async interrupt() { q.interruptCount++; },
        close() { q.closeCount++; output.end(); },
        [Symbol.asyncIterator]: () => output[Symbol.asyncIterator](),
      };
      q.inputPump = (async () => { for await (const m of args.prompt) q.inputs.push(m); })();
      queries.push(q);
      return q;
    },
    async getSessionInfo(id, options) { return id === sid ? { sessionId: sid, cwd: '/work', summary: 'Native title' } : undefined; },
    async listSessions(options) { return [{ sessionId: sid, cwd: '/native', summary: 'Native title', lastModified: 123 }, { sessionId: 'no-cwd', summary: 'Unknown workspace' }]; },
    async getSessionMessages(id, options) { historyCalls.push({ id, options }); return []; },
    ...overrides,
  };
  const adapter = createAdapter({ cwd: '/work', executable: '/installed/claude', sdk,
    emit() { throw new Error('Turn events must not use context.emit'); }, request() { throw new Error('Turn interactions must not use context.request'); } });
  const start = (extra = {}) => adapter.start({ text: 'hello', emit: e => events.push(e), request: async (method, params) => {
    interactions.push({ method, params }); return { decision: 'allow_once' };
  }, ...extra });
  const send = m => queries.at(-1).output.push({ session_id: sid, parent_tool_use_id: null, ...m });
  const partial = (event, parent = null) => send({ type: 'stream_event', event, parent_tool_use_id: parent });
  const result = (extra = {}) => send({ type: 'result', subtype: 'success', is_error: false,
    usage: { input_tokens: 10, output_tokens: 5, cache_read_input_tokens: 3, cache_creation_input_tokens: 2 }, ...extra });
  return { sdk, adapter, queries, events, interactions, historyCalls, start, send, partial, result };
}

test('describe initializes without sending a turn; fresh identity is deferred, native executable and safe policy are preserved', async t => {
  const h = harness(); t.after(() => h.adapter.close());
  const d = await h.adapter.describe();
  assert.equal(d.capabilities.nativeChildren, 'observed');
  assert.equal(d.capabilities.steer, false);
  assert.deepEqual(d.models.map(m => m.id), ['default', 'opus', 'haiku']);
  assert.equal(d.auth.owner, 'native');
  assert.equal((await h.adapter.open()).sessionId, null);
  const q = h.queries[0];
  assert.equal(q.inputs.length, 0);
  assert.equal(q.args.options.pathToClaudeCodeExecutable, '/installed/claude');
  assert.equal(q.args.options.includePartialMessages, true);
  assert.equal(q.args.options.permissionMode, 'default');
  assert.equal(q.args.options.allowDangerouslySkipPermissions, undefined);
  assert.equal(q.args.options.env, undefined);
  const hook = q.args.options.hooks.PreToolUse[0].hooks[0];
  assert.equal((await hook()).hookSpecificOutput.permissionDecision, 'ask');
});

test('projects real partial text, thinking, tool JSON and final user tool result without duplication', async t => {
  const h = harness(); t.after(() => h.adapter.close());
  await h.adapter.open();
  const turn = h.start(); await tick();
  h.send({ type: 'system', subtype: 'init', model: 'claude-sonnet-4-6' });
  h.partial({ type: 'message_start', message: { id: 'msg-1', role: 'assistant', content: [] } });
  h.partial({ type: 'content_block_start', index: 0, content_block: { type: 'thinking', thinking: '', signature: '' } });
  h.partial({ type: 'content_block_delta', index: 0, delta: { type: 'thinking_delta', thinking: 'Consider' } });
  h.partial({ type: 'content_block_stop', index: 0 });
  h.partial({ type: 'content_block_start', index: 1, content_block: { type: 'text', text: '' } });
  h.partial({ type: 'content_block_delta', index: 1, delta: { type: 'text_delta', text: 'Hello ' } });
  h.partial({ type: 'content_block_delta', index: 1, delta: { type: 'text_delta', text: 'world' } });
  h.partial({ type: 'content_block_stop', index: 1 });
  h.partial({ type: 'content_block_start', index: 2, content_block: { type: 'tool_use', id: 'tool-1', name: 'Bash', input: {} } });
  h.partial({ type: 'content_block_delta', index: 2, delta: { type: 'input_json_delta', partial_json: '{"command":' } });
  h.partial({ type: 'content_block_delta', index: 2, delta: { type: 'input_json_delta', partial_json: '"pwd"}' } });
  h.partial({ type: 'content_block_stop', index: 2 });
  h.send({ type: 'assistant', uuid: 'a-1', message: { id: 'msg-1', role: 'assistant', content: [
    { type: 'thinking', thinking: 'Consider' }, { type: 'text', text: 'Hello world' },
    { type: 'tool_use', id: 'tool-1', name: 'Bash', input: { command: 'pwd' } },
  ] } });
  h.send({ type: 'user', uuid: 'u-1', message: { role: 'user', content: [{ type: 'tool_result', tool_use_id: 'tool-1', content: [{ type: 'text', text: '/work' }] }] } });
  h.send({ type: 'tool_progress', tool_use_id: 'tool-1', tool_name: 'Bash', elapsed_time_seconds: 1 });
  h.result();
  const completed = await turn;
  assert.equal(completed.finish, 'completed');
  assert.equal(completed.model, 'claude-sonnet-4-6');
  assert.deepEqual(completed.usage, { input: 10, output: 5, cacheRead: 3, cacheWrite: 2 });
  assert.deepEqual(h.events.filter(e => e.type === 'text.delta').map(e => e.text), ['Hello ', 'world']);
  assert.equal(h.events.filter(e => e.type === 'text.completed').length, 1);
  assert.equal(h.events.filter(e => e.type === 'reasoning.completed').length, 1);
  assert.equal(h.events.filter(e => e.type === 'tool.started').length, 1);
  const updates = h.events.filter(e => e.type === 'tool.updated');
  assert.equal(updates[0].input, undefined);
  assert.equal(updates[0].metadata.inputJsonDelta, '{"command":');
  assert.deepEqual(updates[1].input, { command: 'pwd' });
  assert.deepEqual(h.events.filter(e => e.type === 'tool.output').map(e => [e.text, e.mode]), [['/work', 'replace']]);
  assert.equal(h.events.at(-2).type, 'tool.completed');
  assert.equal(h.events.at(-1).type, 'usage.updated');
  assert.equal((await h.adapter.open()).sessionId, sid);
  assert.equal(h.queries.length, 1);
});

test('canUseTool relays allow/deny and AskUserQuestion answers to the turn-bound Rust callback', async t => {
  const h = harness(); t.after(() => h.adapter.close());
  await h.adapter.open();
  const calls = [];
  const turn = h.start({ request: async (method, params) => {
    calls.push({ method, params });
    if (method === 'question.request') return { answers: { 0: ['Rust', 'Other'] } };
    return { decision: params.toolCallId === 'deny' ? 'deny' : 'allow_session' };
  } });
  await tick();
  const permit = h.queries[0].args.options.canUseTool;
  const options = id => ({ toolUseID: id, signal: new AbortController().signal });
  assert.deepEqual(await permit('Bash', { command: 'pwd' }, options('allow')), { behavior: 'allow', updatedInput: { command: 'pwd' } });
  assert.equal((await permit('Bash', {}, options('deny'))).behavior, 'deny');
  const question = { questions: [{ question: 'Language?', header: 'Language', multiSelect: true, options: [{ label: 'Rust', description: 'Native' }] }] };
  const answer = await permit('AskUserQuestion', question, options('q1'));
  assert.deepEqual(answer.updatedInput.answers, { 'Language?': 'Rust, Other' });
  assert.equal(calls[2].method, 'question.request');
  assert.equal(calls[2].params.questions[0].multiple, true);
  assert.equal(calls[0].params.toolName, 'Bash');
  h.result(); await turn;
  assert.equal((await permit('Bash', {}, options('late'))).behavior, 'deny');
});

test('question cancellation and native permission signal cancellation fail closed', async t => {
  const h = harness(); t.after(() => h.adapter.close());
  await h.adapter.open();
  const turn = h.start({ request: async method => {
    if (method === 'question.request') throw new Error('Question cancelled');
    return new Promise(() => {});
  } }); await tick();
  const permit = h.queries[0].args.options.canUseTool;
  assert.equal((await permit('AskUserQuestion', { questions: [] }, { toolUseID: 'q', signal: new AbortController().signal })).behavior, 'deny');
  const abort = new AbortController();
  const pending = permit('Bash', {}, { toolUseID: 'b', signal: abort.signal });
  abort.abort();
  assert.equal((await pending).behavior, 'deny');
  h.result(); await turn;
});

test('resume uses validated native identity and SDK resume, never silently forks on failure', async t => {
  const h = harness(); t.after(() => h.adapter.close());
  await assert.rejects(h.adapter.open({ sessionId: 'missing' }), /not found/);
  assert.equal(h.queries.length, 0);
  assert.equal((await h.adapter.open({ sessionId: sid })).sessionId, sid);
  assert.equal(h.queries[0].args.options.resume, sid);
  const turn = h.start({ sessionId: sid }); await tick();
  const failure = assert.rejects(turn, /different session/);
  h.send({ type: 'system', subtype: 'init', model: 'sonnet', session_id: 'wrong' });
  await failure;
  assert.equal(h.queries[0].args.options.abortController.signal.aborted, true);
});

test('warm query across turns; late native child output remains bound to the originating turn', async t => {
  const h = harness(); t.after(() => h.adapter.close());
  await h.adapter.open();
  const first = [], second = [];
  const a = h.start({ emit: e => first.push(e) }); await tick();
  h.send({ type: 'system', subtype: 'task_started', task_id: 'native-task', tool_use_id: 'parent-tool', description: 'Explore' });
  h.send({ type: 'assistant', uuid: 'child', parent_tool_use_id: 'parent-tool', message: { id: 'child-msg', content: [{ type: 'tool_use', id: 'child-tool', name: 'Read', input: { file_path: 'a' } }] } });
  h.result(); await a;
  const b = h.start({ sessionId: sid, emit: e => second.push(e) }); await tick();
  h.send({ type: 'user', uuid: 'child-result', parent_tool_use_id: 'parent-tool', message: { content: [{ type: 'tool_result', tool_use_id: 'child-tool', content: 'native child output' }] } });
  h.send({ type: 'system', subtype: 'task_notification', task_id: 'native-task', tool_use_id: 'parent-tool', status: 'completed', summary: 'Done' });
  h.send({ type: 'assistant', uuid: 'second', message: { id: 'second-msg', content: [{ type: 'text', text: 'Next answer' }] } });
  h.result(); await b;
  assert.equal(h.queries.length, 1);
  assert.equal(h.queries[0].inputs.length, 2);
  assert.equal(h.queries[0].inputs[1].session_id, sid);
  assert.equal(first.find(e => e.type === 'tool.output').text, 'native child output');
  const scope = first.find(e => e.type === 'tool.output').scope;
  assert.equal(scope.id, 'parent-tool');
  assert.equal(scope.sessionId, undefined);
  assert.equal(first.at(-1).status, 'completed');
  assert.equal(second.some(e => e.type === 'tool.output'), false);
  assert.equal(second.find(e => e.type === 'text.completed').text, 'Next answer');
});

test('interrupt awaits native acknowledgment and terminal result; close aborts and cleans up idempotently', async t => {
  const h = harness(); t.after(() => h.adapter.close());
  await h.adapter.open();
  const turn = h.start(); await tick();
  let acknowledge;
  h.queries[0].interrupt = () => new Promise(resolve => { acknowledge = resolve; });
  const interrupted = h.adapter.interrupt();
  let finished = false;
  turn.then(() => { finished = true; });
  h.result(); await tick();
  assert.equal(finished, false);
  acknowledge();
  assert.deepEqual(await interrupted, { interrupted: true });
  assert.equal((await turn).finish, 'interrupted');
  const pending = h.start({ sessionId: sid }); await tick();
  const rejected = assert.rejects(pending, /closed/);
  await h.adapter.close(); await rejected;
  await h.adapter.close();
  assert.equal(h.queries[0].closeCount, 1);
  assert.equal(h.queries[0].args.options.abortController.signal.aborted, true);
  await assert.rejects(h.adapter.open(), /closed/);
});

test('failed native interrupt is not reported as confirmed cancellation', async t => {
  const h = harness(); t.after(() => h.adapter.close());
  await h.adapter.open();
  const turn = h.start(); await tick();
  h.queries[0].interrupt = async () => { throw new Error('Native interrupt failed'); };
  await assert.rejects(h.adapter.interrupt(), /Native interrupt failed/);
  const rejected = assert.rejects(turn, /Native interrupt failed/);
  h.result(); await rejected;
});

test('model and effort choices are SDK-discovered; effort reinitializes using native resume', async t => {
  const h = harness(); t.after(() => h.adapter.close());
  await h.adapter.open();
  await assert.rejects(h.adapter.configure({ selection: { model: 'invented' } }), /Unsupported Claude model/);
  await assert.rejects(h.adapter.configure({ selection: { options: { permissionMode: 'bypassPermissions' } } }), /Unsupported Claude option/);
  await assert.rejects(h.adapter.configure({ selection: { model: 'haiku', options: { effort: 'max' } } }), /Unsupported Claude effort/);
  const turn = h.start(); await tick(); h.result(); await turn;
  const selection = await h.adapter.configure({ sessionId: sid, selection: { model: 'opus', options: { effort: 'max' } } });
  assert.deepEqual(selection, { model: 'opus', options: { effort: 'max' } });
  assert.deepEqual(h.queries[0].modelChanges, ['opus']);
  assert.equal(h.queries[0].closeCount, 1);
  assert.equal(h.queries[1].args.options.resume, sid);
  assert.equal(h.queries[1].args.options.effort, 'max');
  assert.equal((await h.adapter.describe()).options[0].currentValue, 'max');
});

test('history imports native user/assistant tool results without executing a prompt or asserting full system history', async t => {
  const h = harness({ getSessionMessages: async () => [
    { type: 'user', uuid: 'u', session_id: sid, parent_tool_use_id: null, message: { role: 'user', content: 'hello' } },
    { type: 'assistant', uuid: 'a', session_id: sid, parent_tool_use_id: null, message: { id: 'm', role: 'assistant', content: [
      { type: 'text', text: 'Reading' }, { type: 'thinking', thinking: 'Need context' }, { type: 'tool_use', id: 'read', name: 'Read', input: { file_path: 'missing' } },
    ] } },
    { type: 'user', uuid: 'r', session_id: sid, parent_tool_use_id: null, message: { content: [{ type: 'tool_result', tool_use_id: 'read', content: 'not found', is_error: true }] } },
  ] }); t.after(() => h.adapter.close());
  const history = await h.adapter.history({ sessionId: sid, cwd: '/work' });
  assert.equal(history.cwd, '/work');
  await assert.rejects(h.adapter.history({ sessionId: sid, cwd: '/query' }), /cwd mismatch/);
  assert.equal(history.complete, false);
  assert.equal(history.messages[0].events[0].text, 'hello');
  assert.equal(history.messages[1].events[2].type, 'tool.started');
  assert.equal(history.messages[2].events.at(-1).type, 'tool.failed');
  assert.equal(h.queries.length, 0);
  await assert.rejects(h.adapter.history({ sessionId: 'missing' }), /not found/);
  const listed = await h.adapter.list({ cwd: '/query' });
  assert.equal(listed.sessions[0].cwd, '/native');
  assert.equal(Object.hasOwn(listed.sessions[1], 'cwd'), false);
});

test('missing SDK history/list APIs are advertised unsupported and fail explicitly', async t => {
  const h = harness({ getSessionInfo: undefined, getSessionMessages: undefined, listSessions: undefined }); t.after(() => h.adapter.close());
  const d = await h.adapter.describe();
  assert.equal(d.capabilities.history, false);
  assert.equal(d.capabilities.list, false);
  assert.equal(d.capabilities.resume, false);
  await assert.rejects(h.adapter.history({ sessionId: sid }), /unsupported/);
  await assert.rejects(h.adapter.list(), /unsupported/);
  await assert.rejects(h.adapter.open({ sessionId: sid }), /unavailable/);
});

test('native auth and result errors remain explicit; no provider fallback or login is attempted', async t => {
  const h = harness(); t.after(() => h.adapter.close());
  await h.adapter.open();
  const turn = h.start(); await tick();
  const rejected = assert.rejects(turn, /authentication_failed/);
  h.send({ type: 'assistant', error: 'authentication_failed', message: { id: 'err', content: [{ type: 'text', text: 'Please log in' }] } });
  await rejected;
  assert.equal(h.queries.length, 1);
  assert.equal(h.queries[0].args.options.abortController.signal.aborted, true);
  const h2 = harness(); t.after(() => h2.adapter.close());
  await h2.adapter.open();
  const second = h2.start(); await tick();
  const resultError = assert.rejects(second, /billing failure/);
  h2.result({ subtype: 'error_during_execution', is_error: true, errors: ['billing failure'] });
  await resultError;
  assert.equal(h2.events.at(-1).type, 'usage.updated');
});

test('initialization failure closes the SDK query and preserves a readable native error', async t => {
  const h = harness(); t.after(() => h.adapter.close());
  const query = h.sdk.query;
  h.sdk.query = args => {
    const q = query(args);
    q.initializationResult = async () => { throw new Error('Claude executable is unavailable'); };
    return q;
  };
  await assert.rejects(h.adapter.describe(), /Claude executable is unavailable/);
  assert.equal(h.queries[0].closeCount, 1);
  assert.equal(h.queries[0].args.options.abortController.signal.aborted, true);
  assert.equal(h.queries[0].inputs.length, 0);
});

test('unexpected SDK EOF rejects an active turn rather than inventing successful completion', async t => {
  const h = harness(); t.after(() => h.adapter.close());
  await h.adapter.open();
  const turn = h.start(); await tick();
  const rejected = assert.rejects(turn, /stream ended/);
  h.queries[0].output.end();
  await rejected;
  assert.equal(h.events.some(e => e.type === 'tool.completed'), false);
});

test('list pagination uses SDK offset/limit and never fills in an unconfirmed workspace', async t => {
  const calls = [];
  const h = harness({ listSessions: async options => {
    calls.push(options);
    return Array.from({ length: options.offset === 0 ? 100 : 1 }, (_, i) => ({ sessionId: String(options.offset + i), summary: 'session' }));
  } }); t.after(() => h.adapter.close());
  const first = await h.adapter.list({ cwd: '/work' });
  assert.equal(first.nextCursor, '100');
  const second = await h.adapter.list({ cwd: '/work', cursor: first.nextCursor });
  assert.equal(second.nextCursor, undefined);
  assert.equal(second.sessions[0].sessionId, '100');
  assert.equal(Object.hasOwn(second.sessions[0], 'cwd'), false);
  assert.deepEqual(calls, [
    { dir: '/work', includeWorktrees: false, offset: 0, limit: 100 },
    { dir: '/work', includeWorktrees: false, offset: 100, limit: 100 },
  ]);
  await assert.rejects(h.adapter.list({ cursor: 'bad' }), /Invalid Claude session cursor/);
});