import test from 'node:test';
import assert from 'node:assert/strict';
import { setImmediate as tick } from 'node:timers/promises';
import { createAdapter } from './codex.mjs';

const model = { id: 'catalog-id', model: 'gpt-test', displayName: 'Test', isDefault: true, defaultReasoningEffort: 'medium', supportedReasoningEfforts: [{ reasoningEffort: 'medium' }, { reasoningEffort: 'high' }] };
class FakeRpc {
  calls = []; serial = 0; closed = 0;
  overrides = new Map();
  request = async (method, params) => {
    this.calls.push([method, params]);
    if (this.overrides.has(method)) return this.overrides.get(method)(params);
    if (method === 'initialize') return {};
    if (method === 'account/read') return { account: { type: 'chatgpt' }, requiresOpenaiAuth: true };
    if (method === 'model/list') return { data: [model], nextCursor: null };
    if (method === 'thread/start' || method === 'thread/resume') return { thread: { id: params.threadId ?? 'thread-1', cwd: '/workspace' }, model: params.model ?? 'gpt-test', reasoningEffort: params.config?.model_reasoning_effort ?? 'medium' };
    if (method === 'turn/start') return { turn: { id: `turn-${++this.serial}`, status: 'inProgress' } };
    return {};
  };
  notify(method, params) { this.calls.push([method, params]); }
  onNotification(fn) { this.notification = fn; }
  onFailure(fn) { this.failure = fn; }
  setRequestHandler(fn) { this.interaction = fn; }
  async close() { this.closed++; }
  event(method, params = {}) { this.notification(method, { threadId: 'thread-1', turnId: `turn-${this.serial}`, ...params }); }
  end(status = 'completed', items = []) { this.event('turn/completed', { turn: { id: `turn-${this.serial}`, status, items } }); }
}
async function fixture(t, policy) {
  const rpc = new FakeRpc();
  const adapter = createAdapter({ cwd: '/workspace', executable: '/installed/codex', policy, emit: () => { throw new Error('Transcript escaped turn callback'); } }, rpc);
  t.after(() => adapter.close());
  await adapter.open({});
  return { rpc, adapter };
}
async function start(adapter, request = async () => { throw new Error('Unexpected interaction'); }) {
  const events = [];
  const result = adapter.start({ sessionId: 'thread-1', text: 'test', emit: e => events.push(e), request });
  await tick();
  return { events, result };
}
function outputs(events, id) {
  let text = '';
  for (const event of events.filter(e => e.id === id && e.type === 'tool.output')) text = event.mode === 'replace' ? event.text : text + event.text;
  return text;
}

test('initialize → initialized → account/models; describe never sends a turn', async t => {
  const { rpc, adapter } = await fixture(t);
  const description = await adapter.describe();
  assert.deepEqual(rpc.calls.slice(0, 4).map(c => c[0]), ['initialize', 'initialized', 'account/read', 'model/list']);
  assert.equal(description.models[0].id, 'gpt-test');
  assert.equal(description.capabilities.nativeChildren, 'observed');
  assert.equal(description.capabilities.steer, true);
  assert.equal(rpc.calls.filter(c => c[0] === 'turn/start').length, 0);
});

test('initial command aggregate seeds first stdout, later bytes append, final snapshot does not duplicate', async t => {
  const { rpc, adapter } = await fixture(t);
  const { result, events } = await start(adapter);
  rpc.event('item/started', { item: { id: 'cmd', type: 'commandExecution', command: 'printf first; printf later', cwd: '/workspace', aggregatedOutput: 'first\n', status: 'inProgress' } });
  assert.equal(outputs(events, 'cmd'), 'first\n');
  rpc.event('item/commandExecution/outputDelta', { itemId: 'cmd', delta: 'later\n' });
  assert.equal(outputs(events, 'cmd'), 'first\nlater\n');
  rpc.event('item/completed', { item: { id: 'cmd', status: 'completed', exitCode: 0, aggregatedOutput: 'first\nlater\n' } });
  rpc.event('item/commandExecution/outputDelta', { itemId: 'cmd', delta: 'must not resurrect' });
  rpc.end(); await result;
  assert.deepEqual(events.filter(e => e.type === 'tool.output').map(e => [e.mode, e.text]), [['replace', 'first\n'], ['append', 'later\n']]);
  assert.equal(events.filter(e => e.type === 'tool.started').length, 1);
  assert.equal(events.find(e => e.type === 'tool.updated').input.command, 'printf first; printf later');
  assert.equal(events.find(e => e.type === 'tool.completed').exitCode, 0);
});

