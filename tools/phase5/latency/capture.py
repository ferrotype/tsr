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
COMPARISON_RULE = 'completion-client-order-v1'


def normalize_completion_order(transcript):
    """Apply client completion ordering only to the normalized latency transcript."""
    # Pin: upstream/tsc/internal/ls/completions.go:3829-3840 leaves non-tie
    # ordering to editors; internal/fourslash/fourslash.go:1343 uses a stable
    # client sort. Exact (sortText or label, label) keys suffice for this ASCII
    # scenario. Python's stable sort retains wire order for equal keys; every
    # item field and every other response/notification array stays unchanged.
    def key(item):
        if not isinstance(item, dict) or not isinstance(item.get('label'), str):
            raise ValueError('Completion ordering requires an item with a string label')
        if 'sortText' in item and not isinstance(item['sortText'], str):
            raise ValueError('Completion ordering requires string sortText when present')
        return (item.get('sortText') or item['label'], item['label'])
    for response in transcript['responses']:
        if response['method'] != 'textDocument/completion':
            continue
        result = response['message'].get('result')
        items = result.get('items') if isinstance(result, dict) else result
        if isinstance(items, list):
            items.sort(key=key)
    return transcript


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
            timestamp, message = self.next_message(deadline)
            if isinstance(message, Exception):
                raise message
            if 'method' not in message:
                if type(message['id']) is not int or message['id'] != self.id:
                    raise ValueError(f'Unsolicited response: {message!r}')
                self.response_time = timestamp
                return message
            self.respond(message)

    def next_message(self, deadline):
        remaining = deadline - time.monotonic()
        if remaining <= 0:
            raise TimeoutError('Response/diagnostics deadline expired')
        try:
            return self.queue.get(timeout=remaining)
        except queue.Empty as error:
            raise TimeoutError('Response/diagnostics deadline expired') from error

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
            timestamp, message = self.next_message(deadline)
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


def diagnostic_mode(scenario):
    specification = scenario.get('first_diagnostics', {'mode': 'push'})
    if not isinstance(specification, dict) or specification.get('mode') not in ('push', 'pull'):
        raise ValueError('First diagnostics requires an explicit push/pull mode')
    if specification['mode'] == 'push' and set(specification) != {'mode'}:
        raise ValueError('Push diagnostics takes only mode')
    if specification['mode'] == 'pull' and (set(specification) != {'mode', 'params'} or not isinstance(specification['params'], dict)):
        raise ValueError('Pull diagnostics requires fixed request params')
    return specification['mode']


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
    mode = diagnostic_mode(scenario)
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
    substitutions = [('@PROJECT_ROOT_URI@', fixture.as_uri()), ('@PROJECT_ROOT@', str(fixture))]
    expanded = replay.substitute(scenario, substitutions)
    document_params = [expanded['edit'], expanded['warmup_hover'], *expanded['requests'].values()]
    if mode == 'pull':
        params = expanded['first_diagnostics']['params']
        document_params.append(params)
        if any(key in params for key in ('previousResultId', 'partialResultToken', 'workDoneToken')):
            raise ValueError('First diagnostic pull requires a full response without result ids or progress tokens')
    if any(params.get('textDocument', {}).get('uri') != (fixture / opened['path']).as_uri() for params in document_params):
        raise ValueError('Scenario edit and requests must target the opened document')
    if replay.exact_equal(scenario['warmup_hover'].get('position'), scenario['requests']['hover'].get('position')):
        raise ValueError('Warmup hover position must differ from measured hover')
    changes = scenario['edit'].get('contentChanges', [])
    if len(changes) != 1 or not isinstance(changes[0].get('text'), str) or len(changes[0]['text']) != 1 or 'range' not in changes[0]:
        raise ValueError('Completion warmup requires one fixed one-character ranged edit')
    edit_version = scenario['edit'].get('textDocument', {}).get('version')
    if type(edit_version) is not int or edit_version != opened['version'] + 1:
        raise ValueError('Edit version must advance the open version by one')


