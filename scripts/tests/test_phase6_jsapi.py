"""The jsapi suite adapter: reporter rows, file outcomes and the gate's counts."""
import json
import os
from pathlib import Path
import shutil
import subprocess
import sys
import tempfile
import unittest

ROOT = Path(__file__).resolve().parents[2]
sys.path.insert(0, str(ROOT / 'scripts'))
sys.path.insert(0, str(ROOT))
import parity  # noqa: E402
import phase6_parity  # noqa: E402
from tools.phase6.jsapi import runner  # noqa: E402

FAKE_TEST = '''
import { describe, test } from "node:test";
import assert from "node:assert";
describe("Outer", () => {
    test("passes", () => { assert.equal(1, 1); });
    describe("inner", () => {
        test("fails", () => { assert.equal(1, 2); });
        test("skipped", { skip: "not here" }, () => {});
    });
});
test("passes", () => {});
test("passes", () => {});
'''


def row(suffix='/case', state='pass', parent='jsapi/test/a.test.ts', **extra):
    return {'id': parent + suffix, 'parent': parent, 'state': state, **extra}


class Rows(unittest.TestCase):
    def test_counts_cases_not_files(self):
        native = [row(''), row('/a'), row('/b'), row('/c', state='skip')]
        rust = [row(''), row('/a'), row('/b', state='fail'), row('/c', state='skip')]
        out = phase6_parity.compare_parent(native, rust)
        counts = phase6_parity.counts(out)
        self.assertEqual(counts, dict(files=1, cases=3, executed=2, failing=1, native_skips=1, reference_failures=0))

    def test_rust_cannot_introduce_skips_and_native_failures_do_not_pass(self):
        out = phase6_parity.compare_parent([row(''), row('/a')], [row(''), row('/a', state='skip', reason='later')])
        self.assertEqual(phase6_parity.counts(out)['failing'], 1)
        out = phase6_parity.compare_parent([row(''), row('/a', state='fail')], [row(''), row('/a')])
        self.assertEqual([r['state'] for r in out if r['id'].endswith('a.test.ts')], ['fail'])

    def test_the_bridge_is_registered(self):
        self.assertIs(parity.bridge('jsapi'), phase6_parity)
        self.assertTrue(parity.SUITES['jsapi'].batch)
        self.assertIn('jsapi', parity.BATCH_SUITES)

    def test_the_suite_refuses_parallel_workers(self):
        # The pinned client's getExePath reads one repository symlink that the
        # runner points at the binary under test, so two workers would run
        # each other's binary.
        import types
        args = types.SimpleNamespace(suite='jsapi', jobs=2, runner=None, id=None, shard=None,
                                     output='unused', timeout=None)
        with self.assertRaises(SystemExit) as refused:
            parity.run(args)
        self.assertIn('one file at a time', str(refused.exception))


@unittest.skipUnless(shutil.which('node'), 'node is required')
class Reporter(unittest.TestCase):
    def test_a_fake_file_yields_one_row_per_case_and_a_file_row(self):
        with tempfile.TemporaryDirectory() as tmp:
            package = Path(tmp) / 'package'
            (package / 'test').mkdir(parents=True)
            (package / 'test/a.test.ts').write_text(FAKE_TEST)
            binary = Path(tmp) / 'server'
            binary.write_text('#!/bin/sh\nexit 0\n')
            binary.chmod(0o755)
            prepared = dict(suite='jsapi', node=shutil.which('node'), native=str(binary), rust=str(binary),
                            package=str(package), compiled_sources=['test/a.test.ts'],
                            exe_link=str(Path(tmp) / 'built/local/tsc'), goos='test', goarch='test')
            rows = runner._run_file(prepared, 'test/a.test.ts', Path(tmp), 60, str(binary))
            by_id = {r['id']: r for r in rows}
            self.assertEqual(by_id['jsapi/test/a.test.ts']['state'], 'pass')
            self.assertEqual(by_id['jsapi/test/a.test.ts/Outer > passes']['state'], 'pass')
            self.assertEqual(by_id['jsapi/test/a.test.ts/Outer > inner > fails']['state'], 'fail')
            self.assertIn('assertion', by_id['jsapi/test/a.test.ts/Outer > inner > fails']['reason'])
            self.assertEqual(by_id['jsapi/test/a.test.ts/Outer > inner > skipped']['state'], 'skip')
            self.assertEqual(by_id['jsapi/test/a.test.ts/passes']['state'], 'pass')
            self.assertEqual(by_id['jsapi/test/a.test.ts/passes #2']['state'], 'pass')
            self.assertEqual(len(rows), 6)
            self.assertTrue(all(r['parent'] == 'jsapi/test/a.test.ts' for r in rows))
            self.assertEqual(os.readlink(Path(tmp) / 'built/local/tsc'), str(binary))

    def test_a_crashing_file_fails_its_file_row(self):
        with tempfile.TemporaryDirectory() as tmp:
            package = Path(tmp) / 'package'
            (package / 'test').mkdir(parents=True)
            (package / 'test/b.test.ts').write_text('import { test } from "node:test";\ntest("starts", () => { process.exit(3); });\n')
            binary = Path(tmp) / 'server'
            binary.write_text('')
            prepared = dict(suite='jsapi', node=shutil.which('node'), native=str(binary), rust=str(binary),
                            package=str(package), compiled_sources=['test/b.test.ts'],
                            exe_link=str(Path(tmp) / 'built/local/tsc'), goos='test', goarch='test')
            rows = runner._run_file(prepared, 'test/b.test.ts', Path(tmp), 60, str(binary))
            # Node reports a dying file as one test named after the file: the
            # file row fails and no case is invented.
            self.assertEqual([(r['id'], r['state']) for r in rows], [('jsapi/test/b.test.ts', 'fail')])
            self.assertEqual(rows[0]['reason'], 'test file failed')
            (package / 'test/c.test.ts').write_text('process.exit(4);\n')
            prepared['compiled_sources'].append('test/c.test.ts')
            rows = runner._run_file(prepared, 'test/c.test.ts', Path(tmp), 60, str(binary))
            self.assertEqual([r['state'] for r in rows], ['fail'])
            self.assertIn(rows[0]['reason'], ('test file failed', 'worker exited 4 without results'))


    def test_classify_labels_the_cause(self):
        from tools.phase6.jsapi import runner
        self.assertEqual(runner.classify('Error [ERR_TEST_FAILURE]: not implemented: initialize\ncode: -32603'),
                         'server: not implemented: initialize')
        self.assertEqual(runner.classify('Error: unknown method: getServerTiming'), 'server: unknown method: getServerTiming')
        self.assertEqual(runner.classify("Error [ERR_TEST_FAILURE]: test did not finish before its parent and was cancelled"),
                         'cancelled by parent')
        self.assertEqual(runner.classify('AssertionError: expected 1 to equal 2'), 'assertion failed')
        self.assertEqual(runner.classify(None), 'assertion failed')


if __name__ == '__main__':
    unittest.main()
