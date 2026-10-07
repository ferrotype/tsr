import importlib.util
import json
from pathlib import Path
import sys
import tempfile
import unittest
from unittest import mock

ROOT = Path(__file__).resolve().parents[2]
spec = importlib.util.spec_from_file_location('latency', ROOT / 'tools/phase5/latency/capture.py')
latency = importlib.util.module_from_spec(spec)
spec.loader.exec_module(latency)
prepare_spec = importlib.util.spec_from_file_location('latency_prepare', ROOT / 'tools/phase5/latency/prepare.py')
prepare = importlib.util.module_from_spec(prepare_spec)
prepare_spec.loader.exec_module(prepare)

SERVER = '''import json, sys
while True:
    headers = {}
    while True:
        line = sys.stdin.buffer.readline()
        if not line: sys.exit(0)
        if line == b"\\r\\n": break
        key, value = line.decode().split(":", 1)
        headers[key.lower()] = value.strip()
    message = json.loads(sys.stdin.buffer.read(int(headers["content-length"])))
    def emit(value):
        body = json.dumps({"jsonrpc":"2.0", **value}).encode()
        sys.stdout.buffer.write(f"Content-Length: {len(body)}\\r\\n\\r\\n".encode() + body)
        sys.stdout.buffer.flush()
    if message.get("method") == "exit": sys.exit(0)
    if message.get("method") == "textDocument/didOpen":
        doc = message["params"]["textDocument"]
        for uri, version in [(doc["uri"] + ".other", doc["version"]), (doc["uri"], doc["version"] - 1), (doc["uri"], doc["version"])]:
            emit({"method":"textDocument/publishDiagnostics","params":{"uri":uri,"version":version,"diagnostics":[]}})
    if "id" in message:
        result = {"method":message["method"]}
        if message["method"] == "initialize": result["capabilities"] = {"positionEncoding": message["params"]["capabilities"]["general"]["positionEncodings"][0]}
        if message["method"] == "textDocument/diagnostic": result = {"kind":"full", "items":[]}
        if len(sys.argv) > 1 and message["method"] == "textDocument/rename": result["wrong"] = True
        emit({"id":message["id"],"result":result})
'''


def scenario():
    uri = '@PROJECT_ROOT_URI@/main.ts'
    def query(character):
        return {'textDocument': {'uri': uri}, 'position': {'line': 0, 'character': character}}
    requests = {name: query(2) for name in latency.METRICS[1:]}
    requests['references']['context'] = {'includeDeclaration': True}
    requests['rename']['newName'] = 'renamed'
    return {'encoding': 'utf-16', 'open': {'path': 'main.ts', 'languageId': 'typescript', 'version': 1},
            'warmup_hover': query(1), 'edit': {'textDocument': {'uri': uri, 'version': 2},
            'contentChanges': [{'range': {'start': {'line': 0, 'character': 0}, 'end': {'line': 0, 'character': 0}}, 'text': ' '}]}, 'requests': requests}


