"""Supplemental results authenticate the driver, selection and native inputs."""
import contextlib
import io
from pathlib import Path
import sys
import tempfile
import unittest
from unittest.mock import patch

sys.path.insert(0, str(Path(__file__).resolve().parents[1]))
import s08_p5_recheck as recheck
import test_s08_p5_corpus as fixtures
p5 = recheck.p5


class RecheckContract(unittest.TestCase):
    def setUp(self):
        self.fixture = fixtures.CorpusContract()
        self.fixture.setUp()
        self.selection = {'version': 1, 'selection': 'regression', 'ids': [self.fixture.request['id']]}

    def control(self, root):
        control = root / 'control'; control.mkdir()
        p5.write_new(control / 'requests.json', [self.fixture.request])
        p5.write_new(control / 'expected.json', [self.fixture.native])
        p5.write_new(control / 'capture.json', {
            'requests_sha256': p5.digest((control / 'requests.json').read_bytes()),
            'expected_sha256': p5.digest((control / 'expected.json').read_bytes())})
        return control

    def test_selection_cannot_silently_drop_duplicate_or_invent_ids(self):
        with tempfile.TemporaryDirectory() as tmp:
            control = self.control(Path(tmp))
            for ids in ([], ['missing'], self.selection['ids'] * 2, [True]):
                with self.subTest(ids=ids), self.assertRaises(ValueError):
                    recheck.select(control, {**self.selection, 'ids': ids})
            for name in ('requests.json', 'expected.json'):
                path = control / name; raw = path.read_bytes(); path.write_bytes(raw + b' ')
                with self.assertRaisesRegex(ValueError, 'fingerprint'): recheck.select(control, self.selection)
                path.write_bytes(raw)

    def test_capture_resume_and_replay_authenticate_all_inputs_and_results(self):
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp); control = self.control(root)
            binary = root / 'adapter'
            binary.write_text('#!/usr/bin/env python3\nimport sys\nfrom pathlib import Path\nPath(sys.argv[2]).write_bytes(' + repr(p5.canonical(self.fixture.row)) + ')\n')
            binary.chmod(0o755)
            def build(directory, **_):
                directory.mkdir(); snapshot = directory / 'source-snapshot'; snapshot.mkdir()
                return {'sources': {}, 'source_snapshot': str(snapshot), 'binary': str(binary),
                        'binary_sha256': p5.digest(binary.read_bytes())}
            output = root / 'output'
            with patch.object(p5.p4, 'build', side_effect=build), patch.object(p5, 'sources', return_value={}) as current, contextlib.redirect_stdout(io.StringIO()), contextlib.redirect_stderr(io.StringIO()):
                recheck.run(control, self.selection, output, 5)
                self.assertTrue(p5.read(output / 'report.json')['source_stable'])
                before = (output / 'cases/00000/result.json').read_bytes()
                current.return_value = {'changed.rs': 'changed'}
                recheck.run(control, self.selection, output, 5, resume=True)
                self.assertFalse(p5.read(output / 'report.json')['source_stable'])
                self.assertEqual((output / 'cases/00000/result.json').read_bytes(), before)
                self.assertEqual(recheck.replay(output)[2]['counts_by_tier'], {'acceptance': {'match': 1}, 'informational': {}})
                with self.assertRaisesRegex(ValueError, 'identical'):
                    recheck.run(control, self.selection, output, 6, resume=True)
                for name in ('producer.py', 'selection.json', 'executable', 'requests.json', 'expected.json', 'cases/00000/observation.json'):
                    path = output / name; raw = path.read_bytes(); path.write_bytes(raw + b' ')
                    with self.subTest(name=name), self.assertRaises(ValueError): recheck.replay(output)
                    path.write_bytes(raw)
                row = p5.read(output / 'cases/00000/result.json')
                row['row']['error_baseline']['pretty'] = True
                (output / 'cases/00000/result.json').write_bytes(p5.canonical(row))
                with self.assertRaisesRegex(ValueError, 'raw observation'): recheck.replay(output)

    def test_historical_producer_bytes_match_the_recorded_hashes(self):
        root = p5.ROOT / 'tools/s08/results/p5-corpus/producers'
        records = p5.read(root / 'manifest.json')['records']
        self.assertEqual(len(records), 9)
        for record in records:
            self.assertEqual(p5.digest((root / record['producer']).read_bytes()), record['producer_sha256'])
            selection = p5.read(root / record['selection'])
            self.assertEqual(len(selection['ids']), len(set(selection['ids'])))


if __name__ == '__main__': unittest.main()
