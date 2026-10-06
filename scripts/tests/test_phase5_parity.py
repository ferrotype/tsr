"""The native execution, not inferred skips or subtest counts, defines N/F."""
import json
from pathlib import Path
import sys
import tempfile
import unittest
sys.path.insert(0, str(Path(__file__).resolve().parents[1]))
import parity
import phase5_parity


def row(name='TestA', state='pass', suffix='', **fields):
    parent = 'fourslash/' + name
    return dict(id=parent + suffix, parent=parent, state=state, **fields)


class Phase5ParityTests(unittest.TestCase):
    def test_unexpected_skip_fails_but_pinned_skip_is_visible(self):
        out = phase5_parity.compare_parent([row()], [row(state='skip', reason='unsupported')])
        self.assertEqual(out[0]['state'], 'fail')
        self.assertEqual(phase5_parity.counts(out)['failing'], 1)
        out = phase5_parity.compare_parent([row(state='skip')], [row(state='skip')])
        self.assertEqual(out[0]['state'], 'skip')
        self.assertEqual(phase5_parity.counts(out)['executed'], 0)
        self.assertEqual(phase5_parity.counts(out)['native_skips'], 1)

    def test_missing_baseline_cannot_hide_under_passing_parent(self):
        native = [row(), row(suffix='/baseline/TestA/a%2Fb')]
        out = phase5_parity.compare_parent(native, [row()])
        self.assertEqual(phase5_parity.counts(out)['failing'], 1)
        self.assertEqual(out[-1]['reason'], 'native observation missing from Rust execution')

    def test_native_failure_cannot_establish_parity(self):
        out = phase5_parity.compare_parent([row(state='fail')], [row()])
        self.assertEqual(out[0]['state'], 'fail')
        self.assertEqual(out[0]['reason'], 'native reference test failed')

    def test_counts_failures_once_per_parent_and_keeps_approvals(self):
        rows = [row(f'Test{i}', native_state='pass') for i in range(400)]
        rows += [row('Test0', 'fail', '/subtest/a'), row('Test0', 'fail', '/baseline/a/b'),
                 row('Test1', 'fail', '/subtest/a', approved='owner')]
        self.assertEqual(phase5_parity.counts(rows), dict(executed=400, failing=2, native_skips=0, reference_failures=0, limit=2))
        rows.append(row('Test2', 'fail', '/baseline/a/c'))
        self.assertEqual(phase5_parity.counts(rows)['failing'], 3)

    def test_explicit_parent_handles_nested_rows_and_rejects_forgery(self):
        selected = {'fourslash/TestA'}
        self.assertEqual(parity.variant_of('fourslash/TestA/baseline/TestA/a%2Fb', selected, 'fourslash/TestA'), 'fourslash/TestA')
        self.assertNotIn(parity.variant_of('fourslash/TestOther/baseline/x', selected, 'fourslash/TestA'), selected)
        meta = dict(suite='fourslash', variants=list(selected))
        with self.assertRaisesRegex(SystemExit, 'terminal parent'):
            parity.complete(Path('/not-written'), meta, [row(suffix='/subtest/a')])
        parity.complete(Path('/not-written'), meta, [row(native_state='pass'), row(suffix='/subtest/a')])

    def test_parent_cannot_be_guessed_or_omitted(self):
        with self.assertRaisesRegex(ValueError, 'missing terminal'):
            phase5_parity.compare_parent([row()], [row(suffix='/subtest/a')])
        with self.assertRaisesRegex(ValueError, 'duplicate'):
            phase5_parity.compare_parent([row()], [row(), row()])
