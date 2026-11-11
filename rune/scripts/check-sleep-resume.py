#!/usr/bin/env python3
"""A real Rune window loses its daemon mid-reply, the way a closed lid does, and comes back.

The window runs on a headless Hyprland output against the running local daemon and sends a
real prompt (`NEOISM_RUNE_DRIVE`), so nothing takes the keyboard or pointer. Its connection
goes through a small TCP relay in this script, which can go dark without closing anything:
what a link to another machine looks like from a laptop that slept. Then:

  sleep    the relay goes dark and the app is stopped (SIGSTOP) while the reply streams; a few
           seconds later both come back. The app must notice it woke, find its socket dead on
           the short deadline, dial again and carry on with the reply where it is live.
  wake     only the app is stopped: the lid of a machine whose daemon is local. The socket
           survives; the app must still bring itself up to date.
  away     the relay stays dark until the reply has finished, and the app is never stopped:
           a network that dropped. The quiet-connection ping must find it.
  steady   nothing happens to it: what the comparison below says about an undisturbed reply.
  subagent like `sleep`, with a prompt that starts a sub-agent and the window showing it.

Afterwards a second window opens the same conversation fresh from history, and the two
transcripts must match row for row: nothing lost, nothing twice, no stale text.

    rune/app/build.sh build --release
    python3 rune/scripts/check-sleep-resume.py sleep wake away
"""
import argparse, json, os, re, select, shlex, signal, socket, subprocess, sys, threading, time
from pathlib import Path

PROMPT = ('Write 300 numbered one-sentence facts about the ocean, each on its own line. '
          'Do not use any tools and do not ask anything.')
CHILD_PROMPT = ('Use the task tool to start exactly one sub-agent with this job: "Write 120 numbered one-sentence facts '
                'about rivers, each on its own line, without using any tools." Then repeat its last fact and stop.')

p = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
p.add_argument('scenarios', nargs='+', choices=['steady', 'sleep', 'wake', 'away', 'subagent'])
p.add_argument('--binary', default='/tmp/neoism-rune-target/release/neoism-rune')
p.add_argument('--cwd', default='/tmp/neoism-rune-verify/sleep-workspace', help='Workspace the test chats are created in')
p.add_argument('--out', default='/tmp/neoism-rune-verify/sleep-resume')
p.add_argument('--output', default='neoperf', help='Headless output to use (created and removed if missing)')
p.add_argument('--asleep', type=float, default=8, help='Seconds the lid stays closed')
p.add_argument('--turn', type=int, default=170, help='Seconds allowed for the reply to finish')
a = p.parse_args()

def hypr(*args): return subprocess.run(['hyprctl', *args], check=True, capture_output=True, text=True).stdout

class Relay:
    """Forwards to the daemon. `dark()`: every open connection stops carrying anything, for
    good and without a word, and new ones are turned away. `light()`: new ones pass again."""
    def __init__(self, upstream):
        self.upstream, self.is_dark, self.pairs, self.dead = upstream, False, [], []
        self.server = socket.create_server(('127.0.0.1', 0))
        self.port = self.server.getsockname()[1]
        threading.Thread(target=self.accept, daemon=True).start()
    def accept(self):
        while True:
            client, _ = self.server.accept()
            if self.is_dark: client.close(); continue
            try: daemon = socket.create_connection(self.upstream)
            except OSError: client.close(); continue
            pair = {'client': client, 'daemon': daemon, 'cut': False}
            self.pairs.append(pair)
            threading.Thread(target=self.pump, args=(pair,), daemon=True).start()
    def pump(self, pair):
        client, daemon = pair['client'], pair['daemon']
        try:
            while not pair['cut']:
                for ready in select.select([client, daemon], [], [], .05)[0]:
                    data = ready.recv(1 << 16)
                    if not data: raise OSError
                    if pair['cut']: return
                    (daemon if ready is client else client).sendall(data)
        except OSError: pass
        finally:
            if pair['cut']: return
            client.close(); daemon.close()
    def dark(self):
        self.is_dark = True
        for pair in self.pairs:
            pair['cut'] = True
            # The app's side stays open and silent; the daemon is let go of.
            self.dead.append(pair['client'])
            pair['daemon'].close()
        self.pairs = []
    def light(self):
        self.is_dark = False

