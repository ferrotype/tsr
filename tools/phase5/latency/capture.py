#!/usr/bin/env python3
"""Capture five LSP latencies from supplied hermetic fixtures; never build/install."""
import argparse
from datetime import datetime, timezone
import hashlib
import importlib.util
import json
import os
from pathlib import Path
import platform
import queue
import shutil
import subprocess
import tempfile
import threading
import time

ROOT = Path(__file__).resolve().parents[3]
spec = importlib.util.spec_from_file_location('phase5_replay', ROOT / 'tools/phase5/replay/replay.py')
replay = importlib.util.module_from_spec(spec)
spec.loader.exec_module(replay)
METRICS = ('first_diagnostics', 'completion', 'hover', 'references', 'rename')
METHODS = {name: 'textDocument/' + name for name in METRICS[1:]}


class TimedStream:
    def __init__(self, stream):
        self.stream = stream
        self.frame_complete = None

    def readline(self):
        return self.stream.readline()

    def read(self, length):
        body = self.stream.read(length)
        self.frame_complete = time.perf_counter_ns()
        return body


class TimedQueue(queue.Queue):
    def __init__(self, stream):
        super().__init__()
        self.stream = stream

    def put(self, item, *args, **kwargs):
        # Reader thread: body read completion excludes decoder/consumer scheduling.
        timestamp = time.perf_counter_ns() if isinstance(item, Exception) else self.stream.frame_complete
        return super().put((timestamp, item), *args, **kwargs)


class Peer(replay.Peer):
    def __init__(self, command, cwd, stderr_path):
        self.stderr_path = stderr_path
        self.stderr_file = stderr_path.open('wb')
        try:
            self.process = subprocess.Popen(command, cwd=cwd, stdin=subprocess.PIPE, stdout=subprocess.PIPE, stderr=self.stderr_file)
        except BaseException:
            self.stderr_file.close()
            raise
        timed_stream = TimedStream(self.process.stdout)
        self.queue = TimedQueue(timed_stream)
        self.id = 0
        self.traffic, self.received, self.sent, self.wire_frames = [], [], [], []
        self.thread = threading.Thread(target=replay.reader, args=(timed_stream, self.queue, self.received, self.wire_frames), daemon=True)
        self.thread.start()
        self.write_times = []

    def close(self):
        try:
            replay.interop.Peer.close(self)
        finally:
            for stream in (self.process.stdin, self.process.stdout, self.stderr_file):
                stream.close()

    def write(self, message):
        full = {'jsonrpc': '2.0', **message}
        replay.validate_message(full)
        body = json.dumps(full, ensure_ascii=False).encode()
        frame = f'Content-Length: {len(body)}\r\n\r\n'.encode() + body
        self.sent.append(full)
        self.write_times.append(time.perf_counter_ns())
        self.process.stdin.write(frame)
        self.process.stdin.flush()

    def await_response(self):
        deadline = time.monotonic() + 20
        while True:
            timestamp, message = self.queue.get(timeout=max(0, deadline - time.monotonic()))
            if isinstance(message, Exception):
                raise message
            if 'method' not in message:
                if type(message['id']) is not int or message['id'] != self.id:
                    raise ValueError(f'Unsolicited response: {message!r}')
                self.response_time = timestamp
                return message
            self.respond(message)

    def measured_request(self, method, params):
        response = self.exchange(method, params)
        # Replies to server requests may write later: retain initial request write timestamp.
        start = self.request_start
        if 'error' in response:
            raise RuntimeError(f'Measured request failed: {response}')
        return response, self.response_time - start

    def exchange(self, method, params=None, *, params_present=True):
        self.id += 1
        self.write({'id': self.id, 'method': method, **({'params': params} if params_present else {})})
        self.request_start = self.write_times[-1]
        return self.await_response()

    def first_diagnostics(self, uri, version, start):
        deadline = time.monotonic() + 20
        while True:
            timestamp, message = self.queue.get(timeout=max(0, deadline - time.monotonic()))
            if isinstance(message, Exception):
                raise message
            if 'method' not in message:
                raise ValueError('Unexpected response while awaiting first diagnostics')
            self.respond(message)
            if message['method'] == 'textDocument/publishDiagnostics':
                params = message.get('params', {})
                if timestamp >= start and params.get('uri') == uri and type(params.get('version')) is int and params['version'] == version:
                    if not isinstance(params.get('diagnostics'), list):
                        raise ValueError('Complete diagnostic publication requires diagnostics array')
                    return message, timestamp - start


