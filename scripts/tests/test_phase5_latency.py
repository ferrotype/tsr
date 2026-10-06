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