def launch(name, state, drive, env_extra, workspace):
    log = Path(f'{a.out}.{name}.log')
    state_path = Path(f'{a.out}.{name}.state.json')
    state_path.write_text(json.dumps(state))
    app_id = f'ai.neoism.rune.sleep{os.getpid()}{name.replace("-", "")}'
    env = [f'NEOISM_RUNE_STATE_PATH={state_path}', f'NEOISM_RUNE_APP_ID={app_id}', 'NEOISM_RUNE_TRACE_DAEMON=1', 'NEOISM_RUNE_TRACE_BODIES=1', f'NEOISM_RUNE_DRIVE={drive}', *env_extra]
    command = f'cd {shlex.quote(a.cwd)} && ' + shlex.join(['env', *env, 'timeout', '--signal=INT', str(a.turn + 90), a.binary]) + f' > {shlex.quote(str(log))} 2>&1'
    hypr('dispatch', f'hl.dsp.exec_cmd({json.dumps(command)}, {{ workspace = {json.dumps(str(workspace) + " silent")} }})')
    for _ in range(100):
        pid = next((c['pid'] for c in json.loads(hypr('-j', 'clients')) if c.get('class') == app_id), None)
        if pid: return pid, log
        time.sleep(.1)
    raise SystemExit(f'the test window never appeared; see {log}')

def wait_for(log, pattern, timeout, count=1, start=0):
    """The time `pattern` has shown `count` times in `log` past line `start`, else None."""
    deadline = time.monotonic() + timeout
    while time.monotonic() < deadline:
        lines = log.read_text(errors='replace').splitlines()[start:] if log.exists() else []
        if sum(1 for line in lines if re.search(pattern, line)) >= count: return time.monotonic()
        time.sleep(.05)
    return None

def wait_exit(pid, timeout):
    deadline = time.monotonic() + timeout
    while Path(f'/proc/{pid}').exists() and time.monotonic() < deadline: time.sleep(.2)
    return not Path(f'/proc/{pid}').exists()

def rows_of(log):
    """`(session, streaming, connection, [(kind, id, length, hash)])` of the log's last `rows` step."""
    found = None
    for line in log.read_text(errors='replace').splitlines():
        if m := re.match(r'\[drive\] rows session=(\S+) streaming=(\w+) live_from=\S+ connection=(.*?) ((?:\w+:\S+:\d+:[0-9a-f]{8} ?)*)$', line):
            found = (m[1], m[2] == 'true', m[3], [(row.split(':', 1)[0], *row.split(':', 1)[1].rsplit(':', 2)) for row in m[4].split()])
    return found

def agent_tab(session): return {'version': 1, 'active': 0, 'workspaces': [{'name': Path(a.cwd).name, 'cwd': a.cwd,
    'tabs': [{'kind': 'agent', 'session': session, 'draft': ''}], 'layout': {'kind': 'pane', 'tabs': [0], 'active': 0, 'focused': True},
    'active': 0, 'sidebar': None, 'unsaved': {}}]}

def run(scenario, relay, workspace):
    try: return disturb(scenario, relay, workspace)
    finally: relay.server.close()