def identity(fixture):
    digest = hashlib.sha256()
    for path in sorted(fixture.rglob('*')):
        if path.is_symlink():
            if Path(os.readlink(path)).is_absolute():
                raise ValueError(f'Fixture absolute symlink cannot be relocated: {path}')
            if not path.resolve().is_relative_to(fixture.resolve()):
                raise ValueError(f'Fixture symlink escapes root: {path}')
            content = os.readlink(path).encode()
        elif path.is_file():
            content = path.read_bytes()
        else:
            continue
        digest.update(str(path.relative_to(fixture)).encode() + b'\0' + content + b'\0')
    return digest.hexdigest()


def validate_scenario(scenario, fixture):
    if scenario.get('encoding') not in ('utf-8', 'utf-16'):
        raise ValueError('Scenario requires an explicit encoding')
    if set(scenario.get('requests', {})) != set(METRICS[1:]):
        raise ValueError('Scenario requires exactly completion/hover/references/rename params')
    opened = scenario['open']
    path = fixture / opened['path']
    if not path.resolve().is_relative_to(fixture.resolve()) or not path.is_file():
        raise ValueError('Open path must be a fixture file')
    if type(opened.get('version')) is not int or opened['version'] < 1:
        raise ValueError('Open version must be a positive integer')
    if not isinstance(scenario.get('warmup_hover'), dict) or not isinstance(scenario.get('edit'), dict):
        raise ValueError('Scenario requires warmup_hover and fixed edit params')
    if replay.exact_equal(scenario['warmup_hover'], scenario['requests']['hover']):
        raise ValueError('Warmup hover must be elsewhere than the measured hover')
    changes = scenario['edit'].get('contentChanges', [])
    if len(changes) != 1 or not isinstance(changes[0].get('text'), str) or len(changes[0]['text']) != 1 or 'range' not in changes[0]:
        raise ValueError('Completion warmup requires one fixed one-character ranged edit')
    if scenario['edit'].get('textDocument', {}).get('version') != opened['version'] + 1:
        raise ValueError('Edit version must advance the open version by one')