for (const finalAggregate of ['END', 'BEGIN', '', 'BEGINEND']) {
  test(`completion aggregate ${JSON.stringify(finalAggregate)} preserves BEGIN initial + END delta exactly once`, async t => {
    const { rpc, adapter } = await fixture(t);
    const { result, events } = await start(adapter);
    rpc.event('item/started', { item: { id: 'cmd', type: 'commandExecution', command: 'stream', aggregatedOutput: 'BEGIN', status: 'inProgress' } });
    assert.equal(outputs(events, 'cmd'), 'BEGIN');
    rpc.event('item/commandExecution/outputDelta', { itemId: 'cmd', delta: 'END' });
    assert.equal(outputs(events, 'cmd'), 'BEGINEND');
    rpc.event('item/completed', { item: { id: 'cmd', status: 'completed', aggregatedOutput: finalAggregate } });
    rpc.end(); await result;
    assert.equal(outputs(events, 'cmd'), 'BEGINEND');
    assert.deepEqual(events.filter(e => e.type === 'tool.output').map(e => [e.mode, e.text]), [['replace', 'BEGIN'], ['append', 'END']]);
  });
}

test('identical legitimate command deltas always append, even when they equal the initial snapshot', async t => {
  const { rpc, adapter } = await fixture(t);
  const { result, events } = await start(adapter);
  rpc.event('item/started', { item: { id: 'cmd', type: 'commandExecution', command: 'repeat', aggregatedOutput: 'SAME' } });
  rpc.event('item/commandExecution/outputDelta', { itemId: 'cmd', delta: 'SAME' });
  rpc.event('item/commandExecution/outputDelta', { itemId: 'cmd', delta: 'SAME' });
  rpc.event('item/completed', { item: { id: 'cmd', status: 'completed', aggregatedOutput: 'SAME' } });
  rpc.end(); await result;
  assert.equal(outputs(events, 'cmd'), 'SAMESAMESAME');
  assert.equal(events.filter(e => e.type === 'tool.output' && e.mode === 'append').length, 2);
});

for (const sandbox of ['workspace-write', 'read-only']) {
  test(`parent ${sandbox} policy explicitly overrides native config on start, resume, configure and turn/start`, async t => {
    const policy = { approvalPolicy: 'untrusted', sandbox, permissionMode: 'default', deniedTools: ['edit'] };
    const { rpc, adapter } = await fixture(t, policy);
    await adapter.open({ sessionId: 'thread-1' });
    await adapter.configure({ sessionId: 'thread-1', selection: { options: { reasoningEffort: 'high' } } });
    const { result } = await start(adapter, async (method, params) => {
      assert.equal(method, 'permission.request');
      assert.equal(params.toolName, 'fileChange');
      return { decision: 'deny' };
    });
    // A user's configured approval_policy=never or persisted dangerous sandbox
    // cannot be inherited when all execution-bearing calls carry overrides.
    for (const [method, params] of rpc.calls.filter(c => ['thread/start', 'thread/resume', 'turn/start'].includes(c[0]))) {
      assert.equal(params.approvalPolicy, 'untrusted', method);
      assert.equal(params.approvalsReviewer, 'user', method);
      assert.equal(params.cwd, '/workspace', method);
      if (method !== 'turn/start') assert.equal(params.sandbox, sandbox, method);
      else assert.deepEqual(params.sandboxPolicy, sandbox === 'read-only' ? { type: 'readOnly', networkAccess: false } : { type: 'workspaceWrite', writableRoots: ['/workspace'], networkAccess: false, excludeTmpdirEnvVar: true, excludeSlashTmp: true });
    }
    assert.deepEqual(await rpc.interaction('item/fileChange/requestApproval', { threadId: 'thread-1', turnId: 'turn-1', itemId: 'edit' }, 100), { decision: 'decline' });
    rpc.end(); await result;
  });
}

