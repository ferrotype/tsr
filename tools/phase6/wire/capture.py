#!/usr/bin/env python3
"""Byte goldens of the API server's two wire protocols.

`record` drives a server binary (the pin's `tsgo`, built by the Phase 5
harness as `native-lsp`) through a fixed script on each protocol and writes
every frame sent and received, with the process's exit code and stderr, to
`<output>/<protocol>.json`. `compare` replays the same script against another
binary (`tsrust`) and reports every step whose frames differ: the framing
bytes must be identical and the payloads value-identical, with the server's
current directory and the measured timings as the only placeholders.

    python3 tools/phase6/wire/capture.py record --binary target/phase6/binaries/native-lsp --output tools/phase6/wire/golden
    python3 tools/phase6/wire/capture.py compare --binary target/release/tsrust --golden tools/phase6/wire/golden

The scripts cover `initialize`, `ping`, `echo` (a binary payload on the
synchronous protocol, a JSON payload on the asynchronous one), an unknown
method, the two timing requests with collection on, and a client-side
framing error that ends the connection. The `readFile` round trips of the
callback file system join the script with A2, when a method that reads
files exists.
"""
from __future__ import annotations
import argparse
import json
import os
from pathlib import Path
import struct
import subprocess
import sys

ROOT = Path(__file__).resolve().parents[3]
CWD_PLACEHOLDER = '<cwd>'
TIMING_KEYS = ('processingTimeMs', 'totalProcessingTimeMs', 'timestamp')
# The one host-dependent response value: `initialize` reports the file
# system's case sensitivity, so a golden recorded on macOS says `false`
# where a Linux runner must say `true`. The comparison expects the running
# host's value, never the recording host's.
HOST_KEYS = ('useCaseSensitiveFileNames',)


def host_case_sensitive(directory):
    """Whether `directory`'s file system distinguishes case, probed the way
    the servers do: a file written in one case is looked up in the other."""
    import tempfile
    with tempfile.TemporaryDirectory(dir=directory) as temp:
        probe = Path(temp) / 'CaseProbe'
        probe.write_text('')
        return not (Path(temp) / 'caseprobe').exists()

# The pin's message types of protocol_msgpack.go.
REQUEST, CALL_RESPONSE, CALL_ERROR, RESPONSE, ERROR, CALL = 1, 2, 3, 4, 5, 6

ECHO_BYTES = bytes(range(256)) * 2


# --- msgpack tuples -------------------------------------------------------

def pack_str(text):
    data = text.encode()
    if len(data) < 32:
        return bytes([0xA0 | len(data)]) + data
    if len(data) < 256:
        return b'\xd9' + bytes([len(data)]) + data
    return b'\xda' + struct.pack('>H', len(data)) + data


def pack_bin(data):
    if len(data) < 256:
        return b'\xc4' + bytes([len(data)]) + data
    if len(data) < 65536:
        return b'\xc5' + struct.pack('>H', len(data)) + data
    return b'\xc6' + struct.pack('>I', len(data)) + data


def pack_tuple(message_type, method, payload):
    # The pin frames the method as binary too, not as a msgpack string.
    return b'\x93' + bytes([message_type]) + pack_bin(method.encode()) + pack_bin(payload)


def read_exact(stream, count):
    data = b''
    while len(data) < count:
        chunk = stream.read(count - len(data))
        if not chunk:
            raise EOFError(f'stream ended after {len(data)} of {count} bytes')
        data += chunk
    return data


def read_tuple(stream):
    """One tuple as (type, method, payload, raw_bytes); the raw bytes are the golden."""
    raw = bytearray(read_exact(stream, 1))
    if raw[0] != 0x93:
        raise ValueError(f'expected 0x93, got 0x{raw[0]:02x}')
    raw += read_exact(stream, 1)
    if raw[1] == 0xCC:
        raw += read_exact(stream, 1)
        message_type = raw[2]
    elif raw[1] < 0x80:
        message_type = raw[1]
    else:
        raise ValueError(f'unexpected type marker 0x{raw[1]:02x}')
    method = read_bin(stream, raw, 'method')
    payload = read_bin(stream, raw, 'payload')
    return message_type, method.decode(), payload, bytes(raw)


def read_bin(stream, raw, what):
    marker = read_exact(stream, 1)
    raw += marker
    if marker[0] == 0xC4:
        size = read_exact(stream, 1); raw += size; length = size[0]
    elif marker[0] == 0xC5:
        size = read_exact(stream, 2); raw += size; length = struct.unpack('>H', size)[0]
    elif marker[0] == 0xC6:
        size = read_exact(stream, 4); raw += size; length = struct.unpack('>I', size)[0]
    else:
        raise ValueError(f'unexpected {what} marker 0x{marker[0]:02x}')
    data = read_exact(stream, length)
    raw += data
    return data


# --- JSON-RPC frames -------------------------------------------------------