class LatencyTests(unittest.TestCase):
    def test_offline_dependencies_require_lock_integrity_and_safe_members(self):
        import base64
        import hashlib
        import io
        import tarfile
        def archive(name, payload):
            data = io.BytesIO()
            with tarfile.open(fileobj=data, mode='w:gz') as bundle:
                entry = tarfile.TarInfo(name)
                entry.size = len(payload)
                bundle.addfile(entry, io.BytesIO(payload))
            return data.getvalue()
        with tempfile.TemporaryDirectory() as directory:
            base = Path(directory)
            path = base / 'node.tgz'
            content = archive('node/package.json', b'{"name":"@types/node","version":"22.20.1"}')
            path.write_bytes(content)
            locked = {'version': '22.20.1', 'resolved': 'pinned-test',
                      'integrity': 'sha512-' + base64.b64encode(hashlib.sha512(content).digest()).decode()}
            result = prepare.unpack_locked(path, base / 'valid', '@types/node', locked)
            self.assertEqual(result['version'], '22.20.1')
            path.write_bytes(content + b'changed')
            with self.assertRaisesRegex(ValueError, 'lock integrity'):
                prepare.unpack_locked(path, base / 'bad', '@types/node', locked)
            content = archive('node/../../escape', b'unsafe')
            path.write_bytes(content)
            locked['integrity'] = 'sha512-' + base64.b64encode(hashlib.sha512(content).digest()).decode()
            with self.assertRaisesRegex(ValueError, 'unsafe path'):
                prepare.unpack_locked(path, base / 'unsafe', '@types/node', locked)
            self.assertFalse((base / 'escape').exists())

    def test_proposed_scenario_positions_match_the_pinned_source(self):
        data = json.loads((ROOT / 'tools/phase5/latency/proposals/typescript-pull.json').read_text())
        fixture = ROOT / 'upstream/packages/typescript'
        latency.validate_scenario(data, fixture)
        lines = (fixture / data['open']['path']).read_text().splitlines()
        position = data['requests']['completion']['position']
        self.assertTrue(lines[position['line']][:position['character']].endswith('SyntaxKind.'))
        for name in ('hover', 'references', 'rename'):
            position = data['requests'][name]['position']
            self.assertEqual(lines[position['line']][position['character'] - 1:position['character'] + 3], 'Node')

    def prepare(self, directory):
        base = Path(directory)
        fixture = base / 'fixture'
        fixture.mkdir()
        (fixture / 'main.ts').write_text('const x = 1;')
        server = base / 'server.py'
        server.write_text(SERVER)
        return base, fixture, [sys.executable, str(server)]

    def test_three_smoke_pairs_are_fresh_alternating_and_matched(self):
        with tempfile.TemporaryDirectory() as directory:
            base, fixture, command = self.prepare(directory)
            report = latency.capture(fixture, scenario(), {'go': command, 'rust': command}, 3, base / 'capture', smoke=True)
            self.assertTrue(report['correctness_matched'])
            self.assertEqual([pair['order'] for pair in report['pairs']], [['go', 'rust'], ['rust', 'go'], ['go', 'rust']])
            for runtime in ('go', 'rust'):
                self.assertEqual(set(report['samples'][runtime]), set(latency.METRICS))
                self.assertTrue(all(len(values) == 3 and all(value > 0 for value in values) for values in report['samples'][runtime].values()))
            with self.assertRaises(ValueError):
                latency.read_capture(base / 'capture/samples.json')
            raw = json.loads((base / 'capture/pair-00/go/raw.json').read_text())
            self.assertEqual(len([m for m in raw['received'] if m.get('method') == 'textDocument/publishDiagnostics']), 3)

    def test_mismatch_excludes_both_runtime_samples_and_retains_evidence(self):
        with tempfile.TemporaryDirectory() as directory:
            base, fixture, command = self.prepare(directory)
            report = latency.capture(fixture, scenario(), {'go': command, 'rust': [*command, 'different']}, 1, base / 'capture', smoke=True)
            self.assertFalse(report['correctness_matched'])
            self.assertFalse(report['pairs'][0]['matched'])
            self.assertEqual(report['samples']['go']['rename'], [])
            self.assertEqual(report['samples']['rust']['rename'], [])
            self.assertTrue((base / 'capture/pair-00/mismatch/actual.json').exists())

    def test_explicit_pull_mode_retains_full_report_and_didopen_clock(self):
        with tempfile.TemporaryDirectory() as directory:
            base, fixture, command = self.prepare(directory)
            data = scenario()
            data['first_diagnostics'] = {'mode': 'pull', 'params': {'textDocument': {'uri': '@PROJECT_ROOT_URI@/main.ts'}}}
            timestamps = {}
            original_write, original_exchange = latency.Peer.write, latency.Peer.exchange
            def write(peer, message):
                original_write(peer, message)
                if message.get('method') == 'textDocument/didOpen':
                    timestamps['open'] = peer.write_times[-1]
            def exchange(peer, method, *args, **kwargs):
                response = original_exchange(peer, method, *args, **kwargs)
                if method == 'textDocument/diagnostic':
                    timestamps.update(response=peer.response_time, request=peer.request_start)
                return response
            with mock.patch.object(latency.Peer, 'write', write), mock.patch.object(latency.Peer, 'exchange', exchange):
                report = latency.capture(fixture, data, {'go': command, 'rust': command}, 1, base / 'capture', smoke=True)
            self.assertTrue(report['correctness_matched'])
            self.assertEqual(report['metadata']['first_diagnostics_mode'], 'pull')
            raw = json.loads((base / 'capture/pair-00/go/raw.json').read_text())
            pull = next(row for row in raw['responses'] if row['name'] == 'first_diagnostics')
            self.assertEqual(pull['message']['result'], {'kind': 'full', 'items': []})
            # Start at didOpen, not the subsequent diagnostic request's write.
            self.assertEqual(report['samples']['rust']['first_diagnostics'][0], timestamps['response'] - timestamps['open'])
            self.assertLess(timestamps['open'], timestamps['request'])

    def test_pull_rejects_unchanged_or_incomplete_reports(self):
        for result in ({'kind': 'unchanged', 'resultId': 'x'}, {'kind': 'full'}, None):
            with self.subTest(result=result), tempfile.TemporaryDirectory() as directory:
                base, fixture, command = self.prepare(directory)
                Path(command[1]).write_text(SERVER.replace('result = {"kind":"full", "items":[]}', 'result = json.loads(' + repr(json.dumps(result)) + ')'))
                data = scenario()
                data['first_diagnostics'] = {'mode': 'pull', 'params': {'textDocument': {'uri': '@PROJECT_ROOT_URI@/main.ts'}}}
                with self.assertRaisesRegex(ValueError, 'full report'):
                    latency.run_runtime(command, fixture, data, base / 'runtime')
                self.assertTrue((base / 'runtime/raw.json').exists())

    def test_pull_error_is_retained_and_fails_capture(self):
        with tempfile.TemporaryDirectory() as directory:
            base, fixture, command = self.prepare(directory)
            Path(command[1]).write_text(SERVER.replace('emit({"id":message["id"],"result":result})',
                'emit({"id":message["id"], **({"error":{"code":-32601,"message":"missing diagnostic"}} if message["method"] == "textDocument/diagnostic" else {"result":result})})'))
            data = scenario()
            data['first_diagnostics'] = {'mode': 'pull', 'params': {'textDocument': {'uri': '@PROJECT_ROOT_URI@/main.ts'}}}
            with self.assertRaisesRegex(RuntimeError, 'missing diagnostic'):
                latency.capture(fixture, data, {'go': command, 'rust': command}, 1, base / 'capture', smoke=True)
            report = json.loads((base / 'capture/samples.json').read_text())
            self.assertFalse(report['correctness_matched'])
            self.assertEqual(report['samples']['go']['first_diagnostics'], [])
            raw = json.loads((base / 'capture/pair-00/go/raw.json').read_text())
            self.assertTrue(any(message.get('error', {}).get('code') == -32601 for message in raw['received']))

    def test_capture_rejects_reusing_evidence_or_output_inside_fixture(self):
        with tempfile.TemporaryDirectory() as directory:
            base, fixture, command = self.prepare(directory)
            output = base / 'capture'
            output.mkdir()
            (output / 'samples.json').write_text('previous evidence')
            for location in (output, fixture / 'capture'):
                with self.assertRaises(ValueError), mock.patch.object(latency, 'run_runtime') as run:
                    latency.capture(fixture, scenario(), {'go': command, 'rust': command}, 1, location, smoke=True)
                run.assert_not_called()
            self.assertEqual((output / 'samples.json').read_text(), 'previous evidence')

    def test_extension_preserves_first_twenty_pairs_and_requires_same_scenario(self):
        with tempfile.TemporaryDirectory() as directory:
            base, fixture, command = self.prepare(directory)
            commands = {'go': command, 'rust': command}
            first = latency.capture(fixture, scenario(), commands, 20, base / 'first')
            extended = latency.capture(fixture, scenario(), commands, 40, base / 'extended', extend=base / 'first')
            self.assertEqual(extended['pairs'][:20], first['pairs'])
            for runtime in ('go', 'rust'):
                for metric in latency.METRICS:
                    self.assertEqual(extended['samples'][runtime][metric][:20], first['samples'][runtime][metric])
                    self.assertEqual(len(extended['samples'][runtime][metric]), 40)
            original = base / 'first/pair-00/go/raw.json'
            self.assertEqual((base / 'extended/pair-00/go/raw.json').read_bytes(), original.read_bytes())
            self.assertEqual(len(latency.read_capture(base / 'extended')['metadata']['pairs']), 40)
            changed = scenario()
            changed['requests']['rename']['newName'] = 'another'
            with self.assertRaisesRegex(ValueError, 'identical host'), mock.patch.object(latency, 'run_runtime') as run:
                latency.capture(fixture, changed, commands, 40, base / 'wrong', extend=base / 'first')
            run.assert_not_called()
            with mock.patch.object(latency.platform, 'node', return_value='another-host'), self.assertRaisesRegex(ValueError, 'identical host'), mock.patch.object(latency, 'run_runtime') as run:
                latency.capture(fixture, scenario(), commands, 40, base / 'host-drift', extend=base / 'first')
            run.assert_not_called()
            artifact = base / 'first/pair-00/rust/transcript.json'
            original = artifact.read_text()
            modified = json.loads(original)
            modified['responses'][0]['message']['result']['different'] = True
            artifact.write_text(json.dumps(modified))
            with self.assertRaisesRegex(ValueError, 'no longer match'), mock.patch.object(latency, 'run_runtime') as run:
                latency.capture(fixture, scenario(), commands, 40, base / 'corrupt', extend=base / 'first')
            run.assert_not_called()
            artifact.write_text(original)
            with mock.patch.object(latency.shutil, 'copytree', side_effect=OSError('evidence copy failure')), self.assertRaisesRegex(OSError, 'evidence copy failure'), mock.patch.object(latency, 'run_runtime') as run:
                latency.capture(fixture, scenario(), commands, 40, base / 'copy-error', extend=base / 'first')
            run.assert_not_called()
            for location in ('wrong', 'host-drift', 'corrupt', 'copy-error'):
                failed = json.loads((base / location / 'samples.json').read_text())
                self.assertFalse(failed['correctness_matched'])
                self.assertIn('failure', failed)
                with self.assertRaises(ValueError):
                    latency.read_capture(base / location)

    def test_scenario_rejects_other_document_same_hover_position_and_partial_pull(self):
        with tempfile.TemporaryDirectory() as directory:
            _, fixture, _ = self.prepare(directory)
            for mutation in ('document', 'warmup', 'partial'):
                data = scenario()
                if mutation == 'document':
                    data['edit']['textDocument']['uri'] += '.other'
                elif mutation == 'warmup':
                    data['warmup_hover'] = {**data['requests']['hover'], 'extra': True}
                else:
                    data['first_diagnostics'] = {'mode': 'pull', 'params': {'textDocument': {'uri': '@PROJECT_ROOT_URI@/main.ts'}, 'partialResultToken': 'partial'}}
                with self.subTest(mutation=mutation), self.assertRaises(ValueError):
                    latency.validate_scenario(data, fixture)

    def test_initialize_encoding_must_match_scenario_and_server(self):
        with tempfile.TemporaryDirectory() as directory:
            base, fixture, command = self.prepare(directory)
            data = scenario()
            data['encoding'] = 'utf-8'
            Path(command[1]).write_text(SERVER.replace('message["params"]["capabilities"]["general"]["positionEncodings"][0]', '"utf-16"'))
            with self.assertRaisesRegex(ValueError, 'selected a different'):
                latency.run_runtime(command, fixture, data, base / 'runtime')

    def test_notifications_do_not_extend_response_or_push_deadline(self):
        import queue
        for response in (True, False):
            peer = object.__new__(latency.Peer)
            peer.queue, peer.traffic, peer.id = queue.Queue(), [], 1
            message = {'jsonrpc': '2.0', 'method': 'example/notification'}
            peer.queue.put((100, message))
            peer.queue.put((200, message))
            with self.subTest(response=response), mock.patch.object(latency.time, 'monotonic', side_effect=[0, 0, 21]), self.assertRaises(TimeoutError):
                peer.await_response() if response else peer.first_diagnostics('target', 1, 0)
            self.assertEqual(len(peer.traffic), 1)

    def test_fixture_links_are_relocatable_and_stay_inside_root(self):
        with tempfile.TemporaryDirectory() as directory:
            base, fixture, _ = self.prepare(directory)
            link = fixture / 'alias.ts'
            link.symlink_to(fixture / 'main.ts')
            with self.assertRaisesRegex(ValueError, 'absolute symlink'):
                latency.identity(fixture)
            link.unlink()
            link.symlink_to('main.ts')
            self.assertEqual(len(latency.identity(fixture)), 64)
            link.unlink()
            link.symlink_to('../server.py')
            with self.assertRaisesRegex(ValueError, 'escapes root'):
                latency.identity(fixture)

    def test_stderr_above_pipe_capacity_is_saved_without_blocking(self):
        with tempfile.TemporaryDirectory() as directory:
            base, fixture, command = self.prepare(directory)
            Path(command[1]).write_text("import sys; sys.stderr.write('x' * 1048576); sys.stderr.flush()\n" + SERVER)
            latency.run_runtime(command, fixture, scenario(), base / 'runtime')
            self.assertEqual((base / 'runtime/stderr.log').stat().st_size, 1048576)

    def test_raw_write_failure_still_retires_live_peer(self):
        with tempfile.TemporaryDirectory() as directory:
            base, fixture, command = self.prepare(directory)
            peers = []
            original = latency.Peer
            def create(*args):
                peer = original(*args)
                peers.append(peer)
                return peer
            with mock.patch.object(latency, 'Peer', side_effect=create), mock.patch.object(original, 'exchange', side_effect=RuntimeError('request failure')), mock.patch.object(Path, 'write_text', side_effect=OSError('raw write failure')):
                with self.assertRaisesRegex(OSError, 'raw write failure'):
                    latency.run_runtime(command, fixture, scenario(), base / 'runtime')
            self.assertIsNotNone(peers[0].process.poll())
            self.assertTrue(peers[0].stderr_file.closed)
            self.assertFalse(peers[0].thread.is_alive())

    def test_scenario_requires_single_character_versioned_edit(self):
        with tempfile.TemporaryDirectory() as directory:
            _, fixture, _ = self.prepare(directory)
            data = scenario()
            data['edit']['contentChanges'][0]['text'] = 'two'
            with self.assertRaises(ValueError):
                latency.validate_scenario(data, fixture)

    def test_reader_rejects_missing_pairs_wrong_count_and_boolean_samples(self):
        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory) / 'samples.json'
            report = {'format': 1, 'smoke': False, 'correctness_matched': True, 'pairs': [], 'samples': {}}
            path.write_text(json.dumps(report))
            with self.assertRaises(ValueError):
                latency.read_capture(path)
            report['pairs'] = [{'index': i, 'order': ['go', 'rust'] if i % 2 == 0 else ['rust', 'go'], 'matched': True} for i in range(20)]
            report['samples'] = {runtime: {name: [True] * 20 for name in latency.METRICS} for runtime in ('go', 'rust')}
            path.write_text(json.dumps(report))
            with self.assertRaises(ValueError):
                latency.read_capture(path)

    def test_first_diagnostics_uses_matching_uri_and_version_timestamp(self):
        import queue
        peer = object.__new__(latency.Peer)
        peer.queue = queue.Queue()
        peer.traffic = []
        for timestamp, uri, version in [(110, 'other', 1), (120, 'target', 0), (130, 'target', True), (500, 'target', 1)]:
            peer.queue.put((timestamp, {'jsonrpc': '2.0', 'method': 'textDocument/publishDiagnostics', 'params': {'uri': uri, 'version': version, 'diagnostics': []}}))
        publication, duration = peer.first_diagnostics('target', 1, 100)
        self.assertEqual(duration, 400)
        self.assertEqual(publication['params']['version'], 1)
        self.assertEqual(len(peer.traffic), 4)

    def test_reader_accepts_complete_record_and_detects_pair_sample_drift(self):
        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory) / 'samples.json'
            samples = {runtime: {name: [100] * 20 for name in latency.METRICS} for runtime in ('go', 'rust')}
            pairs = [{'index': i, 'order': ['go', 'rust'] if i % 2 == 0 else ['rust', 'go'], 'matched': True, 'measurements_ns': {runtime: {name: 100 for name in latency.METRICS} for runtime in ('go', 'rust')}} for i in range(20)]
            report = {'format': 1, 'smoke': False, 'correctness_matched': True, 'pairs': pairs, 'samples': samples, 'host': {}, 'revision': 'unit-test', 'recorded_at': 'unit-test', 'metadata': {}}
            path.write_text(json.dumps(report))
            actual = latency.read_capture(path)
            self.assertEqual(len(actual['metadata']['pairs']), 20)
            sys.path.insert(0, str(ROOT / 'scripts'))
            try:
                perf_spec = importlib.util.spec_from_file_location('phase5_perf', ROOT / 'scripts/perf.py')
                perf = importlib.util.module_from_spec(perf_spec)
                sys.modules[perf_spec.name] = perf
                perf_spec.loader.exec_module(perf)
                result = perf.read_lsp(path.parent, None)
                self.assertEqual(set(result['ratios']), set(latency.METRICS))
                self.assertTrue(all(value == 1.0 for value in result['ratios'].values()))
                self.assertTrue(all(summary['inconclusive'] and summary['needs_more'] for summary in result['summaries'].values()))
            finally:
                sys.path.remove(str(ROOT / 'scripts'))
            report['samples']['rust']['rename'][0] = 200
            path.write_text(json.dumps(report))
            with self.assertRaisesRegex(ValueError, 'Pair evidence'):
                latency.read_capture(path)
