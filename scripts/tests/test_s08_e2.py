"""Acceptance cannot be manufactured by shrinking, skipping or waiving failures."""
import copy
from pathlib import Path
import sys
import tempfile
import unittest
from unittest.mock import patch

sys.path.insert(0, str(Path(__file__).resolve().parents[1]))
import s08_e2 as e2
import s08_e2_contract as c
import s08_e2_obligations as obligations
import test_s08_p5_corpus as p5_tests


class Acceptance(unittest.TestCase):
    def setUp(self):
        base = p5_tests.CorpusContract(); base.setUp()
        self.request, self.row, self.native = base.request, base.row, base.native
        self.request['public_type_strings'] = True
        self.frozen = {k: self.request[k] for k in ('id', 'acceptance_tier', 'type_baseline_requested', 'diagnostic_phases')}
        self.frozen['loading_request_sha256'] = c.digest(c.canonical(self.request['loading']) + b'\n')
        self.pin = '1' * 40
        self.ledger = {'divergence': []}

    def grade(self, **kwargs):
        return c.grade([self.request], [self.row], [self.native], self.ledger, self.pin, **kwargs)

    def enable_types(self):
        self.request['type_baseline_requested'] = True
        self.native.update(baseline_inputs=[], baseline_header='')
        q = {'operation': 'GetTypeAtLocation', 'file': '/a.ts', 'kind': 80, 'pos': 0, 'end': 1}
        display = {'state': 'executed', 'queries': [dict(q, operation='TypeToString', text_hex='616e79')]}
        self.native.update(types={'state': 'content', 'text_hex': '61'}, symbols={'state': 'no_content'},
                           queries=[q], public_type_strings=display)
        self.row['type_symbol_baselines'] = copy.deepcopy({k: self.native[k] for k in ('types', 'symbols', 'queries', 'public_type_strings')})
        self.row['type_symbol_baselines']['state'] = 'executed'

    def test_exact_outcomes_and_disabled_policy_are_observations(self):
        self.assertTrue(all(v == 1 for v in self.grade()['metrics'].values()))
        self.enable_types()
        self.assertEqual(self.grade()['metrics']['type_to_string_parity'], 1)

    def test_partial_smoke_never_emits_metrics(self):
        self.assertEqual(self.grade(partial=True)['metrics'], {})

    def test_inventory_is_complete_ordered_and_source_bound(self):
        c.inventory([self.request], [self.frozen])
        for mutation in ('missing', 'duplicate', 'loading', 'tier', 'phase', 'display'):
            requests = copy.deepcopy([self.request])
            if mutation == 'missing': requests.clear()
            elif mutation == 'duplicate': requests *= 2
            elif mutation == 'loading': requests[0]['loading']['new'] = True
            elif mutation == 'tier': requests[0]['acceptance_tier'] = 'informational'
            elif mutation == 'phase': requests[0]['diagnostic_phases'].pop()
            else: requests[0].pop('public_type_strings')
            with self.subTest(mutation=mutation), self.assertRaises(ValueError): c.inventory(requests, [self.frozen])
        extra = dict(self.frozen, id='second')
        with self.assertRaises(ValueError): c.inventory([self.request], [self.frozen, extra])
        c.inventory([self.request], [self.frozen, extra], partial=True)

    def test_missing_rows_cannot_reduce_the_denominator(self):
        for rows in ([], [self.row, self.row]):
            with self.assertRaises(ValueError): c.grade([self.request], rows, [self.native], self.ledger, self.pin)

    def test_informational_failure_does_not_enter_acceptance_metrics(self):
        request = dict(self.request, id='info', acceptance_tier='informational')
        native = dict(self.native, id='info', acceptance_tier='informational', state='upstream_failed')
        row = e2.p4.fatal(request, 'native_unavailable', 'rejected options')
        result = c.grade([self.request, request], [self.row, row], [self.native, native], self.ledger, self.pin)
        self.assertEqual(result['metrics']['errors_parity'], 1)
        self.assertEqual(result['counts']['informational']['errors_parity'], {'native_unavailable': 1})

    def test_errors_and_query_and_display_differences_are_independent(self):
        self.enable_types()
        original = copy.deepcopy(self.row)
        for metric in c.METRICS:
            self.row = copy.deepcopy(original)
            if metric == 'errors_parity': self.row['error_baseline']['pretty'] = True
            elif metric == 'types_parity': self.row['type_symbol_baselines']['types']['text_hex'] = '62'
            else: self.row['type_symbol_baselines']['public_type_strings']['queries'][0]['text_hex'] = '6e65766572'
            with self.subTest(metric=metric):
                metrics = self.grade()['metrics']
                self.assertEqual(metrics[metric], 0)
                self.assertFalse(metrics['divergences_approved'])
                for other in set(c.METRICS) - {metric}: self.assertEqual(metrics[other], 1)

    def test_display_must_cover_all_actual_type_queries(self):
        self.enable_types()
        self.row['type_symbol_baselines']['public_type_strings']['queries'] = []
        with self.assertRaisesRegex(ValueError, 'schedule'): self.grade()

    def test_old_capture_or_failed_display_cannot_pass(self):
        self.enable_types()
        self.row['type_symbol_baselines'].pop('public_type_strings')
        self.assertEqual(self.grade()['metrics']['type_to_string_parity'], 0)
        with self.assertRaisesRegex(ValueError, 'public-type-strings'): e2.native_current({})

    def test_exact_approved_differences_do_not_cover_changed_bytes(self):
        self.row['error_baseline']['pretty'] = True
        check = self.grade()['rows'][0]['checks']['errors_parity']
        entry = {'id': 'D1', 'title': 'specific difference', 'scope': [self.request['id']],
                 'kind': 'message', 'rationale': 'owner decision', 'approved_by': 'test-owner',
                 'approved_on': '2026-09-14', 'upstream_pin': self.pin,
                 'observations': [dict(variant_id=self.request['id'], metric='errors_parity',
                                       **{k: check[k] for k in ('native_sha256', 'rust_sha256')})]}
        self.ledger['divergence'] = [entry]
        self.assertEqual(self.grade()['metrics']['errors_parity'], 1)
        self.assertTrue(self.grade()['metrics']['divergences_approved'])
        self.row['error_baseline']['baseline'] = {'state': 'content', 'text_hex': 'fe'}
        self.assertEqual(self.grade()['metrics']['errors_parity'], 0)
        self.row = e2.p4.fatal(self.request, 'panic', 'never waive a panic')
        self.assertTrue(all(self.grade()['metrics'][m] == 0 for m in c.METRICS))

    def test_glob_without_exact_witnesses_is_not_an_approval(self):
        self.ledger['divergence'] = [{'id': 'D1', 'scope': ['*']}]
        with self.assertRaises(ValueError): self.grade()

    def test_unused_informational_approval_does_not_gate_acceptance(self):
        entry = {'id': 'D-info', 'title': 'informational', 'scope': ['info'], 'kind': 'other',
                 'rationale': 'not an acceptance case', 'approved_by': 'test-owner', 'approved_on': '2026-09-14',
                 'upstream_pin': self.pin, 'observations': [{'variant_id': 'info', 'metric': 'errors_parity',
                 'native_sha256': '0' * 64, 'rust_sha256': '1' * 64}]}
        self.ledger['divergence'] = [entry]
        request = dict(self.request, id='info', acceptance_tier='informational')
        row = e2.p4.fatal(request, 'native_unavailable', 'rejected options')
        native = dict(self.native, id='info', acceptance_tier='informational', state='upstream_failed')
        report = c.grade([self.request, request], [self.row, row], [self.native, native], self.ledger, self.pin)
        self.assertTrue(report['metrics']['divergences_approved'])
        self.assertEqual(report['unused_approvals'], [['info', 'errors_parity']])
        # A smoke selection remains usable with approvals elsewhere in the full
        # inventory; it still cannot emit acceptance metrics.
        self.assertEqual(self.grade(partial=True, approval_ids={self.request['id'], 'info'})['metrics'], {})

    def test_real_named_test_inventory_detects_missing_and_duplicate_tests(self):
        obligations.listed(b'one: test\ntwo: test\n', ['one'])
        for output in (b'two: test\n', b'one: test\none: test\n'):
            with self.assertRaises(ValueError): obligations.listed(output, ['one'])

    def test_stale_obligations_are_rejected_before_replay(self):
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            e2.p4.write_new(root / 'capture.json', {'version': 1, 'sources': {'old': 'hash'}})
            with patch.object(obligations, 'inputs', return_value=({}, b'', b'', {})):
                with self.assertRaisesRegex(ValueError, 'stale'): obligations.replay(root, lambda: {'new': 'hash'})

    def test_preflight_failure_precedes_any_rust_capture(self):
        with patch.object(e2.corpus, 'prepare') as prepare:
            with tempfile.TemporaryDirectory() as tmp:
                root = Path(tmp); e2.p4.write_new(root / 'report.json', {'public_type_strings': False})
                with self.assertRaises(ValueError): e2.preflight(root, None)
            prepare.assert_not_called()


if __name__ == '__main__': unittest.main()
