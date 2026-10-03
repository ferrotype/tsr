"""S10 acceptance safeguards without running a compiler or long benchmark."""
import contextlib
import copy
import io
import json
import os
import shlex
import tempfile
import tomllib
import unittest
from pathlib import Path
from unittest.mock import patch

import s10_corpus as corpus
import s10_measure as measure
from s06_ownership import validate_output


class S10Evidence(unittest.TestCase):
    def test_capture_inputs_include_the_measurement_dependencies(self):
        captured = set(corpus.sources())
        for required in ['scripts/s10_measure.py', 'tools/s10/wasm/imports.mjs',
                         'tools/s10/go-parser/main.go', 'tools/s10/rust-consumer/Cargo.lock',
                         'scripts/s07_benchmark_stats.py']:
            self.assertIn(required, captured)

    def test_unrelated_edits_do_not_stale_capture(self):
        import fnmatch
        patterns = corpus.source_patterns()
        for name in ['scripts/s09_format.py', 'tools/s08/p7/child.rs',
                     'crates/tsr_api/src/lib.rs', 'data/s10/initial-acceptance.json']:
            self.assertFalse(any(fnmatch.fnmatchcase(name, p) for p in patterns), name)
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            (root / 'tools/s10').mkdir(parents=True)
            (root / 'tools/s10/sources.json').write_bytes((corpus.ROOT / 'tools/s10/sources.json').read_bytes())
            (root / 'scripts').mkdir()
            dependent = root / 'scripts/s10_measure.py'
            unrelated = root / 'scripts/s09_format.py'
            finder_metadata = root / 'tools/s10/.DS_Store'
            finder_metadata.write_text('original')
            dependent.write_text('original'); unrelated.write_text('original')
            with patch.object(corpus, 'ROOT', root):
                before = corpus.sources()
                self.assertNotIn('tools/s10/.DS_Store', before)
                unrelated.write_text('changed')
                finder_metadata.write_text('changed')
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

    def test_timing_stability_uses_approved_parser_and_node_limits(self):
        for kind, experiment, criterion, rust_ns, approved, original in [
                ('parser', 'E7', 'parse_throughput', 600, 1.5, 2.0),
                ('node', 'E8', 'node_parse_latency', 350, 0.40, 0.10)]:
            with self.subTest(kind=kind):
                configured = measure.threshold(experiment, criterion)
                self.assertEqual(configured, approved)
                raw = self.timing()
                for row in raw['samples']:
                    row['elapsed_ns']['rust'] = rust_ns
                    if kind == 'node':
                        row['order'].reverse()
                for limit, passes in [(configured, True), (original, False)]:
                    with patch.dict(measure.THRESHOLDS, {(experiment, criterion): limit}):
                        result = measure.summarize(raw, kind, 7)
                    self.assertEqual(result['stable'], passes)
                    expected = 1 / limit if kind == 'parser' else limit
                    self.assertEqual(result['bootstrap']['threshold'], expected)

    def test_stale_source_rejected_before_raw_data_or_process_access(self):
        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory)
            corpus.p4.write_new(path / 'capture.json', {'sources': {'old': 'hash'}})
            corpus.p4.write_new(path / 'completed.json', {'source_stable': True})
            with patch.object(measure, 'sources', return_value={'new': 'hash'}), self.assertRaisesRegex(ValueError, 'stale'):
                measure.verify(path)

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
