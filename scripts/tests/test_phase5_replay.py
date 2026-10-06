import importlib.util
import json
from pathlib import Path
import tempfile
import unittest
ROOT = Path(__file__).resolve().parents[2]
spec = importlib.util.spec_from_file_location('replay', ROOT / 'tools/phase5/replay/replay.py')
replay = importlib.util.module_from_spec(spec)
spec.loader.exec_module(replay)


class ReplayTests(unittest.TestCase):
    def test_exact_comparison_witnesses(self):
        with tempfile.TemporaryDirectory() as directory:
            output = Path(directory)
            for expected, actual in [({'result': [1, 2]}, {'result': [2, 1]}), ({}, {'result': None}), ({'error': {'code': 1}}, {'error': {'code': 2}})]:
                self.assertFalse(replay.compare(expected, actual, output))
                self.assertEqual(json.loads((output / 'actual.json').read_text()), actual)

    def test_diagnostic_streams_preserve_document_order(self):
        def diagnostic(uri, number):
            return {'method': 'textDocument/publishDiagnostics', 'params': {'uri': uri, 'diagnostics': [number]}}
        def normalized(traffic):
            return replay.normalize({'responses': [], 'traffic': traffic}, Path('/tmp/project'))
        a, b, c = diagnostic('a', 1), diagnostic('b', 2), diagnostic('a', 3)
        self.assertEqual(normalized([a, b, c]), normalized([b, a, c]))
        self.assertNotEqual(normalized([a, b, c]), normalized([c, b, a]))

    def test_server_ids_only_and_root_placeholders(self):
        raw = {'responses': [{'message': {'result': '/tmp/project/a'}}], 'traffic': [{'id': 'ts2', 'method': 'request', 'params': {'id': 'keep'}}]}
        actual = replay.normalize(raw, Path('/tmp/project'))
        self.assertEqual(actual['server_requests'][0]['id'], 0)
        self.assertEqual(actual['server_requests'][0]['params']['id'], 'keep')
        self.assertEqual(actual['responses'][0]['message']['result'], '@PROJECT_ROOT@/a')

    def test_fixture_is_self_contained(self):
        for fixture in (replay.HOME / 'fixtures').iterdir():
            self.assertGreaterEqual(sum(p.is_file() for p in fixture.rglob('*')), 20)
            self.assertLessEqual(sum(p.is_file() for p in fixture.rglob('*')), 80)
            for path in fixture.rglob('*'):
                if path.is_symlink():
                    self.assertTrue(path.resolve().is_relative_to(fixture.resolve()))

    def test_real_transport_retains_error_null_and_server_traffic(self):
        import sys
        with tempfile.TemporaryDirectory() as directory:
            base = Path(directory)
            fixture = base / 'fixture'
            fixture.mkdir()
            (fixture / 'a.ts').write_text('const a = 1;')
            server = base / 'server.py'
            server.write_text('''import json, sys
while True:
    headers = {}
    while True:
        line = sys.stdin.buffer.readline()
        if not line:
            sys.exit(0)
        if line == b"\\r\\n":
            break
        key, value = line.decode().split(":", 1)
        headers[key.lower()] = value.strip()
    message = json.loads(sys.stdin.buffer.read(int(headers["content-length"])))
    def emit(value):
        body = json.dumps({"jsonrpc":"2.0", **value}).encode()
        sys.stdout.buffer.write(f"Content-Length: {len(body)}\\r\\n\\r\\n".encode() + body)
        sys.stdout.buffer.flush()
    if message.get("method") == "exit":
        sys.exit(0)
    if "id" in message:
        emit({"method":"example/notification", "params":{"ordered":[2,1]}})
        emit({"id":message["id"], **({"error":{"code":-32601,"message":"missing","data":None}} if message["method"] == "unknown" else {"result":None})})
''')
            session = base / 'session.jsonl'
            session.write_text('\n'.join(json.dumps(row) for row in [{'fixture': 'unused'}, {'kind': 'request', 'method': 'unknown'}, {'kind': 'request', 'method': 'shutdown', 'params': None}, {'kind': 'notification', 'method': 'exit'}]))
            actual = replay.run(session, [sys.executable, str(server)], base / 'output', fixture_root=fixture)
            self.assertEqual(actual['responses'][0]['message']['error']['data'], None)
            self.assertIn('result', actual['responses'][1]['message'])
            self.assertIsNone(actual['responses'][1]['message']['result'])
            self.assertEqual(actual['server_notifications'][0]['params']['ordered'], [2, 1])
            self.assertTrue((base / 'output/raw.json').is_file())

    def test_mutations_are_scheduled_and_confined(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory).resolve()
            target = root / 'tsconfig.json'
            target.write_text('old')
            mutation = {'before': 3, 'path': 'tsconfig.json', 'text': 'new'}
            replay.apply_mutations(root, [mutation], 2)
            self.assertEqual(target.read_text(), 'old')
            replay.apply_mutations(root, [mutation], 3)
            self.assertEqual(target.read_text(), 'new')
            with self.assertRaises(ValueError):
                replay.apply_mutations(root, [{'before': 0, 'path': '../escape', 'text': 'bad'}], 0)

    def test_json_types_and_invalid_values(self):
        self.assertFalse(replay.exact_equal({'x': True}, {'x': 1}))
        self.assertFalse(replay.exact_equal([False], [0]))
        self.assertTrue(replay.exact_equal({'x': 1}, {'x': 1.0}))
        for text in ('{"a":1,"a":2}', '{"x":NaN}', '{"x":Infinity}', '{"x":1e999}'):
            with self.assertRaises(ValueError):
                replay.strict_json(text)
        for message in ({'jsonrpc': '2.0', 'id': True, 'result': None}, {'jsonrpc': '2.0', 'id': 1, 'result': None, 'error': {}}, {'jsonrpc': '2.0', 'id': 1}, {'jsonrpc': '2.0', 'id': 1, 'error': {'code': True, 'message': 'bad'}}):
            with self.assertRaises(ValueError):
                replay.validate_message(message)

    def test_reader_distinguishes_clean_and_partial_eof(self):
        import io
        import queue
        for stream, expected in [(b'', replay.CleanEOF), (b'Content-Length: 4\r\n\r\n{}', EOFError), (b'Content-Length: 1\r\n\r\n{', ValueError), (b'Content-Length: 3\r\n', EOFError)]:
            frames = queue.Queue()
            replay.reader(io.BytesIO(stream), frames, [])
            error = frames.get_nowait()
            self.assertIsInstance(error, expected)
            if stream:
                self.assertNotIsInstance(error, replay.CleanEOF)

    def test_raw_ids_and_executed_inventory_are_retained(self):
        cwd = Path('/tmp/project')
        raw = {'responses': [{'position': 0, 'method': 'hover', 'message': {'jsonrpc': '2.0', 'id': 7, 'result': None}}], 'traffic': [], 'received': [{'jsonrpc': '2.0', 'id': 7, 'result': None}], 'executed': [{'jsonrpc': '2.0', 'id': 7, 'method': 'hover', 'params': {'textDocument': {'uri': 'file:///tmp/project/a.ts'}}}]}
        normalized = replay.normalize(raw, cwd)
        self.assertEqual(raw['responses'][0]['message']['id'], 7)
        self.assertNotIn('id', normalized['responses'][0]['message'])
        self.assertEqual(normalized['executed'][0]['params']['textDocument']['uri'], '@PROJECT_ROOT_URI@/a.ts')
        changed = json.loads(json.dumps(normalized))
        changed['executed'][0]['params']['textDocument']['uri'] = '@PROJECT_ROOT_URI@/b.ts'
        self.assertFalse(replay.exact_equal(normalized, changed))

    def test_capture_rejects_trailing_partial_frame(self):
        import sys
        with tempfile.TemporaryDirectory() as directory:
            base = Path(directory)
            fixture = base / 'fixture'
            fixture.mkdir()
            server = base / 'server.py'
            server.write_text('import sys\nsys.stdin.buffer.read()\nsys.stdout.buffer.write(b"Content-Length: 5\\r\\n\\r\\n{}")\nsys.stdout.buffer.flush()\n')
            session = base / 'session.jsonl'
            session.write_text('{"fixture":"unused"}\n{"kind":"notification","method":"exit"}\n')
            with self.assertRaisesRegex(EOFError, 'Partial frame body'):
                replay.run(session, [sys.executable, str(server)], base / 'output', fixture_root=fixture)
            raw = json.loads((base / 'output/raw.json').read_text())
            self.assertEqual(raw['wire_frames'][0]['body_hex'], b'{}'.hex())
            self.assertEqual(raw['executed'][0]['method'], 'exit')

    def test_reference_dependencies_are_reached(self):
        fixture = replay.HOME / 'fixtures/references'
        config = replay.strict_json((fixture / 'core/tsconfig.json').read_text())
        index = (fixture / 'core/index.ts').read_text()
        self.assertFalse((fixture / 'src').exists())
        for number in range(20):
            self.assertIn(f'value{number}.ts', config['files'])
            self.assertIn(f'export * from "./value{number}.js";', index)

    def test_generated_positions_distinguish_utf8_utf16(self):
        for session in (replay.HOME / 'sessions').glob('*.jsonl'):
            rows = [replay.strict_json(line) for line in session.read_text().splitlines()]
            header = rows[0]
            text = next(row['params']['textDocument']['text'] for row in rows[1:] if row['method'] == 'textDocument/didOpen')
            line = text.splitlines()[2]
            prefix = line[:line.index('answer') + 2]
            expected = {'utf-8': len(prefix.encode('utf-8')), 'utf-16': len(prefix.encode('utf-16-le')) // 2}
            self.assertNotEqual(expected['utf-8'], expected['utf-16'])
            hover = next(row for row in rows[1:] if row['method'] == 'textDocument/hover')
            for encoding, character in expected.items():
                expanded = replay.expand_positions(hover, header['positions'], encoding)
                self.assertEqual(expanded['params']['position'], {'line': 2, 'character': character})