test('missing parent policy still pins safe native defaults at thread and turn boundaries', async t => {
  const { rpc, adapter } = await fixture(t);
  const { result } = await start(adapter);
  const thread = rpc.calls.find(c => c[0] === 'thread/start')[1];
  const turn = rpc.calls.find(c => c[0] === 'turn/start')[1];
  assert.equal(thread.approvalPolicy, 'untrusted'); assert.equal(thread.sandbox, 'workspace-write');
  assert.equal(turn.approvalPolicy, 'untrusted'); assert.equal(turn.sandboxPolicy.type, 'workspaceWrite');
  assert.equal(turn.sandboxPolicy.networkAccess, false);
  rpc.end(); await result;
});

test('unsafe or unsupported native policy fails before any subprocess request', () => {
  for (const policy of [{ approvalPolicy: 'never' }, { approvalPolicy: 'on-failure' }, { sandbox: 'danger-full-access' }, { sandbox: 'externalSandbox' }]) {
    const rpc = new FakeRpc();
    assert.throws(() => createAdapter({ cwd: '/workspace', policy }, rpc), /Unsafe or unsupported Codex/);
    assert.equal(rpc.calls.length, 0);
  }
});

test('inherited restrictions that native auto-approved tools cannot enforce fail closed', () => {
  for (const rule of [{ permission: 'read', pattern: 'secret.txt', action: 'deny' }, { permission: 'bash', pattern: '*', action: 'deny' }]) {
    const rpc = new FakeRpc();
    assert.throws(() => createAdapter({ cwd: '/workspace', policy: { sandbox: 'read-only', deniedRules: [rule] } }, rpc), /cannot enforce this inherited permission/);
    assert.equal(rpc.calls.length, 0);
  }
});

test('read-only inherited policy cannot be relaxed by command or permission approval', async t => {
  const { rpc, adapter } = await fixture(t, { sandbox: 'read-only', deniedRules: [{ permission: 'edit', pattern: '*', action: 'deny' }] });
  const { result } = await start(adapter, async () => ({ decision: 'allow_session' }));
  const scope = { threadId: 'thread-1', turnId: 'turn-1', itemId: 'tool' };
  assert.deepEqual(await rpc.interaction('item/commandExecution/requestApproval', scope, 10), { decision: 'decline' });
  assert.deepEqual(await rpc.interaction('item/permissions/requestApproval', { ...scope, permissions: { fileSystem: { write: ['/outside'] } } }, 11), { permissions: {}, scope: 'turn' });
  rpc.end(); await result;
});

test('model-specific option discovery never creates a provider thread', async t => {
  const rpc = new FakeRpc();
  rpc.overrides.set('model/list', () => ({ data: [model, { ...model, id: 'other', model: 'other', isDefault: false, defaultReasoningEffort: 'low', supportedReasoningEfforts: [{ reasoningEffort: 'low' }] }], nextCursor: null }));
  const adapter = createAdapter({ cwd: '/workspace', executable: '/installed/codex' }, rpc);
  t.after(() => adapter.close());
  const description = await adapter.describe({ selection: { model: 'other' } });
  assert.deepEqual(description.options[0].options.map(option => option.value), ['low']);
  assert.ok(!rpc.calls.some(([method]) => method === 'thread/start' || method === 'turn/start'));
});