def disturb(scenario, relay, workspace):
    child = scenario == 'subagent'
    prompt = CHILD_PROMPT if child else PROMPT
    drive = f'wait:3;prompt:{prompt};' + ('child;' if child else '') + f'wait:{a.turn};rows;quit'
    pid, log = launch(scenario, agent_tab(None), drive, [f'NEOISM_GPUIX_DAEMON_URL=ws://127.0.0.1:{relay.port}/session'], workspace)
    problems, notes = [], []
    try:
        if child and not wait_for(log, r'\[drive\] wait:', 90, count=2): return [f'no sub-agent started; see {log}'], notes
        # Well into the reply (the sub-agent's, when one is being watched).
        if not wait_for(log, r'\[daemon\] AgentReply ContentDelta', 90, count=40): return [f'the reply never streamed; see {log}'], notes
        before = len(log.read_text(errors='replace').splitlines())
        if scenario == 'steady':
            if not wait_exit(pid, a.turn + 60): problems.append('the window did not finish its script')
            return compare(scenario, log, problems, notes, workspace)
        if scenario != 'wake': relay.dark()
        if scenario != 'away': os.kill(pid, signal.SIGSTOP)
        time.sleep(a.asleep)
        if scenario == 'away':
            # The network stays down until the agent has long finished: twice the quiet ping.
            lost = wait_for(log, r'link Disconnected', 30, start=before)
            if not lost: problems.append('a connection gone quiet was never given up on')
            time.sleep(max(0, a.turn / 3))
        relay.light()
        woke = time.monotonic()
        if scenario != 'away': os.kill(pid, signal.SIGCONT)
        if scenario == 'wake':
            # The socket survived, unless the daemon's queue to a stopped window overflowed,
            # which hangs up on purpose: either way the app says how it came back.
            back = wait_for(log, r'link (Woke|Connected \{ again: true \})', 20, start=before)
        else:
            back = wait_for(log, r'link Connected \{ again: true \}', 40, start=before)
            if scenario == 'sleep' or child:
                # Found by asking on waking (a few seconds), not by waiting out the quiet.
                lost = wait_for(log, r'link Disconnected\("Daemon stopped answering"\)', 0.2, start=before)
                if not lost or (back and back - woke > 12): problems.append('the dead socket was not found on the wake deadline')
        if not back: problems.append('the connection never came back')
        else:
            notes.append(f'back {back - woke:.1f} s after waking')
            live = wait_for(log, r'\[daemon\] AgentReply (ContentDelta|MessageUpdated|HistoryChunk)', 20, start=before)
            if live: notes.append(f'transcript moving again {live - woke:.1f} s after waking')
            else: problems.append('nothing reached the transcript after coming back')
        if not wait_exit(pid, a.turn + 60): problems.append('the window did not finish its script')
        tail = log.read_text(errors='replace').splitlines()[before:]
        notes.append(f'{sum(line.startswith("[daemon] AgentReply ContentDelta") for line in tail)} live deltas after coming back')
    finally:
        try: os.kill(pid, signal.SIGCONT); os.kill(pid, signal.SIGINT)
        except ProcessLookupError: pass
    return compare(scenario, log, problems, notes, workspace)

def compare(scenario, log, problems, notes, workspace):
    got = rows_of(log)
    if not got: return problems + [f'no transcript was logged; see {log}'], notes
    session, streaming, connection, rows = got
    if streaming: problems.append('the reply still reads as streaming after the turn')
    if 'Connected' not in connection or 'Dis' in connection: problems.append(f'the window ended {connection}')
    # The same conversation, opened fresh from its history.
    pid, reference = launch(f'{scenario}-reference', agent_tab(session), 'ready;wait:3;rows;quit', [], workspace)
    wait_exit(pid, 60)
    expected = rows_of(reference)
    if not expected: return problems + [f'the reference window logged no transcript; see {reference}'], notes
    want, have = expected[3], rows
    notes.append(f'{len(have)} rows, {sum(int(r[2]) for r in have)} characters; fresh from history: {len(want)} rows, {sum(int(r[2]) for r in want)} characters')
    have_by_id = {r[1]: r for r in have}
    for row in want:
        mine = have_by_id.get(row[1])
        if mine is None: problems.append(f'missing row {row[0]} {row[1][-6:]} ({row[2]} characters)')
        elif mine[2:] != row[2:]: problems.append(f'row {row[0]} {row[1][-6:]} differs: {mine[2]} characters here, {row[2]} in history')
    wanted = {r[1] for r in want}
    for row in have:
        if row[1] not in wanted: problems.append(f'row {row[0]} {row[1][-6:]} ({row[2]} characters) is not in the history')
    if len({r[1] for r in have}) != len(have): problems.append('a row is shown twice')
    shared = [r[1] for r in have if r[1] in wanted]
    if shared != [r[1] for r in want if r[1] in have_by_id]: problems.append('rows are in another order than the history')
    return problems, notes

endpoint = json.loads((Path(os.environ['XDG_RUNTIME_DIR']) / 'neoism/daemon-endpoint.json').read_text())
host, port = endpoint['address'].rsplit(':', 1)
Path(a.cwd).mkdir(parents=True, exist_ok=True)
Path(a.out).parent.mkdir(parents=True, exist_ok=True)
created = a.output not in {m['name'] for m in json.loads(hypr('-j', 'monitors'))}
if created:
    hypr('output', 'create', 'headless', a.output)
    time.sleep(5)
failed = False
try:
    workspace = next(m for m in json.loads(hypr('-j', 'monitors')) if m['name'] == a.output)['activeWorkspace']['id']
    for scenario in a.scenarios:
        problems, notes = run(scenario, Relay((host, int(port))), workspace)
        print(f'{scenario}: {"FAILED" if problems else "ok"}')
        for note in notes: print(f'    {note}')
        for problem in problems: print(f'    PROBLEM: {problem}')
        failed |= bool(problems)
finally:
    if created: hypr('output', 'remove', a.output)
sys.exit(1 if failed else 0)
