// Isolated, real-image smoke test. Leaves its container and volumes for GUI joining.
// Usage: node scripts/test-docker-release.mjs [--existing]
import assert from 'node:assert/strict';
import { execFileSync } from 'node:child_process';
import { randomBytes } from 'node:crypto';
import { setTimeout as delay } from 'node:timers/promises';

const name = process.env.NEOISM_TEST_CONTAINER ?? 'neoism-release-test-0150';
const image = process.env.NEOISM_TEST_IMAGE ?? 'ghcr.io/parkers0405/neoism-daemon:0.7.150';
const port = process.env.NEOISM_TEST_PORT ?? '19876';
const base = `http://127.0.0.1:${port}`;
const directory = '/var/lib/neoism/workspaces';
let workspaceId;
const sockets = new Set();
const docker = (...args) => execFileSync('docker', args, { encoding: 'utf8', timeout: 90_000 });
let token;
if (process.argv.includes('--existing')) {
  token = docker('inspect', '--format', '{{range .Config.Env}}{{println .}}{{end}}', name)
    .split('\n').find(line => line.startsWith('NEOISM_DAEMON_TOKEN=')).split('=').slice(1).join('=');
} else {
  token = randomBytes(24).toString('hex');
  const env = { ...process.env, NEOISM_DAEMON_TOKEN: token, NEOISM_AGENT_TOKEN: token };
  const mounts = ['state', 'data', 'config', 'workspaces'].flatMap(kind =>
    ['--mount', `type=volume,source=${name}-${kind},target=/var/lib/neoism/${kind}`]);
  execFileSync('docker', ['run', '-d', '--name', name, '--cpus', '2', '--memory', '2g',
    '-p', `127.0.0.1:${port}:9876`, '--env', 'NEOISM_DAEMON_TOKEN', '--env', 'NEOISM_AGENT_TOKEN',
    '--env', 'NEOISM_REQUIRE_AUTH=1', ...mounts, image,
    'neoism-workspace-daemon', '--state-dir', '/var/lib/neoism/state', '--workspace', directory],
    { env, stdio: ['ignore', 'pipe', 'pipe'], timeout: 90_000 });
}
console.log(`Testing ${name} at ${base}; no host directories or credentials mounted.`);

async function eventually(fn, description, ms = 60_000) {
  const deadline = Date.now() + ms;
  let last;
  do {
    try { return await fn(); } catch (error) { last = error; }
    await delay(250);
  } while (Date.now() < deadline);
  throw new Error(`${description}: ${last}`);
}

async function api(path, method = 'GET', body) {
  const response = await fetch(`${base}/agent/workspaces/${workspaceId}${path}`, {
    method, headers: { authorization: `Bearer ${token}`, ...(body === undefined ? {} : { 'content-type': 'application/json' }) },
    ...(body === undefined ? {} : { body: JSON.stringify(body) }), signal: AbortSignal.timeout(20_000),
  });
  const text = await response.text();
  assert.ok(response.ok, `${method} ${path}: ${response.status} ${text.slice(0, 400)}`);
  return text ? JSON.parse(text) : undefined;
}

// Hosted guests must not change global config. Exercise operator setup only
// through the container's private loopback Agent endpoint, never public ingress.
function operatorConfig(body) {
  const args = ['exec', '-i', name, 'sh', '-c',
    `curl -fsS -H "Authorization: Bearer $NEOISM_AGENT_TOKEN" -H 'Content-Type: application/json' ${body === undefined ? '' : '-X PATCH --data-binary @-'} 'http://127.0.0.1:4096/v2/config?directory=${encodeURIComponent(directory)}'`];
  return JSON.parse(execFileSync('docker', args, { encoding: 'utf8', input: body === undefined ? undefined : JSON.stringify(body), timeout: 20_000 }));
}

async function join() {
  const socket = new WebSocket(`ws://127.0.0.1:${port}/session?token=${encodeURIComponent(token)}`);
  sockets.add(socket);
  socket.addEventListener('close', () => sockets.delete(socket));
  await new Promise((resolve, reject) => {
    const timeout = setTimeout(() => { socket.close(); reject(new Error('workspace handshake timed out')); }, 15_000);
    socket.addEventListener('open', () => socket.send(JSON.stringify({ Workspace: {
      request_id: 1, message: { Hello: { token, client_name: 'docker-release-smoke' } },
    } })));
    socket.addEventListener('message', event => {
      const value = JSON.parse(event.data);
      const body = Object.values(value)[0];
      if (body?.message?.HelloAck) socket.send(JSON.stringify({ Workspace: {
        request_id: 2, message: { ResolveInitialWorkspace: { preferred_host_id: null } },
      } }));
      if (body?.message?.InitialWorkspaceResolved) {
        workspaceId = body.message.InitialWorkspaceResolved.workspace.id;
        clearTimeout(timeout); resolve();
      }
      if (body?.message?.Error) { clearTimeout(timeout); reject(new Error(JSON.stringify(body.message.Error))); }
    });
    socket.addEventListener('error', () => { clearTimeout(timeout); reject(new Error('workspace connection failed')); }, { once: true });
  });
  return socket;
}

async function ready() {
  await eventually(async () => assert.equal((await fetch(`${base}/health`, { signal: AbortSignal.timeout(2000) })).status, 200), 'daemon readiness');
  const socket = await join();
  await eventually(() => api('/v2/health'), 'agent readiness');
  return socket;
}