test('different final aggregate replaces, sparse completion retains read/search/edit inputs', async t => {
  const { rpc, adapter } = await fixture(t);
  const { result, events } = await start(adapter);
  for (const [id, action] of [['read', 'read'], ['search', 'search']]) {
    rpc.event('item/started', { item: { id, type: 'commandExecution', command: id, commandActions: [{ type: action, path: 'file' }], aggregatedOutput: 'partial' } });
    rpc.event('item/completed', { item: { id, aggregatedOutput: 'full', status: 'completed' } });
    assert.equal(outputs(events, id), 'full');
    assert.equal(events.find(e => e.id === id && e.type === 'tool.started').kind, action);
    assert.equal(events.find(e => e.id === id && e.type === 'tool.updated').input.actions[0].path, 'file');
  }
  rpc.event('item/started', { item: { id: 'edit', type: 'fileChange', changes: [{ path: 'file', diff: '+line' }] } });
  rpc.event('item/fileChange/patchUpdated', { itemId: 'edit', changes: [{ path: 'file', diff: '+new line' }] });
  rpc.event('item/fileChange/outputDelta', { itemId: 'edit', delta: 'applied' });
  rpc.event('item/completed', { item: { id: 'edit', status: 'completed' } });
  rpc.end(); await result;
  assert.equal(events.find(e => e.id === 'edit' && e.type === 'tool.started').kind, 'edit');
  assert.equal(outputs(events, 'edit'), 'applied');
  assert.equal(events.find(e => e.id === 'edit' && e.type === 'tool.updated').input.changes[0].diff, '+new line');
});

test('text, reasoning, plans, MCP progress, failure, model and multi-call turn usage are observable', async t => {
  const { rpc, adapter } = await fixture(t);
  const { result, events } = await start(adapter);
  rpc.event('item/agentMessage/delta', { itemId: 'text', delta: 'hello' });
  rpc.event('item/completed', { item: { type: 'agentMessage', id: 'text', text: 'hello' } });
  rpc.event('item/reasoning/summaryTextDelta', { itemId: 'reason', delta: 'thinking' });
  rpc.event('item/completed', { item: { type: 'reasoning', id: 'reason', summary: ['thinking'] } });
  rpc.event('turn/plan/updated', { plan: [{ step: 'Read', status: 'inProgress' }] });
  rpc.event('item/started', { item: { type: 'mcpToolCall', id: 'mcp', tool: 'lookup', arguments: { key: 1 } } });
  rpc.event('item/mcpToolCall/progress', { itemId: 'mcp', message: 'working' });
  rpc.event('item/completed', { item: { id: 'mcp', status: 'failed', error: { message: 'broken' } } });
  rpc.event('model/rerouted', { toModel: 'fallback' });
  const count = n => ({ inputTokens: n, outputTokens: n, cachedInputTokens: 0, reasoningOutputTokens: 0, cacheWriteInputTokens: 0, totalTokens: n * 2 });
  rpc.event('thread/tokenUsage/updated', { tokenUsage: { total: count(110), last: count(10), modelContextWindow: 1000 } });
  rpc.event('thread/tokenUsage/updated', { tokenUsage: { total: count(115), last: count(5), modelContextWindow: 1000 } });
  rpc.end();
  const completion = await result;
  assert.equal(completion.model, 'fallback'); assert.equal(completion.usage.input, 15);
  for (const type of ['text.delta', 'text.completed', 'reasoning.delta', 'reasoning.completed', 'plan.updated', 'tool.failed', 'usage.updated', 'model.updated']) assert.ok(events.some(e => e.type === type), type);
  assert.equal(events.find(e => e.type === 'tool.updated' && e.metadata.message).metadata.message, 'working');
});

