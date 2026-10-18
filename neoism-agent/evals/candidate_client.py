"""Trusted, stdlib-only transport. Never import candidate code in this process."""
import json
import os
import selectors
import signal
import subprocess
import time

WORKER_PATH = '/opt/eval/candidate_worker.py'
MAX_BYTES = 1024 * 1024
DEADLINE_SECONDS = 3


class CandidateError(AssertionError):
    pass


def _reject_constant(value):
    raise ValueError('Non-finite JSON number')


def _unique_object(pairs):
    result = {}
    for key, value in pairs:
        if key in result:
            raise ValueError('Duplicate JSON key')
        result[key] = value
    return result


def _kill_group(pid):
    try:
        os.killpg(pid, signal.SIGKILL)
    except ProcessLookupError:
        return
    except PermissionError:
        # A cap-drop=ALL container with only SETUID/SETGID cannot signal a
        # different uid, even from uid0. This isolated, trusted helper uses a
        # matching effective uid to signal the worker. Keep real/saved uid0:
        # the candidate cannot signal this helper (Linux checks the target's
        # real/saved uids, not its effective uid). Never import workspace code.
        code = (
            'import os,sys\n'
            'os.setresuid(0,65534,0)\n'
            'try: os.killpg(int(sys.argv[1]),9)\n'
            'except ProcessLookupError: pass\n'
        )
        try:
            subprocess.run(
                ['/usr/bin/python3', '-I', '-S', '-c', code, str(pid)],
                stdin=subprocess.DEVNULL, stdout=subprocess.DEVNULL,
                stderr=subprocess.DEVNULL, cwd='/', env={}, close_fds=True,
                start_new_session=True, check=True, timeout=1,
            )
        except (OSError, subprocess.SubprocessError) as exc:
            raise RuntimeError('Unable to clean up candidate process group') from exc


def call(module, function, args, iterator=None, mutations=None):
    """Return {result, args_after, snapshots}; all values cross a JSON boundary.

    iterator: None, 'generator', or 'single_pass_reused' (wraps args[0]).
    mutations: sequential {target: 'result'|'args', path: [keys/indices],
    op: 'set'|'append', value: JSON}. Each produces a {result, args} snapshot.
    A set replaces the selected value (including a root with path=[]).
    """
    if os.geteuid() != 0:
        raise RuntimeError('Candidate supervisor must run as root')
    request = {'module': module, 'function': function, 'args': args,
               'iterator': iterator, 'mutations': mutations or []}
    payload = json.dumps(request, allow_nan=False, ensure_ascii=True).encode('utf-8')
    if len(payload) > MAX_BYTES:
        raise CandidateError('Candidate request exceeds 1 MiB')
    deadline = time.monotonic() + DEADLINE_SECONDS
    try:
        process = subprocess.Popen(
            ['/usr/bin/python3', WORKER_PATH],
            stdin=subprocess.PIPE, stdout=subprocess.PIPE, stderr=subprocess.DEVNULL,
            cwd='/workspace', env={'PYTHONPATH': '/workspace', 'PATH': '/usr/bin:/bin',
                                   'LANG': 'C.UTF-8'},
            user=65534, group=65534, extra_groups=[], start_new_session=True,
            close_fds=True,
        )
    except OSError as exc:
        raise RuntimeError('Unable to launch candidate worker') from exc
    assert process.stdin is not None and process.stdout is not None
    output = bytearray()
    try:
        with selectors.DefaultSelector() as selector:
            os.set_blocking(process.stdin.fileno(), False)
            os.set_blocking(process.stdout.fileno(), False)
            selector.register(process.stdin, selectors.EVENT_WRITE)
            selector.register(process.stdout, selectors.EVENT_READ)
            sent = 0
            while selector.get_map():
                remaining = deadline - time.monotonic()
                if remaining <= 0:
                    raise CandidateError('Candidate deadline exceeded')
                events = selector.select(remaining)
                for key, _ in events:
                    if key.fileobj is process.stdin:
                        try:
                            sent += os.write(process.stdin.fileno(), payload[sent:])
                        except BrokenPipeError:
                            sent = len(payload)
                        if sent == len(payload):
                            selector.unregister(process.stdin)
                            process.stdin.close()
                    else:
                        chunk = os.read(process.stdout.fileno(), min(65536, MAX_BYTES + 1 - len(output)))
                        output.extend(chunk)
                        if len(output) > MAX_BYTES:
                            raise CandidateError('Candidate response exceeds 1 MiB')
                        if not chunk:
                            selector.unregister(process.stdout)
            try:
                status = process.wait(timeout=max(0, deadline - time.monotonic()))
            except subprocess.TimeoutExpired:
                raise CandidateError('Candidate deadline exceeded') from None
        if status != 0:
            raise CandidateError('Candidate worker failed')
        try:
            response = json.loads(output.decode('utf-8'), parse_constant=_reject_constant,
                                  object_pairs_hook=_unique_object)
        except (ValueError, RecursionError):
            raise CandidateError('Invalid candidate JSON response') from None
        if (not isinstance(response, dict)
                or set(response) != {'result', 'args_after', 'snapshots'}
                or not isinstance(response['args_after'], list)
                or len(response['args_after']) != len(args)
                or not isinstance(response['snapshots'], list)
                or len(response['snapshots']) != len(request['mutations'])
                or any(not isinstance(item, dict) or set(item) != {'result', 'args'}
                       or not isinstance(item['args'], list) or len(item['args']) != len(args)
                       for item in response['snapshots'])):
            raise CandidateError('Invalid candidate response schema')
        return response
    finally:
        # Also kill descendants holding pipes open, on success or failure.
        try:
            _kill_group(process.pid)
        finally:
            process.stdin.close()
            process.stdout.close()
            try:
                process.wait(timeout=1)
            except subprocess.TimeoutExpired as exc:
                raise RuntimeError('Unable to reap candidate worker') from exc
