#!/usr/bin/env python3
"""Bounded stdio replay. Never builds servers or installs dependencies."""
import argparse
import importlib.util
import json
from pathlib import Path
import queue
import os
import shutil
import tempfile
import subprocess
import threading
import math
import time

ROOT = Path(__file__).resolve().parents[3]
HOME = Path(__file__).resolve().parent
spec = importlib.util.spec_from_file_location('interop', ROOT / 'tools/phase5/lsp/interop.py')
interop = importlib.util.module_from_spec(spec)
spec.loader.exec_module(interop)

NORMALIZATIONS = {
    'project-root': 'Scratch filesystem paths and URIs use the session placeholders.',
    'client-request-id': 'Responses are correlated with the session position, not allocated ids.',
    'server-request-id': 'Server request ids are replaced with their ordinal within that stream.',
    'diagnostic-document-stream': 'Push diagnostics preserve order per URI; cross-document scheduling is independent.',
    'pinned-exit-race': 'After a final exit, the pin\'s exit log and its "context canceled" exit status 1 are dropped.',
}

# The pin's exit handler returns io.EOF to stop the dispatch loop, which logs it
# as "error handling method 'exit': EOF", and its read, dispatch and write loops
# then race to stop (tsc/internal/lsp/server.go, Run and handleExit): the log
# reaches the client only when the writer wins, and Run returns "context
# canceled", which tsc/cmd/tsc/lsp.go turns into exit status 1, when dispatch
# wins. Neither outcome is session traffic, so after a final exit both are
# accepted; any other late traffic, stderr or status still fails.
EXIT_LOG = "error handling method 'exit'"
EXIT_RACE_STDERR = 'context canceled'


def pinned_exit_log(message):
    params = message.get('params')
    return (message.get('method') == 'window/logMessage' and isinstance(params, dict)
            and isinstance(params.get('message'), str) and params['message'].startswith(EXIT_LOG))


def substitute(value, replacements):
    if isinstance(value, str):
        for old, new in replacements:
            value = value.replace(old, new)
        return value
    if isinstance(value, list):
        return [substitute(v, replacements) for v in value]
    if isinstance(value, dict):
        return {substitute(k, replacements): substitute(v, replacements) for k, v in value.items()}
    return value


class CleanEOF(EOFError):
    """EOF exactly between frames, after the server has stopped."""


def strict_json(data):
    def pairs(entries):
        result = {}
        for key, value in entries:
            if key in result:
                raise ValueError(f'Duplicate JSON key: {key}')
            result[key] = value
        return result
    def constant(value):
        raise ValueError(f'Nonfinite JSON number: {value}')
    value = json.loads(data, object_pairs_hook=pairs, parse_constant=constant)
    def finite(v):
        if isinstance(v, float) and not math.isfinite(v):
            raise ValueError('Nonfinite JSON number')
        if isinstance(v, dict):
            for child in v.values():
                finite(child)
        elif isinstance(v, list):
            for child in v:
                finite(child)
    finite(value)
    return value


def validate_message(value):
    if not isinstance(value, dict) or value.get('jsonrpc') != '2.0':
        raise ValueError('Expected JSON-RPC 2.0 object')
    if 'id' in value and type(value['id']) not in (int, str):
        raise ValueError('Request/response id must be an integer or string')
    if 'method' in value:
        if not isinstance(value['method'], str) or 'result' in value or 'error' in value:
            raise ValueError('Malformed request/notification')
        if 'params' in value and value['params'] is not None and not isinstance(value['params'], (dict, list)):
            raise ValueError('Params must be object, array or null')
    else:
        if 'id' not in value or ('result' in value) == ('error' in value):
            raise ValueError('Response requires id and exactly one of result/error')
        if 'error' in value:
            error = value['error']
            if not isinstance(error, dict) or type(error.get('code')) is not int or not isinstance(error.get('message'), str):
                raise ValueError('Malformed response error')
    return value