def run_runtime(command, cwd, scenario, output):
    output.mkdir(parents=True, exist_ok=True)
    peer = Peer(command, cwd, output / 'stderr.log')
    raw = {'encoding': scenario['encoding'], 'responses': [], 'traffic': peer.traffic,
           'executed': peer.sent, 'received': peer.received, 'wire_frames': peer.wire_frames}
    measurements = {}
    substitutions = [('@PROJECT_ROOT_URI@', cwd.as_uri()), ('@PROJECT_ROOT@', str(cwd))]
    expanded = replay.substitute(scenario, substitutions)
    def request(name, method, params, measured=False):
        if measured:
            response, elapsed = peer.measured_request(method, params)
            measurements[name] = elapsed
        else:
            response = peer.exchange(method, params)
            if 'error' in response:
                raise RuntimeError(f'Warmup request failed: {response}')
        raw['responses'].append({'position': len(raw['responses']), 'method': method, 'name': name, 'message': response})
    try:
        initialize = expanded.get('initialize', {'processId': None, 'rootUri': cwd.as_uri(), 'initializationOptions': {'logVerbosity': 5, 'userPreferences': {'disableAutomaticTypeAcquisition': True}}, 'capabilities': {'general': {'positionEncodings': [scenario['encoding']]}}})
        preferences = initialize.get('initializationOptions', {}).get('userPreferences', {})
        if preferences.get('disableAutomaticTypeAcquisition') is not True and preferences.get('automaticTypeAcquisitionEnabled') is not False:
            raise ValueError('Initialize must explicitly disable automatic type acquisition')
        if initialize.get('capabilities', {}).get('workspace', {}).get('configuration'):
            raise ValueError('Workspace configuration callbacks can override ATA settings; omit this capability')
        if initialize.get('capabilities', {}).get('window', {}).get('workDoneProgress'):
            raise ValueError('Latency scenario must not advertise timer-dependent workDoneProgress')
        request('initialize', 'initialize', initialize)
        peer.write({'method': 'initialized', 'params': {}})
        opened = expanded['open']
        uri = (cwd / opened['path']).as_uri()
        text = opened.get('text', (cwd / opened['path']).read_text())
        peer.write({'method': 'textDocument/didOpen', 'params': {'textDocument': {'uri': uri, 'languageId': opened['languageId'], 'version': opened['version'], 'text': text}}})
        diagnostic, elapsed = peer.first_diagnostics(uri, opened['version'], peer.write_times[-1])
        measurements['first_diagnostics'] = elapsed
        # First diagnostic already belongs to the complete per-document traffic stream.
        request('warmup_hover', 'textDocument/hover', expanded['warmup_hover'])
        peer.write({'method': 'textDocument/didChange', 'params': expanded['edit']})
        for name in METRICS[1:]:
            request(name, METHODS[name], expanded['requests'][name], measured=True)
        request('shutdown', 'shutdown', None)
        peer.write({'method': 'exit'})
        peer.process.stdin.close()
        if peer.process.wait(timeout=20) != 0:
            raise RuntimeError(peer.stderr_path.read_text(errors='replace'))
        peer.thread.join(timeout=2)
        if peer.thread.is_alive():
            raise RuntimeError('Reader did not finish')
        while not peer.queue.empty():
            _, message = peer.queue.get_nowait()
            if isinstance(message, replay.CleanEOF):
                continue
            if isinstance(message, Exception):
                raise message
            if 'method' not in message or 'id' in message:
                raise ValueError('Unexpected pending response/request after exit')
            peer.respond(message)
        return measurements, replay.normalize(raw, cwd)
    finally:
        try:
            (output / 'raw.json').write_text(json.dumps(raw, indent=2, ensure_ascii=False) + '\n')
        finally:
            peer.close()


def capture(fixture, scenario, commands, pairs, output, smoke=False):
    if pairs not in ((1, 3) if smoke else (20, 40)):
        raise ValueError('Smoke captures require 1/3 pairs; records require 20/40')
    if set(commands) != {'go', 'rust'} or any(not isinstance(command, list) or not command or any(not isinstance(arg, str) for arg in command) for command in commands.values()):
        raise ValueError('Commands require exactly two nonempty argument arrays')
    binaries = {}
    for runtime, command in commands.items():
        executable = Path(command[0]) if Path(command[0]).is_absolute() else Path(shutil.which(command[0]) or '')
        if not executable.is_file():
            raise ValueError('Commands require existing absolute executable paths or PATH executables')
        binaries[runtime] = {'path': str(executable.resolve()), 'sha256': hashlib.sha256(executable.read_bytes()).hexdigest()}
    fixture = fixture.resolve()
    validate_scenario(scenario, fixture)
    output.mkdir(parents=True, exist_ok=True)
    report = {'format': 1, 'smoke': smoke, 'correctness_matched': True, 'pairs': [],
              'samples': {runtime: {name: [] for name in METRICS} for runtime in ('go', 'rust')},
              'host': {'os': platform.system().lower(), 'architecture': platform.machine(), 'cpu_capacity': os.cpu_count()},
              'revision': subprocess.check_output(['git', 'rev-parse', 'HEAD'], cwd=ROOT, text=True).strip(),
              'recorded_at': datetime.now(timezone.utc).isoformat(),
              'metadata': {'fixture_sha256': identity(fixture), 'scenario': scenario, 'scenario_sha256': hashlib.sha256(json.dumps(scenario, sort_keys=True).encode()).hexdigest(), 'commands': commands, 'binaries': binaries}}
    try:
        for index in range(pairs):
            order = ['go', 'rust'] if index % 2 == 0 else ['rust', 'go']
            pair = {'index': index, 'order': order, 'matched': False}
            report['pairs'].append(pair)
            values, transcripts = {}, {}
            with tempfile.TemporaryDirectory(prefix='tsr-lsp-latency-') as directory:
                cwd = Path(directory).resolve() / 'project'
                # Restore original bytes between runtimes, preserving the same absolute root.
                for runtime in order:
                    if cwd.exists():
                        shutil.rmtree(cwd)
                    shutil.copytree(fixture, cwd, symlinks=True)
                    artifact = output / f'pair-{index:02d}' / runtime
                    values[runtime], transcripts[runtime] = run_runtime(commands[runtime], cwd, scenario, artifact)
                    (artifact / 'transcript.json').write_text(json.dumps(transcripts[runtime], indent=2, ensure_ascii=False) + '\n')
                pair['measurements_ns'] = values
                pair['matched'] = replay.compare(transcripts['go'], transcripts['rust'], output / f'pair-{index:02d}' / 'mismatch')
            if not pair['matched']:
                report['correctness_matched'] = False
                # Pair remains visible, but neither runtime contributes latency samples.
                continue
            for runtime in order:
                for name in METRICS:
                    report['samples'][runtime][name].append(values[runtime][name])
    except Exception as error:
        report['correctness_matched'] = False
        report['failure'] = str(error)
        raise
    finally:
        (output / 'samples.json').write_text(json.dumps(report, indent=2, ensure_ascii=False) + '\n')
    return report


