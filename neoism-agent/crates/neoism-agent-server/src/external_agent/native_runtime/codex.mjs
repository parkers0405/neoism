import { spawnJsonRpc } from './json-rpc.mjs';

const capabilities = Object.freeze({ resume: true, history: true, list: true, modelSelection: true, steer: true, toolStreaming: true, reasoning: true, nativeChildren: 'observed' });
const defined = object => Object.fromEntries(Object.entries(object).filter(([, value]) => value !== undefined && value !== null));
const printable = value => typeof value === 'string' ? value : JSON.stringify(value);
const inputText = text => [{ type: 'text', text, text_elements: [] }];
const deferred = () => { let resolve, reject; const promise = new Promise((a, b) => { resolve = a; reject = b; }); return { promise, resolve, reject }; };

// Public factory has no startup side effects until first use. The optional transport
// is a dependency-free test seam, not a second provider or a model-turn simulator.
export function createAdapter(context, injectedTransport) {
  // Parent policy is authoritative, not ~/.codex/config.toml or persisted thread
  // settings. Native approvals route through Rust, but native auto-approved
  // tools cannot enforce arbitrary Neoism path/tool denials. Refuse those modes.
  const approvalPolicy = context.policy?.approvalPolicy ?? 'untrusted';
  const sandbox = context.policy?.sandbox ?? 'workspace-write';
  if (!['untrusted', 'on-request'].includes(approvalPolicy)) throw new Error(`Unsafe or unsupported Codex approval policy: ${approvalPolicy}`);
  if (!['workspace-write', 'read-only'].includes(sandbox)) throw new Error(`Unsafe or unsupported Codex sandbox: ${sandbox}`);
  for (const rule of context.policy?.deniedRules ?? []) {
    if (sandbox !== 'read-only' || rule.pattern !== '*' || !['edit', 'write', 'apply_patch'].includes(rule.permission)) {
      throw new Error('Codex app-server cannot enforce this inherited permission restriction; execution is refused');
    }
  }
  function nativePolicy(forTurn = false) {
    const common = { cwd: context.cwd, approvalPolicy, approvalsReviewer: 'user' };
    // 0.163 ThreadStart/ResumeParams take SandboxMode strings, whereas
    // TurnStartParams takes the tagged SandboxPolicy object (camelCase).
    return forTurn ? {
      ...common,
      sandboxPolicy: sandbox === 'read-only' ? { type: 'readOnly', networkAccess: false } : {
        type: 'workspaceWrite', writableRoots: [context.cwd], networkAccess: false,
        excludeTmpdirEnvVar: true, excludeSlashTmp: true,
      },
    } : { ...common, sandbox };
  }
  let rpc, ready, closed = false, discovery, closing;
  const sessions = new Map(), active = new Map(), childOwners = new Map(), usageBaselines = new Map();
  async function connect() {
    if (closed) throw new Error('Codex adapter is closed');
    if (!ready) ready = (async () => {
      rpc = injectedTransport ?? spawnJsonRpc({ executable: context.executable, cwd: context.cwd });
      rpc.onNotification(notification);
      rpc.onFailure(error => { for (const turn of active.values()) turn.done.reject(error); active.clear(); });
      rpc.setRequestHandler(interaction);
      await rpc.request('initialize', { clientInfo: { name: 'neoism', title: 'Neoism', version: '1.0.0' }, capabilities: { experimentalApi: true } });
      rpc.notify('initialized');
    })();
    await ready;
    return rpc;
  }
  async function discover() {
    await connect();
    if (!discovery) discovery = (async () => {
      const auth = await rpc.request('account/read', { refreshToken: false });
      const models = [];
      let cursor;
      do {
        const page = await rpc.request('model/list', defined({ cursor, limit: 100 }));
        models.push(...page.data); cursor = page.nextCursor;
      } while (cursor);
      return { auth, models };
    })();
    return discovery;
  }
  function effective(response) {
    return { model: response.model ?? response.thread?.model, options: defined({ reasoningEffort: response.reasoningEffort ?? response.thread?.reasoningEffort }) };
  }
  async function selectionParams(selection = {}, previous = {}) {
    const catalog = (await discover()).models;
    const model = selection.model ?? previous.model;
    const entry = catalog.find(m => m.model === model || m.id === model);
    if (selection.model && !entry) throw new Error(`Unsupported Codex model: ${selection.model}`);
    const options = { ...previous.options, ...selection.options };
    for (const key of Object.keys(options)) if (key !== 'reasoningEffort') throw new Error(`Unsupported Codex option: ${key}`);
    if (options.reasoningEffort !== undefined && !(entry ?? catalog.find(m => m.isDefault))?.supportedReasoningEfforts?.some(e => e.reasoningEffort === options.reasoningEffort)) throw new Error(`Unsupported Codex reasoning effort: ${options.reasoningEffort}`);
    return { model: entry?.model ?? model, options };
  }
  function threadParams(selection) {
    return defined({ model: selection.model, config: selection.options.reasoningEffort === undefined ? undefined : { model_reasoning_effort: selection.options.reasoningEffort } });
  }
  function confirm(response, requested, sessionId) {
    if (!response.thread?.id || (sessionId && response.thread.id !== sessionId)) throw new Error('Codex returned a different or missing native thread ID');
    if (response.thread.cwd !== context.cwd) throw new Error('Codex native thread cwd mismatch or unavailable');
    const value = effective(response);
    if (requested.model && value.model !== requested.model) throw new Error(`Codex did not apply requested model ${requested.model}`);
    if (requested.options.reasoningEffort !== undefined && value.options.reasoningEffort !== requested.options.reasoningEffort) throw new Error('Codex did not apply requested reasoning effort');
    sessions.set(response.thread.id, value);
    return { sessionId: response.thread.id, ...value, capabilities };
  }
  function owner(params) {
    const root = active.get(params.threadId);
    if (root) return root;
    return childOwners.get(params.threadId);
  }
  function send(turn, event, params = {}) {
    const scope = params.threadId && params.threadId !== turn.sessionId ? turn.scopes.get(params.threadId) : undefined;
    turn.emit(defined({ sessionId: turn.sessionId, turnId: params.turnId ?? turn.id, scope, ...event }));
  }
  function itemState(turn, id, params) {
    const key = `${params.threadId ?? turn.sessionId}:${id}`;
    if (!turn.items.has(key)) turn.items.set(key, { item: { id }, output: '', started: false, terminal: false, text: '', reasoning: '' });
    return turn.items.get(key);
  }
  function toolPresentation(item) {
    const action = item.commandActions?.find(a => a.type !== 'unknown');
    const kind = item.type === 'fileChange' ? 'edit' : item.type === 'commandExecution' ? (action?.type === 'listFiles' ? 'search' : action?.type ?? 'execute') : item.type === 'webSearch' ? 'search' : item.type === 'imageView' ? 'read' : 'other';
    return { name: item.tool ?? item.type, kind, title: item.command ?? item.tool ?? item.type, input: defined({ command: item.command, cwd: item.cwd, actions: item.commandActions, changes: item.changes, arguments: item.arguments, server: item.server, query: item.query, action: item.action, path: item.path, prompt: item.prompt, receiverThreadIds: item.receiverThreadIds }) };
  }
  function ensureTool(turn, state, params) {
    if (state.started || state.terminal) return;
    state.started = true;
    send(turn, { type: 'tool.started', id: state.item.id, ...toolPresentation(state.item) }, params);
  }
  function snapshot(turn, state, output, params, preserveAccumulated = false) {
    if (typeof output !== 'string' || output === state.output) return;
    // Codex completion aggregates can be only a tail, or an earlier prefix,
    // rather than the full stream. Those snapshots cannot erase observed bytes.
    // This applies ONLY to aggregate snapshots: every outputDelta still appends,
    // even when two legitimate consecutive deltas contain identical bytes.
    if (preserveAccumulated && (state.output.startsWith(output) || state.output.endsWith(output))) return;
    // A genuinely different authoritative snapshot reconciles by replacement.
    send(turn, { type: 'tool.output', id: state.item.id, text: output, mode: 'replace' }, params);
    state.output = output;
  }
  function projectItem(turn, patch, completed, params) {
    const state = itemState(turn, patch.id, params);
    if (state.terminal) return;
    state.item = { ...state.item, ...defined(patch) };
    const item = state.item;
    if (item.type === 'userMessage' || item.type === 'hookPrompt') return;
    if (item.type === 'agentMessage' || item.type === 'reasoning' || item.type === 'plan') {
      const reasoning = item.type === 'reasoning';
      const prefix = reasoning ? 'reasoning' : 'text';
      const text = reasoning ? (item.summary?.join('\n') || item.content?.join('\n') || state.reasoning) : item.text ?? state.text;
      if (!completed && text && !state[reasoning ? 'reasoning' : 'text']) {
        state[reasoning ? 'reasoning' : 'text'] = text;
        send(turn, { type: `${prefix}.delta`, id: item.id, text }, params);
      }
      if (completed) {
        send(turn, defined({ type: `${prefix}.completed`, id: item.id, text }), params);
        state.terminal = true;
      }
      return;
    }
    completed ||= ['completed', 'failed', 'declined'].includes(item.status);
    const wasStarted = state.started;
    ensureTool(turn, state, params);
    if (wasStarted) send(turn, { type: 'tool.updated', id: item.id, ...toolPresentation(item), metadata: defined({ status: item.status, processId: item.processId }) }, params);
    snapshot(turn, state, patch.aggregatedOutput, params, true);
    const output = item.result ?? item.contentItems ?? (item.type === 'functionCallOutput' ? item.output : undefined);
    if (output != null) snapshot(turn, state, printable(output), params);
    for (const id of item.receiverThreadIds ?? (item.agentThreadId ? [item.agentThreadId] : [])) {
      const scope = { id, sessionId: id, parentId: item.senderThreadId ?? params.threadId ?? turn.sessionId, parentToolCallId: item.id, agent: 'codex' };
      turn.scopes.set(id, scope);
      if (!turn.history) childOwners.set(id, turn);
      const agent = item.agentsStates?.[id];
      // A completed spawn/wait tool is not evidence that the child has finished.
      const status = agent?.status;
      send(turn, { type: 'child.updated', scope, status: ['completed', 'shutdown'].includes(status) ? 'completed' : ['errored', 'failed', 'notFound'].includes(status) ? 'error' : status === 'interrupted' ? 'interrupted' : 'running', ...(agent?.message ? { summary: agent.message } : {}) }, params);
    }
    if (completed) {
      const failed = item.status === 'failed' || item.status === 'declined' || item.success === false || item.error != null || (typeof item.exitCode === 'number' && item.exitCode !== 0);
      send(turn, defined({ type: failed ? 'tool.failed' : 'tool.completed', id: item.id, error: failed ? printable(item.error ?? `Codex tool ${item.status ?? 'failed'}${item.exitCode != null ? ` (exit ${item.exitCode})` : ''}`) : undefined, exitCode: item.exitCode, metadata: defined({ status: item.status, durationMs: item.durationMs }) }), params);
      state.terminal = true;
    }
  }
  function notification(method, params) {
    if (method === 'thread/started' && params.thread?.parentThreadId) {
      const thread = params.thread;
      const parent = active.get(thread.parentThreadId) ?? childOwners.get(thread.parentThreadId);
      if (parent && !parent.finished) {
        const scope = defined({ ...parent.scopes.get(thread.id), id: thread.id, sessionId: thread.id, parentId: thread.parentThreadId, agent: thread.agentRole ?? 'codex', title: thread.agentNickname ?? thread.name });
        parent.scopes.set(thread.id, scope); childOwners.set(thread.id, parent);
        send(parent, { type: 'child.updated', scope, status: 'running' });
      }
      return;
    }
    if (method === 'thread/settings/updated' && sessions.has(params.threadId)) {
      const settings = params.threadSettings;
      sessions.set(params.threadId, { model: settings.model, options: defined({ reasoningEffort: settings.effort }) });
    }
    const turn = owner(params);
    if (method === 'thread/tokenUsage/updated' && (!turn || turn.finished)) {
      usageBaselines.set(params.threadId, params.tokenUsage.total);
      return;
    }
    if (!turn || turn.finished) return;
    if (!turn.id) { turn.early.push([method, params]); return; }
    const root = params.threadId === turn.sessionId;
    if (root && params.turnId && params.turnId !== turn.id) {
      if (method === 'thread/tokenUsage/updated') usageBaselines.set(params.threadId, params.tokenUsage.total);
      return;
    }
    if (root && params.turn?.id && params.turn.id !== turn.id) return;
    if (method === 'item/started' || method === 'item/completed') projectItem(turn, params.item, method === 'item/completed', params);
    else if (method === 'item/agentMessage/delta' || method === 'item/reasoning/summaryTextDelta' || method === 'item/reasoning/textDelta' || method === 'item/plan/delta') {
      const state = itemState(turn, params.itemId, params);
      if (state.terminal) return;
      const reasoning = method.includes('/reasoning/');
      state.item.type ??= reasoning ? 'reasoning' : method === 'item/plan/delta' ? 'plan' : 'agentMessage';
      state[reasoning ? 'reasoning' : 'text'] += params.delta;
      send(turn, { type: reasoning ? 'reasoning.delta' : 'text.delta', id: params.itemId, text: params.delta }, params);
    } else if (method === 'item/commandExecution/outputDelta' || method === 'item/fileChange/outputDelta') {
      const state = itemState(turn, params.itemId, params);
      if (state.terminal) return;
      if (!state.item.type) state.item.type = method.includes('commandExecution') ? 'commandExecution' : 'fileChange';
      ensureTool(turn, state, params);
      state.output += params.delta;
      send(turn, { type: 'tool.output', id: params.itemId, text: params.delta, mode: 'append' }, params);
    } else if (method === 'item/fileChange/patchUpdated') {
      projectItem(turn, { id: params.itemId, type: 'fileChange', changes: params.changes }, false, params);
    } else if (method === 'item/commandExecution/terminalInteraction') {
      const state = itemState(turn, params.itemId, params);
      state.item.type ??= 'commandExecution';
      if (!state.terminal) { ensureTool(turn, state, params); send(turn, { type: 'tool.updated', id: params.itemId, metadata: { stdin: params.stdin, processId: params.processId } }, params); }
    } else if (method === 'item/mcpToolCall/progress') {
      const state = itemState(turn, params.itemId, params);
      if (!state.item.type) state.item.type = 'mcpToolCall';
      if (!state.terminal) { ensureTool(turn, state, params); send(turn, { type: 'tool.updated', id: params.itemId, metadata: params }, params); }
    } else if (method === 'turn/plan/updated') send(turn, { type: 'plan.updated', entries: params.plan.map(p => ({ content: p.step, status: p.status })) }, params);
    else if (method === 'thread/tokenUsage/updated') {
      const current = params.tokenUsage.total, previous = usageBaselines.get(params.threadId);
      const reset = !previous || ['inputTokens', 'outputTokens', 'reasoningOutputTokens', 'cachedInputTokens'].some(k => current[k] < previous[k]);
      const delta = reset ? params.tokenUsage.last : Object.fromEntries(Object.entries(current).map(([key, value]) => [key, previous[key] === undefined ? undefined : Math.max(0, value - previous[key])]));
      usageBaselines.set(params.threadId, current);
      const usage = { ...(root ? turn.usage : turn.childUsage.get(params.threadId)) };
      for (const [normalized, native] of Object.entries({ input: 'inputTokens', output: 'outputTokens', reasoning: 'reasoningOutputTokens', cacheRead: 'cachedInputTokens', cacheWrite: 'cacheWriteInputTokens', total: 'totalTokens' })) {
        if (delta[native] !== undefined) usage[normalized] = (usage[normalized] ?? 0) + delta[native];
      }
      if (params.tokenUsage.modelContextWindow != null) usage.contextWindow = params.tokenUsage.modelContextWindow;
      if (root) turn.usage = usage; else turn.childUsage.set(params.threadId, usage);
      send(turn, { type: 'usage.updated', usage }, params);
    } else if (method === 'model/rerouted') {
      if (root) { turn.model = params.toModel; sessions.set(turn.sessionId, { ...sessions.get(turn.sessionId), model: params.toModel }); }
      send(turn, { type: 'model.updated', model: params.toModel }, params);
    } else if (method === 'thread/settings/updated') {
      if (root) turn.model = params.threadSettings.model;
      send(turn, { type: 'model.updated', model: params.threadSettings.model }, params);
    } else if (method === 'thread/status/changed') {
      send(turn, { type: 'runtime.status', status: params.status.type, ...(params.status.activeFlags?.length ? { message: params.status.activeFlags.join(', ') } : {}) }, params);
    } else if (method === 'error') send(turn, { type: 'runtime.status', status: params.willRetry ? 'retrying' : 'error', message: params.error?.message }, params);
    else if (method === 'turn/started') send(turn, { type: 'runtime.status', status: 'running' }, params);
    else if (method === 'turn/completed') {
      for (const item of params.turn.items ?? []) projectItem(turn, item, true, params);
      if (!root) {
        send(turn, { type: 'child.updated', scope: turn.scopes.get(params.threadId), status: params.turn.status === 'failed' ? 'error' : params.turn.status === 'inProgress' ? 'running' : params.turn.status }, params);
        return;
      }
      const finish = params.turn.status;
      send(turn, { type: 'runtime.status', status: finish, ...(params.turn.error ? { message: params.turn.error.message } : {}) }, params);
      turn.finished = true;
      turn.done.resolve(defined({ finish, model: turn.model, usage: turn.usage }));
    }
  }
  async function interaction(method, params, id) {
    const turn = owner(params);
    if (!turn || turn.finished) throw new Error('Codex interaction has no active owning turn');
    // A server request may race the turn/start response. Never bind it to a later start.
    if (!turn.id) await turn.started.promise;
    if (turn.finished || (params.threadId === turn.sessionId && params.turnId !== turn.id)) throw new Error('Stale Codex turn interaction');
    const ask = async (name, payload) => {
      const answer = await turn.request(name, payload);
      if (turn.finished) throw new Error('Codex turn ended while awaiting interaction');
      return answer;
    };
    if (method === 'item/tool/requestUserInput') {
      if (params.questions.some(q => q.isSecret)) throw new Error('Secret Codex input is unsupported');
      const result = await ask('question.request', { id: String(id), toolCallId: params.itemId, questions: params.questions.map(q => defined({ id: q.id, question: q.question, header: q.header, options: q.options, multiple: false, custom: q.isOther })) });
      return { answers: Object.fromEntries(Object.entries(result.answers).map(([key, answers]) => [key, { answers }])) };
    }
    if (['item/commandExecution/requestApproval', 'item/fileChange/requestApproval', 'item/permissions/requestApproval'].includes(method)) {
      const result = await ask('permission.request', { id: String(id), toolCallId: params.itemId, toolName: method.split('/')[1], title: params.reason ?? params.command ?? 'Codex approval', input: params });
      if (!['allow_once', 'allow_session', 'deny'].includes(result.decision)) throw new Error('Invalid permission decision');
      // Read-only is a hard inherited bound, not a default that an approval may relax.
      const effectiveDecision = sandbox === 'read-only' ? 'deny' : result.decision;
      if (method === 'item/permissions/requestApproval') return { permissions: effectiveDecision === 'deny' ? {} : params.permissions, scope: effectiveDecision === 'allow_session' ? 'session' : 'turn' };
      const decision = { allow_once: 'accept', allow_session: 'acceptForSession', deny: 'decline' }[effectiveDecision];
      if (params.availableDecisions && !params.availableDecisions.includes(decision)) throw new Error(`Codex does not offer approval decision ${decision}`);
      return { decision };
    }
    throw Object.assign(new Error(`Unsupported Codex request: ${method}`), { code: -32601 });
  }
  async function pages(method, params) {
    const data = [], seen = new Set(); let cursor;
    do {
      const page = await rpc.request(method, defined({ ...params, cursor, limit: 100, sortDirection: 'asc' }));
      data.push(...page.data); cursor = page.nextCursor;
      if (cursor && seen.has(cursor)) throw new Error('Codex history pagination repeated its cursor');
      seen.add(cursor);
    } while (cursor);
    return data;
  }
  return {
    async describe({ selection = {} } = {}) {
      const { auth, models } = await discover();
      const model = selection.model ? models.find(m => m.model === selection.model || m.id === selection.model) : models.find(m => m.isDefault) ?? models[0];
      if (selection.model && !model) throw new Error(`Unsupported Codex model: ${selection.model}`);
      return { capabilities, auth, models: models.map(m => ({ id: m.model, name: m.displayName })), options: model ? [{ id: 'reasoningEffort', name: 'Reasoning effort', type: 'select', currentValue: model.defaultReasoningEffort, options: model.supportedReasoningEfforts.map(e => ({ value: e.reasoningEffort, name: e.reasoningEffort })) }] : [], commands: [] };
    },
    async open({ sessionId, selection } = {}) {
      if (active.has(sessionId)) throw new Error('Cannot reopen Codex during an active turn');
      const requested = await selectionParams(selection);
      const response = await rpc.request(sessionId ? 'thread/resume' : 'thread/start', { ...threadParams(requested), ...nativePolicy(), ...(sessionId ? { threadId: sessionId } : {}) });
      return confirm(response, requested, sessionId);
    },
    async configure({ sessionId, selection }) {
      await connect();
      if (active.has(sessionId)) throw new Error('Cannot configure Codex during an active turn');
      if (!sessions.has(sessionId)) throw new Error('Open the Codex thread before configuring it');
      const requested = await selectionParams(selection, sessions.get(sessionId));
      const response = await rpc.request('thread/resume', { threadId: sessionId, ...threadParams(requested), ...nativePolicy() });
      const confirmed = confirm(response, requested, sessionId);
      return { model: confirmed.model, options: confirmed.options };
    },
    async start({ sessionId, text, selection, emit, request }) {
      await connect();
      if (active.has(sessionId)) throw new Error('Codex thread already has an active turn');
      if (!sessions.has(sessionId)) throw new Error('Open or explicitly resume the Codex thread before starting');
      const chosen = await selectionParams(selection, sessions.get(sessionId));
      if (closed) throw new Error('Codex adapter is closed');
      if (active.has(sessionId)) throw new Error('Codex thread already has an active turn');
      const turn = { sessionId, id: null, emit, request, done: deferred(), started: deferred(), early: [], items: new Map(), scopes: new Map(), childUsage: new Map(), model: chosen.model, finished: false };
      // Attach handlers before requests, including rejection handlers for startup failures.
      turn.done.promise.catch(() => {}); turn.started.promise.catch(() => {});
      active.set(sessionId, turn);
      try {
        const result = await rpc.request('turn/start', defined({ threadId: sessionId, input: inputText(text), model: chosen.model, effort: chosen.options.reasoningEffort, ...nativePolicy(true) }));
        if (!result.turn?.id) throw new Error('Codex turn/start returned no turn ID');
        turn.id = result.turn.id; turn.started.resolve();
        sessions.set(sessionId, chosen);
        for (const [method, params] of turn.early) notification(method, params);
        turn.early = [];
        if (result.turn.status !== 'inProgress' && !turn.finished) notification('turn/completed', { threadId: sessionId, turn: result.turn });
        return await turn.done.promise;
      } catch (error) { turn.started.reject(error); throw error; }
      finally {
        turn.finished = true;
        if (active.get(sessionId) === turn) active.delete(sessionId);
        for (const [id, owner] of childOwners) if (owner === turn) childOwners.delete(id);
      }
    },
    async interrupt({ sessionId }) {
      const turn = active.get(sessionId);
      if (!turn) throw new Error('No active Codex turn to interrupt');
      await turn.started.promise;
      await rpc.request('turn/interrupt', { threadId: sessionId, turnId: turn.id });
      let timer;
      try {
        const result = await Promise.race([turn.done.promise, new Promise((_, reject) => { timer = setTimeout(() => reject(new Error('Codex did not confirm interruption')), 15000); })]);
        if (result.finish !== 'interrupted') throw new Error(`Codex turn ended ${result.finish}, not interrupted`);
        return { interrupted: true };
      } finally { clearTimeout(timer); }
    },
    async steer({ sessionId, text }) {
      const turn = active.get(sessionId);
      if (!turn || turn.finished) throw new Error('No active Codex turn to steer');
      await turn.started.promise;
      return rpc.request('turn/steer', { threadId: sessionId, expectedTurnId: turn.id, input: inputText(text) });
    },
    async list({ cwd, cursor } = {}) {
      await connect();
      const page = await rpc.request('thread/list', defined({ cwd, cursor, limit: 100, sortKey: 'updated_at' }));
      return { sessions: page.data.filter(t => typeof t.cwd === 'string' && (!cwd || t.cwd === cwd)).map(t => defined({ sessionId: t.id, cwd: t.cwd, title: t.name ?? t.preview, updatedAt: t.updatedAt })), ...(page.nextCursor ? { nextCursor: page.nextCursor } : {}) };
    },
    async history({ sessionId, cwd }) {
      await connect();
      let { thread } = await rpc.request('thread/read', { threadId: sessionId, includeTurns: false });
      if (thread.id !== sessionId || !thread.cwd || (cwd && cwd !== thread.cwd)) throw new Error('Codex history thread or cwd mismatch');
      if (thread.historyMode !== 'paginated') ({ thread } = await rpc.request('thread/read', { threadId: sessionId, includeTurns: true }));
      const turns = thread.historyMode === 'paginated' ? await pages('thread/turns/list', { threadId: sessionId }) : thread.turns;
      const messages = [];
      for (const nativeTurn of turns) {
        const entries = nativeTurn.itemsView && nativeTurn.itemsView !== 'full' ? (await pages('thread/items/list', { threadId: sessionId, turnId: nativeTurn.id })).map(e => e.item) : nativeTurn.items;
        const events = [];
        const projection = { sessionId, id: nativeTurn.id, items: new Map(), scopes: new Map(), history: true, emit: e => events.push(e) };
        for (const item of entries) {
          if (item.type === 'userMessage') messages.push({ id: item.id, role: 'user', events: [{ type: 'text.completed', id: item.id, text: item.content.filter(c => c.type === 'text').map(c => c.text).join('\n'), sessionId, turnId: nativeTurn.id }] });
          else projectItem(projection, item, true, { threadId: sessionId, turnId: nativeTurn.id });
        }
        if (events.length) messages.push({ id: nativeTurn.id, role: 'assistant', events });
        for (const [id, owner] of childOwners) if (owner === projection) childOwners.delete(id);
      }
      return { sessionId, cwd: thread.cwd, messages, complete: true };
    },
    close() {
      if (closing) return closing;
      closed = true;
      for (const turn of active.values()) { const error = new Error('Codex adapter closed'); turn.done.reject(error); turn.started.reject(error); }
      active.clear(); childOwners.clear();
      closing = rpc ? rpc.close() : Promise.resolve();
      return closing;
    },
  };
}