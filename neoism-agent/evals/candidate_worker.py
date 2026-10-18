"""Untrusted execution endpoint; emits data only, never verification/scoring."""
import importlib
import json
import os
import sys

MAX_BYTES = 1024 * 1024


def encode(value):
    chunks = []
    size = 0
    for chunk in json.JSONEncoder(allow_nan=False, ensure_ascii=True).iterencode(value):
        chunk = chunk.encode('utf-8')
        size += len(chunk)
        if size > MAX_BYTES:
            raise ValueError('JSON exceeds 1 MiB')
        chunks.append(chunk)
    return b''.join(chunks)


def snapshot(result, args):
    # Materialize now, not after later probes have changed shared objects.
    return json.loads(encode({'result': result, 'args': args}))


class SinglePass:
    def __init__(self, records):
        self.records = records
        self.started = False

    def __iter__(self):
        if self.started:
            raise AssertionError('Input iterated more than once')
        self.started = True
        return self._records()

    def _records(self):
        record = {}
        for seed in self.records:
            record.clear()
            record.update(seed)
            yield record


def execute(request):
    args = request['args']
    if not isinstance(args, list):
        raise ValueError('args must be a list')
    invocation_args = list(args)
    mode = request['iterator']
    if mode is not None:
        if not args or not isinstance(args[0], list):
            raise ValueError('iterator requires seed records in args[0]')
        if mode == 'generator':
            invocation_args[0] = (record for record in args[0])
        elif mode == 'single_pass_reused':
            invocation_args[0] = SinglePass(args[0])
        else:
            raise ValueError('Unknown iterator mode')
    function = getattr(importlib.import_module(request['module']), request['function'])
    result = function(*invocation_args)
    initial = snapshot(result, args)
    snapshots = []
    for probe in request['mutations']:
        target = probe['target']
        if target not in ('result', 'args'):
            raise ValueError('Unknown mutation target')
        root = result if target == 'result' else args
        path = probe['path']
        if not isinstance(path, list):
            raise ValueError('Mutation path must be a list')
        if probe['op'] == 'append':
            selected = root
            for key in path:
                selected = selected[key]
            selected.append(probe['value'])
        elif probe['op'] == 'set':
            if path:
                selected = root
                for key in path[:-1]:
                    selected = selected[key]
                selected[path[-1]] = probe['value']
            elif target == 'result':
                result = probe['value']
            else:
                args = probe['value']
        else:
            raise ValueError('Unknown mutation operation')
        snapshots.append(snapshot(result, args))
    return {'result': initial['result'], 'args_after': initial['args'],
            'snapshots': snapshots}


def main():
    # Ordinary candidate prints must not become protocol output. This is not an
    # authentication boundary: the entire child response remains untrusted data.
    protocol_fd = os.dup(1)
    with open(os.devnull, 'wb') as sink:
        os.dup2(sink.fileno(), 1)
    raw = sys.stdin.buffer.read(MAX_BYTES + 1)
    if len(raw) > MAX_BYTES:
        raise ValueError('Request exceeds 1 MiB')
    request = json.loads(raw)
    response = encode(execute(request))
    with os.fdopen(protocol_fd, 'wb') as stream:
        stream.write(response)


if __name__ == '__main__':
    main()
