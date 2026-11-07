import test from 'node:test';
import assert from 'node:assert/strict';
import { spawnJsonRpc } from './json-rpc.mjs';

const fake = `
import { createInterface } from 'node:readline';
const send = x => process.stdout.write(JSON.stringify(x) + '\\n');
createInterface({input:process.stdin}).on('line', line => {
 const m = JSON.parse(line);
 if (m.method === 'initialize') { send({id:m.id,result:{ready:true}}); send({method:'ready',params:{ok:true}}); }
 if (m.method === 'reverse') { send({id:'server-id',method:'approval',params:{itemId:'command'}}); send({id:m.id,result:{queued:true}}); }
 if (m.id === 'server-id') send({method:'approvalResult',params:m});
 if (m.method === 'error') send({id:m.id,error:{code:123,message:'native error',data:{why:'test'}}});
 if (m.method === 'exit') process.exit(7);
 if (m.method === 'malformed') process.stdout.write('not JSON\\n');
});
`;
const transport = () => spawnJsonRpc({ executable: process.execPath, cwd: process.cwd(), args: ['--input-type=module', '-e', fake] });
test('real subprocess NDJSON responses, notifications, reverse requests, native errors and cleanup', async t => {
  const rpc = transport(); t.after(() => rpc.close());
  const events = [];
  let answer;
  const approval = new Promise(resolve => { answer = resolve; });
  rpc.onNotification((method, params) => { events.push([method, params]); if (method === 'approvalResult') answer(params); });
  rpc.setRequestHandler(async (method, params, id) => { assert.equal(method, 'approval'); assert.equal(params.itemId, 'command'); assert.equal(id, 'server-id'); return { decision: 'accept' }; });
  assert.deepEqual(await rpc.request('initialize'), { ready: true });
  await rpc.request('reverse');
  assert.deepEqual((await approval).result, { decision: 'accept' });
  assert.ok(events.some(e => e[0] === 'ready'));
  await assert.rejects(rpc.request('error'), error => error.code === 123 && error.details.why === 'test');
  await Promise.all([rpc.close(), rpc.close()]);
  await assert.rejects(rpc.request('closed'), /closed/);
});
test('subprocess death rejects pending requests and reports failure', async t => {
  const rpc = transport(); t.after(() => rpc.close());
  let failure;
  rpc.onFailure(e => { failure = e; });
  await assert.rejects(rpc.request('exit'), /exited \(7\)/);
  assert.match(failure.message, /exited/);
});
test('malformed protocol fails pending requests rather than hanging', async t => {
  const rpc = transport(); t.after(() => rpc.close());
  await assert.rejects(rpc.request('malformed'), /Invalid JSON/);
});
test('relative executables are rejected; no shell or npm resolution', () => {
  assert.throws(() => spawnJsonRpc({ executable: 'codex', cwd: process.cwd() }), /absolute/);
});