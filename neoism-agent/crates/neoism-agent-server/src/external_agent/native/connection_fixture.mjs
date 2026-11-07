import { createInterface } from 'node:readline';
import { appendFileSync, existsSync, readFileSync } from 'node:fs';

const send = value => process.stdout.write(JSON.stringify({ jsonrpc: '2.0', ...value }) + '\n');
let turn;
let provider;
const describe = (selection = {}) => ({ models: ['native-model', 'base', 'fast'].map(id => ({ id, name: id })),
  options: selection.model === 'fast' ? [{ id: 'effort', name: 'Effort', type: 'select', currentValue: 'deep', options: ['deep', 'shallow'].map(value => ({ value, name: value })) }] : [],
  commands: [{ name: 'review', description: 'Review' }], capabilities: { resume: true, history: true, list: true, modelSelection: true,
    completeHistory: provider !== 'claude', ...catalog().capabilities } });
const catalog = () => existsSync('catalog-fixture.json') ? JSON.parse(readFileSync('catalog-fixture.json', 'utf8')) : {};
const lines = createInterface({ input: process.stdin });
lines.on('line', line => {
  const message = JSON.parse(line);
  if (provider || message.method === 'runtime.initialize') appendFileSync('calls.log', JSON.stringify(message) + '\n');
  if (message.method === 'runtime.initialize') {
    provider = message.params.provider;
    appendFileSync('host-initializations', provider + '\n');
    if (existsSync('fail')) send({ id: message.id, error: { code: -32000, message: 'fixture initialization failed' } });
    else send({ id: message.id, result: describe() });
  } else if (message.method === 'runtime.describe') {
    send({ id: message.id, result: describe(message.params.selection) });
  } else if (message.method === 'conversation.open') {
    send({ id: message.id, result: { sessionId: message.params.sessionId ?? 'native-' + provider, model: 'native-model', options: {} } });
  } else if (message.method === 'conversation.list') {
    send({ id: message.id, result: { sessions: catalog().sessions ?? [] } });
  } else if (message.method === 'conversation.history') {
    send({ id: message.id, result: catalog().history ?? { sessionId: message.params.sessionId, cwd: process.cwd(), messages: [], complete: false } });
  } else if (message.method === 'turn.start') {
    turn = message.id;
    send({ method: 'runtime.event', params: { operationId: String(turn), event: { type: 'text.delta', id: 'answer', text: 'early' } } });
    if (message.params.text === 'background') {
      send({ method: 'runtime.event', params: { operationId: String(turn), event: { type: 'child.updated', scope: { id: 'child', sessionId: 'native-child', parentToolCallId: 'spawn' }, status: 'running' } } });
    }
    send({ id: 'permission:1', method: 'permission.request', params: { operationId: String(turn), toolName: 'read', input: { path: 'README.md' } } });
  } else if (message.id === 'permission:1') {
    send({ method: 'runtime.event', params: { operationId: String(turn), event: { type: 'text.completed', id: 'answer', text: 'early' } } });
    send({ id: turn, result: { finish: 'completed', approval: message.result } });
  } else if (message.method === 'fail') {
    send({ id: message.id, error: { code: -32000, message: 'native failure' } });
  } else if (message.method === 'die') {
    process.stderr.write('fixture process died\n', () => process.exit(7));
  } else if (message.method === 'runtime.close') {
    send({ id: message.id, result: {} });
    lines.close(); process.stdin.destroy();
  }
});