def reader(stream, frames, received, wire_frames=None):
    try:
        while True:
            headers = {}
            line = stream.readline()
            if not line:
                raise CleanEOF('Server closed stdout between frames')
            while line != b'\r\n':
                if not line or not line.endswith(b'\r\n'):
                    raise EOFError('Partial or malformed frame header')
                key, separator, content = line[:-2].partition(b':')
                key = key.lower()
                if not separator or key in headers:
                    raise ValueError('Malformed or duplicate frame header')
                headers[key] = content.strip()
                line = stream.readline()
            length = headers.get(b'content-length')
            if length is None or not length.isdigit():
                raise ValueError('Missing/invalid Content-Length')
            body = stream.read(int(length))
            if wire_frames is not None:
                wire_frames.append({'headers': {key.decode('ascii'): value.decode('ascii') for key, value in headers.items()}, 'body_hex': body.hex()})
            if len(body) != int(length):
                raise EOFError('Partial frame body')
            message = validate_message(strict_json(body))
            received.append(message)
            frames.put(message)
    except Exception as error:
        frames.put(error)


def exact_equal(left, right):
    # JSON numbers are semantic values; bool is never a JSON number.
    if type(left) is not type(right):
        if type(left) in (int, float) and type(right) in (int, float):
            return left == right
        return False
    if isinstance(left, dict):
        return left.keys() == right.keys() and all(exact_equal(left[k], right[k]) for k in left)
    if isinstance(left, list):
        return len(left) == len(right) and all(exact_equal(a, b) for a, b in zip(left, right))
    return left == right


class Peer(interop.Peer):
    def __init__(self, command, cwd, stderr_path=None):
        self.stderr_file = open(stderr_path, 'w+b') if stderr_path else tempfile.TemporaryFile()
        try:
            self.process = subprocess.Popen(command, cwd=cwd, stdin=subprocess.PIPE, stdout=subprocess.PIPE, stderr=self.stderr_file)
        except BaseException:
            self.stderr_file.close()
            raise
        self.queue = queue.Queue()
        self.id = 0
        self.traffic = []
        self.received = []
        self.sent = []
        self.wire_frames = []
        self.thread = threading.Thread(target=reader, args=(self.process.stdout, self.queue, self.received, self.wire_frames), daemon=True)
        self.thread.start()

    def write(self, message):
        full = {'jsonrpc': '2.0', **message}
        validate_message(full)
        self.sent.append(full)
        super().write(message)

    def await_response(self):
        deadline = time.monotonic() + 20
        while True:
            remaining = deadline - time.monotonic()
            if remaining <= 0:
                raise TimeoutError('Response deadline expired')
            try:
                message = self.queue.get(timeout=remaining)
            except queue.Empty as error:
                raise TimeoutError('Response deadline expired') from error
            if isinstance(message, Exception):
                raise message
            if 'method' not in message:
                if type(message['id']) is not int or message['id'] != self.id:
                    raise ValueError(f'Unsolicited response: {message!r}')
                return message
            self.respond(message)

    def close(self):
        try:
            super().close()
        finally:
            for stream in (self.process.stdin, self.process.stdout, self.stderr_file):
                stream.close()

    def stderr_text(self):
        self.stderr_file.seek(0)
        return self.stderr_file.read().decode(errors='replace')

    def exchange(self, method, params=None, *, params_present=True):
        self.id += 1
        self.write({'id': self.id, 'method': method, **({'params': params} if params_present else {})})
        return self.await_response()

    def await_notification(self, method, params, count, timeout=20, replacements=()):
        """Synchronize the client without deleting or coalescing server traffic."""
        if type(count) is not int or count < 1 or not isinstance(params, dict):
            raise ValueError('Notification wait requires positive count and object params')
        def matches(message):
            message = substitute(message, replacements)
            return ('id' not in message and message.get('method') == method
                    and isinstance(message.get('params'), dict)
                    and all(key in message['params'] and exact_equal(value, message['params'][key])
                            for key, value in params.items()))
        deadline = time.monotonic() + timeout
        while sum(matches(message) for message in self.traffic) < count:
            remaining = deadline - time.monotonic()
            if remaining <= 0:
                raise TimeoutError(f'Notification wait expired: {method} #{count}')
            try:
                message = self.queue.get(timeout=remaining)
            except queue.Empty as error:
                raise TimeoutError(f'Notification wait expired: {method} #{count}') from error
            if isinstance(message, Exception):
                raise message
            if 'method' not in message:
                raise ValueError(f'Unexpected response during notification wait: {message!r}')
            self.respond(message)

    def respond(self, value):
        self.traffic.append(value)
        if 'id' in value:
            method = value['method']
            if method == 'workspace/configuration':
                result = [{} for _ in value.get('params', {}).get('items', [])]
            elif method in ('client/registerCapability', 'client/unregisterCapability', 'workspace/diagnostic/refresh'):
                result = None
            else:
                raise RuntimeError(f'Unhandled server request: {value!r}')
            self.write({'id': value['id'], 'result': result})