def run_runtime(command, cwd, scenario, output):
    output.mkdir(parents=True, exist_ok=True)
    peer = Peer(command, cwd, output / 'stderr.log')
    raw = {'encoding': scenario['encoding'], 'responses': [], 'traffic': peer.traffic,
           'executed': peer.sent, 'received': peer.received, 'wire_frames': peer.wire_frames}
    measurements = {}
    substitutions = [('@PROJECT_ROOT_URI@', cwd.as_uri()), ('@PROJECT_ROOT@', str(cwd))]
    expanded = replay.substitute(scenario, substitutions)
    def request(name, method, params, measured=False, *, params_present=True):
        if measured:
            response, elapsed = peer.measured_request(method, params)
            measurements[name] = elapsed
        else:
            response = peer.exchange(method, params, params_present=params_present)
            if 'error' in response:
                raise RuntimeError(f'Warmup request failed: {response}')
        raw['responses'].append({'position': len(raw['responses']), 'method': method, 'name': name, 'message': response})
        return response
    try:
        initialize = expanded.get('initialize', {'processId': None, 'rootUri': cwd.as_uri(), 'initializationOptions': {'logVerbosity': 5, 'userPreferences': {'disableAutomaticTypeAcquisition': True}}, 'capabilities': {'general': {'positionEncodings': [scenario['encoding']]}}})
        preferences = initialize.get('initializationOptions', {}).get('userPreferences', {})
        if preferences.get('disableAutomaticTypeAcquisition') is not True and preferences.get('automaticTypeAcquisitionEnabled') is not False:
            raise ValueError('Initialize must explicitly disable automatic type acquisition')
        if initialize.get('capabilities', {}).get('workspace', {}).get('configuration'):
            raise ValueError('Workspace configuration callbacks can override ATA settings; omit this capability')
        if initialize.get('capabilities', {}).get('window', {}).get('workDoneProgress'):
            raise ValueError('Latency scenario must not advertise timer-dependent workDoneProgress')
        if initialize.get('capabilities', {}).get('general', {}).get('positionEncodings') != [scenario['encoding']]:
            raise ValueError('Initialize must advertise exactly the scenario position encoding')
        initialized = request('initialize', 'initialize', initialize)
        selected_encoding = initialized.get('result', {}).get('capabilities', {}).get('positionEncoding', 'utf-16')
        if selected_encoding != scenario['encoding']:
            raise ValueError('Server selected a different position encoding')
        peer.write({'method': 'initialized', 'params': {}})
        opened = expanded['open']
        uri = (cwd / opened['path']).as_uri()
        text = opened.get('text', (cwd / opened['path']).read_text())
        peer.write({'method': 'textDocument/didOpen', 'params': {'textDocument': {'uri': uri, 'languageId': opened['languageId'], 'version': opened['version'], 'text': text}}})
        open_start = peer.write_times[-1]
        if diagnostic_mode(expanded) == 'pull':
            request('first_diagnostics', 'textDocument/diagnostic', expanded['first_diagnostics']['params'])
            diagnostic = raw['responses'][-1]['message'].get('result')
            if not isinstance(diagnostic, dict) or diagnostic.get('kind') != 'full' or not isinstance(diagnostic.get('items'), list):
                raise ValueError('First diagnostic pull requires a full report with items array')
            elapsed = peer.response_time - open_start
        else:
            diagnostic, elapsed = peer.first_diagnostics(uri, opened['version'], open_start)
        measurements['first_diagnostics'] = elapsed
        # Push belongs to the complete traffic stream; pull is retained in responses.
        request('warmup_hover', 'textDocument/hover', expanded['warmup_hover'])
        peer.write({'method': 'textDocument/didChange', 'params': expanded['edit']})
        for name in METRICS[1:]:
            request(name, METHODS[name], expanded['requests'][name], measured=True)
        request('shutdown', 'shutdown', None, params_present=False)
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
        return measurements, normalize_completion_order(replay.normalize(raw, cwd))
    finally:
        try:
            (output / 'raw.json').write_text(json.dumps(raw, indent=2, ensure_ascii=False) + '\n')
        finally:
            peer.close()


