"""S10 acceptance safeguards without running a compiler or long benchmark."""
import contextlib
import copy
import io
import subprocess
import tempfile
import tomllib
import unittest
from pathlib import Path
from unittest.mock import patch

import s10
import s10_corpus as corpus
import s10_measure as measure
from s06_ownership import validate_output


class S10Evidence(unittest.TestCase):
    def test_all_capture_inputs_are_covered_by_consuming_ledgers(self):
        captured = set(corpus.sources())
        runs = tomllib.loads((corpus.ROOT / 'status/runs.toml').read_text())
        for producer in ['e7', 'e8']:
            paths = subprocess.check_output(['git', 'ls-files', '--cached', '--others', '--exclude-standard', '-z', '--',
                    *(':(top,glob)' + p for p in runs[producer]['sources'])], cwd=corpus.ROOT)
            covered = set(paths.decode().split('\0')) | set(runs[producer]['inputs'])
            self.assertFalse(captured - covered, sorted(captured - covered))
        for required in ['scripts/s10_measure.py', 'tools/s10/wasm/imports.mjs',
                         'tools/s10/go-parser/main.go', 'tools/s10/rust-consumer/Cargo.lock',
                         'scripts/s07_benchmark_stats.py', 'status/experiments.toml']:
            self.assertIn(required, captured)

    @staticmethod
    def timing():
        return {'samples': [{'index': i, 'order': ['rust', 'go'] if i % 2 == 0 else ['go', 'rust'],
                             'elapsed_ns': {'go': 1000, 'rust': 400}} for i in range(7)]}

    def test_missing_reordered_or_zero_timing_never_passes(self):
        raw = self.timing()
        self.assertTrue(measure.summarize(raw, 'parser', 7)['stable'])
        for runtime in ['go', 'rust']:
            for invalid in [0, -1, True, float('nan')]:
                changed = copy.deepcopy(raw)
                changed['samples'][0]['elapsed_ns'][runtime] = invalid
                with self.assertRaises(ValueError):
                    measure.summarize(changed, 'parser', 7)
        changed = copy.deepcopy(raw)
        changed['samples'].pop()
        with self.assertRaises(ValueError):
            measure.summarize(changed, 'parser', 7)
        changed = copy.deepcopy(raw)
        changed['samples'][0]['order'].reverse()
        with self.assertRaises(ValueError):
            measure.summarize(changed, 'parser', 7)

    def test_stale_source_rejected_before_raw_data_or_process_access(self):
        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory)
            corpus.p4.write_new(path / 'capture.json', {'sources': {'old': 'hash'}})
            corpus.p4.write_new(path / 'completed.json', {'source_stable': True})
            with patch.object(measure, 'sources', return_value={'new': 'hash'}), self.assertRaisesRegex(ValueError, 'stale'):
                measure.verify(path)

    def test_missing_captures_are_pending_without_launching_work(self):
        with tempfile.TemporaryDirectory() as directory, patch.object(s10, 'ROOT', Path(directory)), \
                patch.object(subprocess, 'run', side_effect=AssertionError('must not run')), \
                contextlib.redirect_stderr(io.StringIO()):
            self.assertEqual(s10.producer('e7'), {'metrics': {}})
            self.assertEqual(s10.producer('e8'), {'metrics': {}})

    def test_ownership_requires_every_named_case_no_ignores(self):
        cases = ['lifetime::first', 'lifetime::second']
        output = ('running 2 tests\ntest lifetime::first ... ok\ntest lifetime::second ... ok\n'
                  'test result: ok. 2 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.1s\n')
        with contextlib.redirect_stderr(io.StringIO()):
            validate_output(output.encode(), cases, 'miri')
            for bad in [output.replace('second ... ok', 'second ... ignored'),
                        output.replace('lifetime::second', 'lifetime::first')]:
                with self.assertRaises(ValueError):
                    validate_output(bad.encode(), cases, 'miri')


if __name__ == '__main__':
    unittest.main()