def root_replacements(cwd):
    replacements = [(cwd.as_uri(), '@PROJECT_ROOT_URI@'), (str(cwd), '@PROJECT_ROOT@')]
    if os.path.exists(str(cwd).lower()) and Path(str(cwd).lower()).samefile(cwd):
        replacements.extend([(cwd.as_uri().lower(), '@PROJECT_ROOT_URI@'), (str(cwd).lower(), '@PROJECT_ROOT@')])
    return replacements


def normalize(raw, cwd):
    value = substitute(raw, root_replacements(cwd))
    value.pop('wire_frames', None)
    value.pop('received', None)  # Raw duplicates the correlated streams; retain it only in raw.json.
    for response in value['responses']:
        response['message'].pop('id', None)
    for index, message in enumerate(value.get('executed', [])):
        if 'id' in message:
            message['id'] = index
    notifications, requests, diagnostics = [], [], {}
    for message in value.pop('traffic'):
        if 'id' in message:
            message['id'] = len(requests)
            requests.append(message)
        elif message['method'] == 'textDocument/publishDiagnostics':
            uri = message['params']['uri']
            diagnostics.setdefault(uri, []).append(message)
        else:
            notifications.append(message)
    value.update(server_requests=requests, server_notifications=notifications, diagnostics=diagnostics)
    return value


def expand_positions(value, positions, encoding):
    if isinstance(value, dict):
        if '$position' in value:
            if set(value) != {'$position'}:
                raise ValueError('Position placeholder has extra keys')
            position = positions[value['$position']][encoding]
            if set(position) != {'line', 'character'} or any(type(v) is not int or v < 0 for v in position.values()):
                raise ValueError('Position must contain nonnegative integer line/character')
            return dict(position)
        return {key: expand_positions(child, positions, encoding) for key, child in value.items()}
    if isinstance(value, list):
        return [expand_positions(child, positions, encoding) for child in value]
    return value


def apply_mutations(cwd, mutations, position):
    for mutation in mutations:
        if mutation['before'] == position:
            path = cwd / mutation['path']
            if not path.resolve().is_relative_to(cwd):
                raise ValueError('Mutation path escapes fixture root')
            path.write_text(mutation['text'])