test('native approvals and questions use captured turn callbacks and translate exact response shape', async t => {
  const { rpc, adapter } = await fixture(t);
  const requests = [];
  const { result } = await start(adapter, async (method, params) => {
    requests.push([method, params]);
    return method === 'permission.request' ? { decision: 'allow_session' } : { answers: { q: ['A'] } };
  });
  const scope = { threadId: 'thread-1', turnId: 'turn-1', itemId: 'tool' };
  assert.deepEqual(await rpc.interaction('item/commandExecution/requestApproval', scope, 10), { decision: 'acceptForSession' });
  assert.deepEqual(await rpc.interaction('item/fileChange/requestApproval', scope, 11), { decision: 'acceptForSession' });
  assert.deepEqual(await rpc.interaction('item/permissions/requestApproval', { ...scope, permissions: { network: { enabled: true } } }, 12), { permissions: { network: { enabled: true } }, scope: 'session' });
  assert.deepEqual(await rpc.interaction('item/tool/requestUserInput', { ...scope, questions: [{ id: 'q', question: 'Pick', isOther: true, options: [{ label: 'A' }] }] }, 13), { answers: { q: { answers: ['A'] } } });
  assert.equal(requests[3][1].questions[0].custom, true);
  assert.equal(requests[0][1].toolCallId, 'tool');
  rpc.end(); await result;
  await assert.rejects(rpc.interaction('item/commandExecution/requestApproval', scope, 14), /no active/);
});

test('deny and cancelled questions fail closed; late answer cannot reach the next turn', async t => {
  const { rpc, adapter } = await fixture(t);
  let answer;
  const first = await start(adapter, (method, params) => method === 'permission.request' ? new Promise(resolve => { answer = resolve; }) : Promise.reject(new Error('question cancelled')));
  const params = { threadId: 'thread-1', turnId: 'turn-1', itemId: 'tool' };
  const pending = rpc.interaction('item/commandExecution/requestApproval', params, 1);
  await assert.rejects(rpc.interaction('item/tool/requestUserInput', { ...params, questions: [] }, 2), /question cancelled/);
  rpc.end('interrupted'); await first.result;
  const second = await start(adapter, async () => ({ decision: 'deny' }));
  answer({ decision: 'allow_once' });
  await assert.rejects(pending, /ended/);
  assert.deepEqual(await rpc.interaction('item/commandExecution/requestApproval', { ...params, turnId: 'turn-2' }, 3), { decision: 'decline' });
  rpc.end(); await second.result;
});

test('interrupt waits for native terminal confirmation; steer pins expected native turn', async t => {
  const { rpc, adapter } = await fixture(t);
  const { result } = await start(adapter);
  await adapter.steer({ sessionId: 'thread-1', text: 'instead' });
  assert.equal(rpc.calls.find(c => c[0] === 'turn/steer')[1].expectedTurnId, 'turn-1');
  let stopped = false;
  const interruption = adapter.interrupt({ sessionId: 'thread-1' }).then(value => { stopped = true; return value; });
  await tick(); assert.equal(stopped, false);
  rpc.end('interrupted');
  assert.deepEqual(await interruption, { interrupted: true });
  assert.equal((await result).finish, 'interrupted');
});

test('interrupt RPC failure never reports confirmed stop', async t => {
  const { rpc, adapter } = await fixture(t);
  const { result } = await start(adapter);
  rpc.overrides.set('turn/interrupt', () => { throw new Error('cannot interrupt'); });
  await assert.rejects(adapter.interrupt({ sessionId: 'thread-1' }), /cannot interrupt/);
  rpc.end(); assert.equal((await result).finish, 'completed');
});

test('completed turn is not falsely reported as interrupted after interrupt acknowledgement', async t => {
  const { rpc, adapter } = await fixture(t);
  const { result } = await start(adapter);
  const interrupted = adapter.interrupt({ sessionId: 'thread-1' });
  await tick(); rpc.end();
  await assert.rejects(interrupted, /not interrupted/);
  assert.equal((await result).finish, 'completed');
});

test('concurrent starts cannot steal the owning operation', async t => {
  const { rpc, adapter } = await fixture(t);
  const events = [];
  const first = adapter.start({ sessionId: 'thread-1', text: 'first', emit: e => events.push(e), request: async () => ({ decision: 'deny' }) });
  const second = adapter.start({ sessionId: 'thread-1', text: 'second', emit: () => assert.fail('stolen callback'), request: async () => ({ decision: 'deny' }) });
  const rejected = assert.rejects(second, /already has an active turn/);
  await tick(); await rejected;
  rpc.event('item/agentMessage/delta', { itemId: 'text', delta: 'first turn' }); rpc.end(); await first;
  assert.equal(events.find(e => e.type === 'text.delta').text, 'first turn');
  assert.equal(rpc.calls.filter(c => c[0] === 'turn/start').length, 1);
});

