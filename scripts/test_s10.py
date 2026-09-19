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
            self.assertEqual(set(runs[producer]['sources']),
                             (set(corpus.source_patterns()) - {'upstream/package.json'})
                             | {'xtask/**', 'upstream'})
            paths = subprocess.check_output(['git', 'ls-files', '--cached', '--others', '--exclude-standard', '-z', '--',
                    *(':(top,glob)' + p for p in runs[producer]['sources'])], cwd=corpus.ROOT)
            covered = set(paths.decode().split('\0')) | set(runs[producer]['inputs'])
            self.assertFalse(captured - covered, sorted(captured - covered))
        for required in ['scripts/s10_measure.py', 'tools/s10/wasm/imports.mjs',
                         'tools/s10/go-parser/main.go', 'tools/s10/rust-consumer/Cargo.lock',
                         'scripts/s07_benchmark_stats.py', 'status/experiments.toml']:
            self.assertIn(required, captured)

    def test_unrelated_edits_do_not_stale_capture_or_ledgers(self):
        import fnmatch
        patterns = corpus.source_patterns()
        runs = tomllib.loads((corpus.ROOT / 'status/runs.toml').read_text())
        for name in ['scripts/s09_format.py', 'tools/s08/p7/child.rs',
                     'crates/ts_api/src/lib.rs', 'data/s10/initial-acceptance.json']:
            self.assertFalse(any(fnmatch.fnmatchcase(name, p) for p in patterns), name)
            for producer in ['e7', 'e8']:
                self.assertFalse(any(fnmatch.fnmatchcase(name, p) for p in runs[producer]['sources']), name)
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            (root / 'tools/s10').mkdir(parents=True)
            (root / 'tools/s10/sources.json').write_bytes((corpus.ROOT / 'tools/s10/sources.json').read_bytes())
            (root / 'scripts').mkdir()
            dependent = root / 'scripts/s10_measure.py'
            unrelated = root / 'scripts/s09_format.py'
            dependent.write_text('original'); unrelated.write_text('original')
            with patch.object(corpus, 'ROOT', root):
                before = corpus.sources()
                unrelated.write_text('changed')
                self.assertEqual(before, corpus.sources())
                dependent.write_text('changed')
                self.assertNotEqual(before, corpus.sources())

    def test_capture_manifest_includes_transitive_local_dependencies(self):
        spec = corpus.p4.read(corpus.ROOT / 'tools/s10/sources.json')
        roots = {(corpus.ROOT / root).resolve() for root in spec['roots']}
        pending = list(roots)
        seen = set()
        captured = corpus.sources()
        while pending:
            path = pending.pop()
            if path in seen: continue
            seen.add(path)
            manifest = path / 'Cargo.toml'
            self.assertIn(str(manifest.relative_to(corpus.ROOT)), set(captured))
            data = tomllib.loads(manifest.read_text())
            for section in [data, *data.get('target', {}).values()]:
                # Cargo does not build transitive dependencies' own tests.
                groups = ['dependencies', 'build-dependencies']
                if path in roots: groups.append('dev-dependencies')
                for group in groups:
                    for dependency in section.get(group, {}).values():
                        if isinstance(dependency, dict) and 'path' in dependency:
                            pending.append((path / dependency['path']).resolve())
            for source in path.rglob('*.rs'):
                self.assertIn(str(source.relative_to(corpus.ROOT)), set(captured))

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