def run(session, command, output, encoding='utf-16', fixture_root=None):
    lines = [strict_json(line) for line in session.read_text().splitlines() if line.strip()]
    header, steps = lines[0], lines[1:]
    for wait in header.get('notification_waits', []):
        if (set(wait) != {'after', 'method', 'params', 'count'}
                or type(wait['after']) is not int or not 0 <= wait['after'] < len(steps)
                or not isinstance(wait['method'], str) or not wait['method']
                or not isinstance(wait['params'], dict)
                or type(wait['count']) is not int or wait['count'] < 1):
            raise ValueError('Invalid notification wait schedule')
    fixture = fixture_root or HOME / 'fixtures' / header['fixture']
    output.mkdir(parents=True, exist_ok=True)
    with tempfile.TemporaryDirectory(prefix='tsr-replay-') as directory:
        cwd = Path(directory).resolve() / 'project'
        shutil.copytree(fixture, cwd, symlinks=True)
        # Every symlink must resolve within the copied fixture: no host dependencies.
        for path in cwd.rglob('*'):
            if path.is_symlink() and not path.resolve().is_relative_to(cwd):
                raise ValueError(f'Fixture symlink escapes root: {path}')
        peer = Peer(command, cwd, output / 'stderr.log')
        raw = {'encoding': encoding, 'responses': [], 'traffic': peer.traffic, 'received': peer.received, 'executed': peer.sent, 'wire_frames': peer.wire_frames, 'waits': header.get('notification_waits', [])}
        try:
            for position, step in enumerate(steps):
                apply_mutations(cwd, header.get('mutations', []), position)
                message = substitute(step, [('@PROJECT_ROOT_URI@', cwd.as_uri()), ('@PROJECT_ROOT@', str(cwd)), ('@ENCODING@', encoding)])
                message = expand_positions(message, header.get('positions', {}), encoding)
                params = message.get('params')
                if message['kind'] == 'request':
                    if isinstance(params, dict) and '$response' in params:
                        previous = raw['responses'][params['$response']]['message']
                        result = previous.get('result')
                        if isinstance(result, dict) and 'items' in result:
                            result = result['items']
                        if not isinstance(result, list) or not result:
                            raise RuntimeError('Completion resolve requires a nonempty completion result')
                        params = result[0]
                    response = peer.exchange(message['method'], params, params_present='params' in message)
                    response = dict(response)
                    raw['responses'].append({'position': position, 'method': message['method'], 'message': response})
                elif message['kind'] == 'notification':
                    peer.write({'method': message['method'], **({'params': params} if 'params' in message else {})})
                else:
                    raise ValueError(f'Unsupported session kind: {message["kind"]}')
                for wait in header.get('notification_waits', []):
                    if wait['after'] == position:
                        peer.await_notification(wait['method'], wait['params'], wait['count'], replacements=root_replacements(cwd))
            peer.process.stdin.close()
            exited = bool(steps) and steps[-1]['kind'] == 'notification' and steps[-1]['method'] == 'exit'
            status = peer.process.wait(timeout=20)
            if status != 0 and not (exited and status == 1 and peer.stderr_text().strip() == EXIT_RACE_STDERR):
                raise RuntimeError(peer.stderr_text())
            # EOF is a reader sentinel, not server traffic. Drain all frames emitted before exit.
            peer.thread.join(timeout=2)
            if peer.thread.is_alive():
                raise RuntimeError('Reader did not finish after process exit')
            while not peer.queue.empty():
                message = peer.queue.get_nowait()
                if isinstance(message, CleanEOF):
                    continue
                if isinstance(message, Exception):
                    raise message
                if 'method' not in message:
                    raise ValueError(f'Unsolicited response after exit: {message!r}')
                if 'id' in message:
                    raise ValueError(f'Unanswered server request after exit: {message!r}')
                if exited and pinned_exit_log(message):
                    continue
                peer.respond(message)
        finally:
            try:
                (output / 'raw.json').write_text(json.dumps(raw, indent=2, ensure_ascii=False) + '\n')
            finally:
                peer.close()
        transcript = normalize(raw, cwd)
        (output / 'transcript.json').write_text(json.dumps(transcript, indent=2, ensure_ascii=False) + '\n')
        return transcript


def compare(expected, actual, output):
    if exact_equal(expected, actual):
        return True
    output.mkdir(parents=True, exist_ok=True)
    for name, value in [('expected', expected), ('actual', actual)]:
        (output / f'{name}.json').write_text(json.dumps(value, indent=2, ensure_ascii=False) + '\n')
    return False


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    sub = parser.add_subparsers(dest='action', required=True)
    sub.add_parser('list')
    for action in ('run', 'record', 'compare'):
        p = sub.add_parser(action)
        p.add_argument('--id', required=True)
        p.add_argument('--output', type=Path, required=True)
        p.add_argument('--encoding', choices=['utf-8', 'utf-16'], default='utf-16')
        if action == 'compare':
            p.add_argument('--expected', type=Path, required=True)
        p.add_argument('--command', nargs=argparse.REMAINDER, required=True, help='Must be last; executable and arguments, no shell')
    args = parser.parse_args()
    if args.action == 'list':
        for path in sorted((HOME / 'sessions').glob('*.jsonl')):
            print(path.stem)
        return
    session = HOME / 'sessions' / f'{args.id}.jsonl'
    if not session.is_file():
        parser.error('Unknown replay id')
    actual = run(session, args.command, args.output, args.encoding)
    if args.action == 'compare':
        expected = strict_json(args.expected.read_text())
        if not compare(expected, actual, args.output / 'mismatch'):
            raise SystemExit(1)


if __name__ == '__main__':
    main()