test('requested resume never falls back to a new thread; configure rejects unsupported or unconfirmed selection', async t => {
  const { rpc, adapter } = await fixture(t);
  rpc.overrides.set('thread/resume', () => { throw new Error('thread not found'); });
  await assert.rejects(adapter.open({ sessionId: 'missing' }), /thread not found/);
  assert.equal(rpc.calls.filter(c => c[0] === 'thread/start').length, 1);
  rpc.overrides.delete('thread/resume');
  const resumed = await adapter.open({ sessionId: 'old' }); assert.equal(resumed.sessionId, 'old');
  assert.deepEqual(await adapter.configure({ sessionId: 'old', selection: { model: 'catalog-id', options: { reasoningEffort: 'high' } } }), { model: 'gpt-test', options: { reasoningEffort: 'high' } });
  await assert.rejects(adapter.configure({ sessionId: 'old', selection: { model: 'nonexistent' } }), /Unsupported Codex model/);
  await assert.rejects(adapter.configure({ sessionId: 'old', selection: { options: { unsafe: true } } }), /Unsupported Codex option/);
  rpc.overrides.set('thread/resume', () => ({ thread: { id: 'old', cwd: '/workspace' }, model: 'gpt-test', reasoningEffort: 'medium' }));
  await assert.rejects(adapter.configure({ sessionId: 'old', selection: { options: { reasoningEffort: 'high' } } }), /did not apply/);
});

test('early notifications before turn/start response retain identity, stale turn events do not leak', async t => {
  const { rpc, adapter } = await fixture(t);
  rpc.overrides.set('turn/start', () => {
    rpc.serial++;
    rpc.event('item/agentMessage/delta', { itemId: 'a', delta: 'early' });
    rpc.event('item/agentMessage/delta', { turnId: 'previous', itemId: 'bad', delta: 'stale' });
    rpc.end();
    return { turn: { id: 'turn-1', status: 'inProgress' } };
  });
  const { result, events } = await start(adapter); await result;
  assert.equal(events.find(e => e.type === 'text.delta').text, 'early');
  assert.ok(!events.some(e => e.id === 'bad'));
});

test('real native receiver IDs create observed child scopes; child completion does not terminate root', async t => {
  const { rpc, adapter } = await fixture(t);
  const { result, events } = await start(adapter);
  rpc.event('item/started', { item: { type: 'collabAgentToolCall', id: 'spawn', tool: 'spawnAgent', receiverThreadIds: ['native-child'], status: 'inProgress' } });
  rpc.event('item/completed', { item: { id: 'spawn', status: 'completed' } });
  assert.ok(!events.some(e => e.type === 'child.updated' && e.status === 'completed'));
  rpc.event('item/agentMessage/delta', { threadId: 'native-child', turnId: 'child-turn', itemId: 'text', delta: 'child output' });
  rpc.event('turn/completed', { threadId: 'native-child', turn: { id: 'child-turn', status: 'completed' } });
  const child = events.find(e => e.text === 'child output');
  assert.equal(child.scope.id, 'native-child'); assert.equal(child.scope.parentToolCallId, 'spawn');
  let done = false; result.then(() => { done = true; }); await tick(); assert.equal(done, false);
  rpc.end(); await result;
});