def validate_original_evidence(source, encoding):
    for index in range(20):
        transcripts = {}
        for runtime in ('go', 'rust'):
            directory = source / f'pair-{index:02d}' / runtime
            if any(not (directory / artifact).is_file() for artifact in ('raw.json', 'transcript.json', 'stderr.log')):
                raise ValueError('Extension requires all original pair artifacts')
            raw = replay.strict_json((directory / 'raw.json').read_text())
            if not isinstance(raw, dict) or raw.get('encoding') != encoding:
                raise ValueError('Original raw evidence has the wrong encoding')
            for message in [*raw['received'], *raw['executed']]:
                replay.validate_message(message)
            bodies = [bytes.fromhex(frame['body_hex']) for frame in raw['wire_frames']]
            if any(frame['headers'].get('content-length') != str(len(body)) for frame, body in zip(raw['wire_frames'], bodies)):
                raise ValueError('Original raw evidence has an invalid frame length')
            if not replay.exact_equal([replay.strict_json(body) for body in bodies], raw['received']):
                raise ValueError('Original raw frames disagree with received messages')
            transcripts[runtime] = replay.strict_json((directory / 'transcript.json').read_text())
            if not isinstance(transcripts[runtime], dict) or transcripts[runtime].get('encoding') != encoding:
                raise ValueError('Original transcript has the wrong encoding')
        if not replay.exact_equal(transcripts['go'], transcripts['rust']):
            raise ValueError('Original pair transcripts no longer match')


def capture(fixture, scenario, commands, pairs, output, smoke=False, extend=None):
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
    if output.resolve().is_relative_to(fixture):
        raise ValueError('Capture output must be outside the fixture')
    if output.exists() and any(output.iterdir()):
        raise ValueError('Capture output must be empty; evidence is never overwritten')
    output.mkdir(parents=True, exist_ok=True)
    report = {'format': 1, 'smoke': smoke, 'correctness_matched': True, 'pairs': [],
              'samples': {runtime: {name: [] for name in METRICS} for runtime in ('go', 'rust')},
              'host': {'os': platform.system().lower(), 'architecture': platform.machine(), 'cpu_capacity': os.cpu_count(), 'hostname': platform.node()},
              'revision': subprocess.check_output(['git', 'rev-parse', 'HEAD'], cwd=ROOT, text=True).strip(),
              'recorded_at': datetime.now(timezone.utc).isoformat(),
              'metadata': {'fixture_sha256': identity(fixture), 'scenario': scenario, 'scenario_sha256': hashlib.sha256(json.dumps(scenario, sort_keys=True).encode()).hexdigest(), 'first_diagnostics_mode': diagnostic_mode(scenario), 'comparison_rules': [COMPARISON_RULE], 'commands': commands, 'binaries': binaries}}
    start_index = 0
    try:
        if extend is not None:
            if smoke or pairs != 40:
                raise ValueError('Extension requires a nonsmoke 40-pair target')
            source = extend / 'samples.json' if extend.is_dir() else extend
            read_capture(source)
            previous = replay.strict_json(source.read_text())
            if len(previous['pairs']) != 20:
                raise ValueError('Only one complete 20-pair capture can be extended')
            if any(previous.get(key) != report[key] for key in ('host', 'revision', 'metadata')):
                raise ValueError('Extension requires identical host, revision, fixture, scenario and commands/binaries')
            # Validate all previous raw evidence before copying or launching a new process.
            validate_original_evidence(source.parent, scenario['encoding'])
            for index in range(20):
                shutil.copytree(source.parent / f'pair-{index:02d}', output / f'pair-{index:02d}')
            report['pairs'], report['samples'] = previous['pairs'], previous['samples']
            report['metadata']['extension'] = {'source_samples_sha256': hashlib.sha256(source.read_bytes()).hexdigest(),
                                               'source_recorded_at': previous['recorded_at'], 'added_pairs': 20}
            start_index = 20
        for index in range(start_index, pairs):
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
    parser.add_argument('--extend', type=Path, help='Preserve a matched 20-pair capture and add 20 pairs; requires --pairs 40')
    parser.add_argument('--output', type=Path, required=True)
    args = parser.parse_args()
    commands = {runtime: replay.strict_json(getattr(args, runtime + '_command')) for runtime in ('go', 'rust')}
    if any(not isinstance(command, list) or not command or any(not isinstance(arg, str) for arg in command) for command in commands.values()):
        parser.error('Commands must be nonempty JSON string arrays')
    result = capture(args.fixture, replay.strict_json(args.scenario.read_text()), commands, args.pairs, args.output, args.smoke, args.extend)
    if not result['correctness_matched']:
        raise SystemExit(1)


if __name__ == '__main__':
    main()