try {
  let socket = await ready();
  console.log('PASS daemon health, authenticated workspace handshake, agent readiness');
  const denied = await fetch(`${base}/agent/workspaces/${workspaceId}/v2/sessions`);
  assert.ok([401, 403].includes(denied.status), `unauthenticated agent proxy returned ${denied.status}`);
  await denied.body?.cancel();
  console.log('PASS unauthenticated Agent API access rejected');
  docker('exec', name, 'sh', '-c', `mkdir -p '${directory}' && printf 'isolated release tool result\n' > '${directory}/input.txt'`);
  const abort = new AbortController();
  const stream = await fetch(`${base}/agent/workspaces/${workspaceId}/v2/events?tail=true`, { headers: { authorization: `Bearer ${token}` }, signal: abort.signal });
  assert.equal(stream.status, 200);
  const reader = stream.body.getReader();
  const event = (async () => {
    let text = '';
    while (!text.includes('session.created')) {
      const chunk = await reader.read();
      assert.equal(chunk.done, false);
      text += new TextDecoder().decode(chunk.value);
    }
  })();
  const session = await api(`/v2/sessions?directory=${encodeURIComponent(directory)}`, 'POST', { title: 'Docker release isolation test' });
  await Promise.race([event, delay(10_000).then(() => { throw new Error('SSE event deadline'); })]);
  abort.abort();
  console.log('PASS live Agent events through daemon proxy');
  await api(`/v2/sessions/${session.id}/prompt`, 'POST', {
    model: { providerId: 'neoism', modelId: 'stub' }, parts: [{ type: 'text', text: 'read-tool: input.txt' }],
  });
  await api(`/v2/sessions/${session.id}/wait`, 'POST');
  const history = await api(`/v2/sessions/${session.id}/messages`);
  assert.match(JSON.stringify(history), /isolated release tool result/);
  console.log('PASS real model/tool loop read from container workspace; no paid provider used');
  socket.close();
  socket = await join();
  assert.deepEqual(await api(`/v2/sessions/${session.id}/messages`), history);
  console.log('PASS disconnect/rejoin retains the same conversation');
  socket.close();
  docker('stop', '--time', '30', name);
  docker('start', name);
  socket = await ready();
  assert.deepEqual(await api(`/v2/sessions/${session.id}/messages`), history);
  assert.match(docker('exec', name, 'cat', `${directory}/input.txt`), /isolated release tool result/);
  console.log('PASS graceful container stop/start retains history and workspace');
  socket.close();
  docker('kill', '--signal', 'KILL', name);
  docker('start', name);
  socket = await ready();
  assert.deepEqual(await api(`/v2/sessions/${session.id}/messages`), history);
  console.log('PASS forced process loss retains completed history');
  socket.close();
  if (process.argv.includes('--recovery')) {
    const forbidden = await fetch(`${base}/agent/workspaces/${workspaceId}/v2/config`, { headers: { authorization: `Bearer ${token}` } });
    assert.equal(forbidden.status, 403);
    await forbidden.body?.cancel();
    console.log('PASS hosted workspace cannot access global operator configuration');
    const config = operatorConfig();
    operatorConfig({ ...config, dangerouslySkipPermissions: false });
    try {
      for (const mode of ['graceful', 'forced', 'stop']) {
        const active = await api('/v2/sessions', 'POST', {
          title: `Docker active recovery: ${mode}`,
          permission: [{ permission: 'read', pattern: '*', action: 'ask' }],
        });
        await api(`/v2/sessions/${active.id}/prompt`, 'POST', {
          model: { providerId: 'neoism', modelId: 'stub' }, parts: [{ type: 'text', text: 'read-tool: input.txt' }],
        });
        await eventually(async () => assert.ok((await api(`/v2/interactions/permissions?sessionId=${active.id}`)).length > 0), 'live permission wait', 15_000);
        let stopped;
        if (mode === 'stop') {
          await api(`/v2/sessions/${active.id}/abort`, 'POST');
          await api(`/v2/sessions/${active.id}/wait`, 'POST');
          stopped = await api(`/v2/sessions/${active.id}/messages`);
        }
        if (mode === 'graceful') docker('stop', '--time', '30', name);
        else docker('kill', '--signal', 'KILL', name);
        docker('start', name);
        socket = await ready();
        await api(`/v2/sessions/${active.id}/wait`, 'POST');
        const recovered = await api(`/v2/sessions/${active.id}/messages`);
        const messages = recovered.items ?? recovered;
        if (mode === 'stop') assert.deepEqual(recovered, stopped, 'explicit Stop must not resume on boot');
        else {
          assert.equal(messages.filter(message => message.info.role === 'user').length, 1);
          assert.ok(messages.filter(message => message.info.role === 'assistant').length >= 2, 'an interrupted turn must actually resume');
          assert.equal((await api(`/v2/interactions/permissions?sessionId=${active.id}`)).length, 0);
        }
        console.log(`PASS active-turn lifecycle: ${mode}`);
        socket.close();
      }
    } finally {
      operatorConfig(config);
    }
  }
  console.log(`LEFT RUNNING: ${name}; GUI endpoint ws://127.0.0.1:${port}/session`);
  console.log(`Retrieve its isolated test credential: docker exec ${name} printenv NEOISM_DAEMON_TOKEN`);
} catch (error) {
  console.error(error);
  console.error(`Container ${name} and its dedicated volumes were retained for diagnosis.`);
  process.exitCode = 1;
} finally {
  for (const socket of sockets) socket.close();
}