test('native list requires confirmed cwd and legacy history projects data without resuming or executing', async t => {
  const { rpc, adapter } = await fixture(t);
  rpc.overrides.set('thread/list', () => ({ data: [{ id: 'a', cwd: '/workspace', name: 'Title', updatedAt: 42 }, { id: 'b' }, { id: 'c', cwd: '/elsewhere' }], nextCursor: 'next' }));
  assert.deepEqual(await adapter.list({ cwd: '/workspace' }), { sessions: [{ sessionId: 'a', cwd: '/workspace', title: 'Title', updatedAt: 42 }], nextCursor: 'next' });
  rpc.overrides.set('thread/read', () => ({ thread: { id: 'old', cwd: '/workspace', historyMode: 'legacy', turns: [{ id: 'historic-turn', items: [{ id: 'u', type: 'userMessage', content: [{ type: 'text', text: 'old prompt' }] }, { id: 'c', type: 'commandExecution', command: 'echo old', aggregatedOutput: 'old stdout', status: 'completed' }, { id: 'a', type: 'agentMessage', text: 'old answer' }] }] } }));
  const history = await adapter.history({ sessionId: 'old', cwd: '/workspace' });
  assert.equal(history.complete, true); assert.equal(history.messages[0].role, 'user');
  assert.equal(history.messages[0].events[0].text, 'old prompt');
  assert.equal(outputs(history.messages[1].events, 'c'), 'old stdout');
  assert.equal(history.messages[1].events.find(e => e.type === 'text.completed').text, 'old answer');
  assert.ok(!rpc.calls.some(c => ['turn/start', 'thread/resume'].includes(c[0])));
  await assert.rejects(adapter.history({ sessionId: 'old', cwd: '/other' }), /mismatch/);
});

test('paginated native history hydrates turns and sparse items, follows cursors', async t => {
  const { rpc, adapter } = await fixture(t);
  rpc.overrides.set('thread/read', () => ({ thread: { id: 'old', cwd: '/workspace', historyMode: 'paginated' } }));
  rpc.overrides.set('thread/turns/list', p => ({ data: [{ id: p.cursor ? 't2' : 't1', itemsView: 'notLoaded' }], nextCursor: p.cursor ? null : 'turn-cursor' }));
  rpc.overrides.set('thread/items/list', p => ({ data: [{ item: { id: `${p.turnId}-${p.cursor ?? 'first'}`, type: 'agentMessage', text: p.cursor ? 'second' : 'first' } }], nextCursor: p.cursor ? null : 'item-cursor' }));
  const history = await adapter.history({ sessionId: 'old' });
  assert.equal(history.messages.length, 2);
  assert.deepEqual(history.messages[0].events.map(e => e.text), ['first', 'second']);
  assert.equal(rpc.calls.filter(c => c[0] === 'thread/items/list').length, 4);
});

test('close rejects in-flight turn and is idempotent', async t => {
  const { rpc, adapter } = await fixture(t);
  const { result } = await start(adapter);
  const rejected = assert.rejects(result, /closed/);
  await Promise.all([adapter.close(), adapter.close()]); await rejected;
  assert.equal(rpc.closed, 1);
  await assert.rejects(adapter.describe(), /closed/);
});

test('transport failure rejects the owning turn', async t => {
  const { rpc, adapter } = await fixture(t);
  const { result } = await start(adapter);
  const rejected = assert.rejects(result, /process died/);
  rpc.failure(new Error('process died')); await rejected;
});

test('delta-only text/reasoning retain content at sparse completion and terminal-start tools stay terminal', async t => {
  const { rpc, adapter } = await fixture(t);
  const { result, events } = await start(adapter);
  rpc.event('item/agentMessage/delta', { itemId: 'text', delta: 'kept' });
  rpc.event('item/completed', { item: { id: 'text' } });
  rpc.event('item/reasoning/textDelta', { itemId: 'reason', delta: 'thought' });
  rpc.event('item/completed', { item: { id: 'reason' } });
  rpc.event('item/started', { item: { type: 'commandExecution', id: 'done', command: 'true', status: 'completed', aggregatedOutput: 'done' } });
  rpc.event('item/commandExecution/outputDelta', { itemId: 'done', delta: 'late' });
  rpc.end(); await result;
  assert.equal(events.find(e => e.type === 'text.completed').text, 'kept');
  assert.equal(events.find(e => e.type === 'reasoning.completed').text, 'thought');
  assert.equal(outputs(events, 'done'), 'done');
});