def frame_json(body):
    data = json.dumps(body, separators=(',', ':')).encode()
    return b'Content-Length: ' + str(len(data)).encode() + b'\r\n\r\n' + data


def read_frame(stream):
    """One Content-Length frame as (headers, body_bytes, raw_bytes)."""
    raw = bytearray()
    while not raw.endswith(b'\r\n\r\n'):
        raw += read_exact(stream, 1)
        if len(raw) > 4096:
            raise ValueError('header too long')
    headers = raw.decode().strip().split('\r\n')
    length = None
    for header in headers:
        name, _, value = header.partition(':')
        if name.strip().lower() == 'content-length':
            length = int(value.strip())
    if length is None:
        raise ValueError('no Content-Length header')
    body = read_exact(stream, length)
    raw += body
    return headers, body, bytes(raw)


# --- the scripts -----------------------------------------------------------

def sync_script():
    """(name, bytes to send, expect a reply) for the msgpack protocol."""
    return [
        ('initialize', pack_tuple(REQUEST, 'initialize', b''), True),
        ('ping', pack_tuple(REQUEST, 'ping', b''), True),
        ('echo-binary', pack_tuple(REQUEST, 'echo', ECHO_BYTES), True),
        ('unknown-method', pack_tuple(REQUEST, 'noSuchMethod', b'{}'), True),
        ('getServerTiming', pack_tuple(REQUEST, 'getServerTiming', b''), True),
        ('resetServerTiming', pack_tuple(REQUEST, 'resetServerTiming', b''), True),
        ('getServerTiming-after-reset', pack_tuple(REQUEST, 'getServerTiming', b''), True),
        # A two-element array where the protocol wants three: the server
        # reports the marker it saw and ends the connection.
        ('client-framing-error', b'\x92' + bytes([REQUEST]) + pack_bin(b'ping') + pack_bin(b''), False),
    ]


def async_script():
    return [
        ('initialize', frame_json({'jsonrpc': '2.0', 'id': 1, 'method': 'initialize', 'params': None}), True),
        ('ping', frame_json({'jsonrpc': '2.0', 'id': 2, 'method': 'ping', 'params': None}), True),
        ('echo-json', frame_json({'jsonrpc': '2.0', 'id': 3, 'method': 'echo', 'params': {'a': [1, 2.5, 'x', None, True], 'b': {'c': 'd'}}}), True),
        ('unknown-method', frame_json({'jsonrpc': '2.0', 'id': 4, 'method': 'noSuchMethod', 'params': {}}), True),
        ('getServerTiming', frame_json({'jsonrpc': '2.0', 'id': 5, 'method': 'getServerTiming', 'params': None}), True),
        ('resetServerTiming', frame_json({'jsonrpc': '2.0', 'id': 6, 'method': 'resetServerTiming', 'params': None}), True),
        ('getServerTiming-after-reset', frame_json({'jsonrpc': '2.0', 'id': 7, 'method': 'getServerTiming', 'params': None}), True),
        ('client-framing-error', b'Content-Length: 5\r\n\r\n{"jso', False),
    ]


def run_script(binary, protocol, cwd, timeout=30):
    args = [str(binary), '--api', '--cwd', str(cwd), '--timing']
    if protocol == 'async':
        args.append('--async')
    process = subprocess.Popen(args, stdin=subprocess.PIPE, stdout=subprocess.PIPE, stderr=subprocess.PIPE)
    steps = []
    reader = read_tuple if protocol == 'sync' else read_frame
    try:
        for name, data, expect_reply in (sync_script() if protocol == 'sync' else async_script()):
            step = {'name': name, 'sent': data.hex()}
            try:
                process.stdin.write(data)
                process.stdin.flush()
            except BrokenPipeError:
                step['error'] = 'server closed its input'
                steps.append(step)
                break
            if expect_reply:
                try:
                    received = reader(process.stdout)
                    step['received'] = received[-1].hex()
                except (EOFError, ValueError) as error:
                    step['received'] = ''
                    step['error'] = str(error)
            steps.append(step)
        try:
            process.stdin.close()
        except BrokenPipeError:
            pass
        try:
            process.wait(timeout=timeout)
        except subprocess.TimeoutExpired:
            process.kill()
            process.wait()
            steps.append({'name': 'exit', 'error': 'server did not exit after the framing error'})
    finally:
        if process.poll() is None:
            process.kill()
            process.wait()
    trailing = process.stdout.read()
    stderr = process.stderr.read().decode(errors='replace')
    return {'protocol': protocol, 'cwd': str(cwd), 'steps': steps, 'exit': process.returncode,
            'stderr': stderr, 'trailing': trailing.hex(), 'case_sensitive': host_case_sensitive(cwd)}


# --- comparison ------------------------------------------------------------