def read_capture(path):
    if path.is_dir():
        path = path / 'samples.json'
    report = replay.strict_json(path.read_text())
    pairs = report.get('pairs', [])
    if type(report.get('format')) is not int or report.get('format') != 1 or report.get('smoke') is not False or report.get('correctness_matched') is not True or len(pairs) not in (20, 40):
        raise ValueError('Performance record requires a complete matched nonsmoke 20/40-pair capture')
    for index, pair in enumerate(pairs):
        expected_order = ['go', 'rust'] if index % 2 == 0 else ['rust', 'go']
        if type(pair.get('index')) is not int or pair.get('index') != index or pair.get('order') != expected_order or pair.get('matched') is not True:
            raise ValueError('Invalid or unmatched pair')
    if set(report.get('samples', {})) != {'go', 'rust'}:
        raise ValueError('Expected both runtime sample sets')
    for samples in report['samples'].values():
        if set(samples) != set(METRICS) or any(len(values) != len(pairs) or any(type(v) is not int or v <= 0 for v in values) for values in samples.values()):
            raise ValueError('Invalid metric sample count/type/value')
    for index, pair in enumerate(pairs):
        for runtime in ('go', 'rust'):
            for name in METRICS:
                if type(pair.get('measurements_ns', {}).get(runtime, {}).get(name)) is not int or pair['measurements_ns'][runtime][name] != report['samples'][runtime][name][index]:
                    raise ValueError('Pair evidence disagrees with sample arrays')
    report['metadata']['pairs'] = pairs
    report['metadata']['format'] = report['format']
    return {key: report[key] for key in ('samples', 'host', 'revision', 'recorded_at', 'correctness_matched', 'smoke', 'metadata')}


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--fixture', type=Path, required=True)
    parser.add_argument('--scenario', type=Path, required=True)
    parser.add_argument('--go-command', required=True, help='JSON array of executable/arguments')
    parser.add_argument('--rust-command', required=True, help='JSON array of executable/arguments')
    parser.add_argument('--pairs', type=int, required=True)
    parser.add_argument('--smoke', action='store_true')
    parser.add_argument('--output', type=Path, required=True)
    args = parser.parse_args()
    commands = {runtime: replay.strict_json(getattr(args, runtime + '_command')) for runtime in ('go', 'rust')}
    if any(not isinstance(command, list) or not command or any(not isinstance(arg, str) for arg in command) for command in commands.values()):
        parser.error('Commands must be nonempty JSON string arrays')
    result = capture(args.fixture, replay.strict_json(args.scenario.read_text()), commands, args.pairs, args.output, args.smoke)
    if not result['correctness_matched']:
        raise SystemExit(1)


if __name__ == '__main__':
    main()