def normalize_value(value, cwd, host=None):
    """`host` is the running host's case sensitivity, substituted into a
    golden's host-dependent values; `None` keeps the value as observed."""
    if isinstance(value, dict):
        return {key: (0 if key in TIMING_KEYS else host if key in HOST_KEYS and host is not None
                      else normalize_value(item, cwd, host)) for key, item in value.items()}
    if isinstance(value, list):
        return [normalize_value(item, cwd, host) for item in value]
    if isinstance(value, str) and cwd and value == cwd:
        return CWD_PLACEHOLDER
    return value


def payload_value(payload, cwd, host=None):
    """The JSON value of a payload, or the raw bytes when it is not JSON."""
    try:
        return normalize_value(json.loads(payload.decode()), cwd, host)
    except (UnicodeDecodeError, ValueError):
        return payload.hex()


class Bytes:
    def __init__(self, data):
        self.data = data
        self.at = 0

    def read(self, count):
        chunk = self.data[self.at:self.at + count]
        self.at += len(chunk)
        return chunk


def describe_sync(raw, cwd, host=None):
    message_type, method, payload, _ = read_tuple(Bytes(raw))
    framing = raw[:len(raw) - len(payload)]
    # The payload marker's size bytes depend on the payload length, which may
    # differ for value-identical JSON; compare the marker kind only.
    marker_at = len(framing) - framing_marker_offset(framing)
    return {'type': message_type, 'method': method,
            'framing': framing[:marker_at].hex() + f' bin{framing[marker_at]:02x}',
            'payload': payload_value(payload, cwd, host)}


def framing_marker_offset(framing):
    for marker, size in ((0xC4, 1), (0xC5, 2), (0xC6, 4)):
        at = len(framing) - 1 - size
        if at >= 0 and framing[at] == marker:
            return 1 + size
    raise ValueError('no payload marker in framing')


def describe_async(raw, cwd, host=None):
    headers, body, _ = read_frame(Bytes(raw))
    names = [header.partition(':')[0].strip() for header in headers]
    return {'headers': names, 'body': payload_value(body, cwd, host)}


def describe(step, protocol, cwd, host=None):
    if 'received' not in step:
        return {'error': step.get('error')}
    raw = bytes.fromhex(step['received'])
    if not raw:
        return {'received': '', 'error': step.get('error')}
    try:
        return describe_sync(raw, cwd, host) if protocol == 'sync' else describe_async(raw, cwd, host)
    except (EOFError, ValueError) as error:
        return {'undecodable': raw.hex(), 'error': str(error)}


def compare(golden, actual):
    differences = []
    by_name = {step['name']: step for step in golden['steps']}
    for step in actual['steps']:
        expected = by_name.get(step['name'])
        if expected is None:
            differences.append((step['name'], 'no golden step', None, step))
            continue
        # The golden's host-dependent values are read as this host's.
        want = describe(expected, golden['protocol'], golden['cwd'], actual.get('case_sensitive'))
        got = describe(step, actual['protocol'], actual['cwd'])
        if want != got:
            differences.append((step['name'], 'frames differ', want, got))
    for key in ('exit',):
        if golden.get(key) != actual.get(key):
            differences.append((key, 'differs', golden.get(key), actual.get(key)))
    golden_stderr = golden['stderr'].replace(golden['cwd'], CWD_PLACEHOLDER).strip()
    actual_stderr = actual['stderr'].replace(actual['cwd'], CWD_PLACEHOLDER).strip()
    if golden_stderr != actual_stderr:
        differences.append(('stderr', 'differs', golden_stderr, actual_stderr))
    return differences


def main():
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    commands = parser.add_subparsers(dest='command', required=True)
    for name in ('record', 'compare'):
        command = commands.add_parser(name)
        command.add_argument('--binary', required=True)
        command.add_argument('--cwd', default=str(ROOT))
        command.add_argument('--protocol', choices=('sync', 'async'), action='append')
        if name == 'record':
            command.add_argument('--output', required=True, type=Path)
        else:
            command.add_argument('--golden', required=True, type=Path)
    args = parser.parse_args()
    protocols = args.protocol or ['sync', 'async']
    failed = False
    for protocol in protocols:
        observed = run_script(args.binary, protocol, args.cwd)
        if args.command == 'record':
            args.output.mkdir(parents=True, exist_ok=True)
            (args.output / f'{protocol}.json').write_text(json.dumps(observed, indent=1) + '\n')
            print(f'{protocol}: {len(observed["steps"])} steps recorded, exit {observed["exit"]}')
            continue
        golden = json.loads((args.golden / f'{protocol}.json').read_text())
        differences = compare(golden, observed)
        print(f'{protocol}: {len(golden["steps"])} steps, {len(differences)} differences')
        for name, what, want, got in differences:
            failed = True
            print(f'  {name}: {what}\n    golden: {json.dumps(want)}\n    actual: {json.dumps(got)}')
    sys.exit(1 if failed else 0)


if __name__ == '__main__':
    main()
