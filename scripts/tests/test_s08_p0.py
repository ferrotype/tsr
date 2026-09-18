"""Adversarial checks for each frozen P0 output class and a native typed-tool fixture."""
import copy
import json
import lzma
import os
from pathlib import Path
import subprocess
import sys
import tempfile
import tomllib
import unittest
from unittest.mock import patch

sys.path.insert(0, str(Path(__file__).resolve().parents[1]))
from s04_common import strict_json_loads
from s08_audit import boundary, prepare as audit, reviewed_boundaries, site_identity
from s08_contracts import compare, diagnostics, ordering, state, validate
from s08_contract_oracle import requests
from s08_inventory import compact, sites
from s08_measurement_contract import measured_ratios, type_mean_ratio
from s08_oracle import ROOT
from s08_queries import action, expected_baseline
import s08_p0 as p0


def load(name):
    path = ROOT / name
    raw = path.read_bytes()
    return strict_json_loads(lzma.decompress(raw) if path.suffix == '.xz' else raw)


class FootprintPolicyTests(unittest.TestCase):
    def test_owner_amendment_agrees_with_all_e5_criteria_and_frozen_receipt(self):
        p0.validate_methods()
        criteria = tomllib.loads((ROOT / 'status/experiments.toml').read_text())['E5']['criteria']
        self.assertEqual({c['id']: (c['op'], c['threshold']) for c in criteria}, {
            'peak_rss': ('<=', 0.85), 'allocated_bytes': ('<=', 0.85),
            'type_footprint': ('<=', 0.85),
        })
        manifest = load('data/s08/p0-contract.json')
        p0.validate_sources({'type-footprint.json': manifest['artifacts']['type-footprint.json']}, p0.DATA)

    def test_old_method_limit_cannot_disagree_with_the_amended_ledger(self):
        read = p0.read

        def old_method(path):
            value = read(path)
            if Path(path) == p0.DATA / 'type-footprint.json':
                value['threshold']['maximum'] = 0.8
            return value

        with patch.object(p0, 'read', side_effect=old_method):
            with self.assertRaisesRegex(ValueError, 'footprint methodology disagrees with ledger'):
                p0.validate_methods()


class P0ProtocolTests(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        cls.spec = load('tools/s08/contracts/relations.json')
        cls.request = requests(cls.spec)
        cls.observed = load('data/s08/supplemental-observations.json.xz')
        cls.text = load('tools/s08/contracts/text.json')
        cls.residuals = load('tools/s08/contracts/residuals.json')

    def validate(self, observed):
        validate(self.spec, self.request, self.text, self.residuals, observed)

    def test_observed_protocol_is_complete(self):
        self.validate(self.observed)

    def test_missing_mode_reordered_requests_and_fake_execution_fail(self):
        for mutation in (
            lambda x: x['rows'].pop(),
            lambda x: x['rows'].reverse(),
            lambda x: x['rows'][0]['groups'].pop(),
            lambda x: x['rows'][0].update(state='not_implemented'),
            lambda x: x['rows'][0].update(panic='bounds'),
            lambda x: x['rows'][0]['groups'][0]['actions'].reverse(),
            lambda x: x['rows'][0]['groups'][0]['actions'][0].update(result=1),
        ):
            altered = copy.deepcopy(self.observed)
            mutation(altered)
            with self.assertRaises(ValueError):
                self.validate(altered)

    def test_cache_counts_and_cold_repeat_state_cannot_be_forged(self):
        row = self.observed['rows'][6]['groups'][1]
        # A real generic fixture allocates during the cold relation and reuses that state.
        self.assertGreater(row['actions'][0]['after']['types_created'], row['actions'][0]['before']['types_created'])
        self.assertEqual(row['actions'][1]['before'], row['actions'][0]['after'])
        altered = copy.deepcopy(row['actions'][0]['after'])
        altered['caches']['assignable']['entries'] += 1
        with self.assertRaises(ValueError):
            state(altered)
        altered = copy.deepcopy(self.observed)
        altered['rows'][6]['groups'][1]['actions'][1]['before']['types_created'] += 1
        with self.assertRaises(ValueError):
            self.validate(altered)

    def test_setup_populated_number_fixtures_are_identity_shortcuts(self):
        for case_id in ('flow-return-inference', 'jsdoc-module'):
            definition = next(row for row in self.spec['cases'] if row['id'] == case_id)
            actual = next(row for row in self.observed['rows'] if row['id'] == case_id)
            self.assertEqual(set(definition['first_call'].values()), {'primitive_identity_shortcut'})
            for group in actual['groups']:
                first = group['actions'][0]
                self.assertEqual(group['display_hex'], {'A': b'number'.hex(), 'B': b'number'.hex()})
                self.assertEqual(group['before_lookup']['caches']['assignable']['entries'], 0)
                self.assertEqual(first['before']['caches']['assignable']['entries'], 1)
                self.assertEqual(first['before'], first['after'])
            for mutation in ('setup_cache', 'fresh_cache', 'resolved_type', 'first_call_work'):
                altered = copy.deepcopy(self.observed)
                group = next(row for row in altered['rows'] if row['id'] == case_id)['groups'][1]
                if mutation == 'setup_cache':
                    group['actions'][0]['before']['caches']['assignable'] = dict(entries=0, result_flags=[])
                elif mutation == 'fresh_cache':
                    group['before_lookup']['caches']['assignable'] = dict(entries=1, result_flags=[1])
                elif mutation == 'resolved_type':
                    group['display_hex']['A'] = b'string'.hex()
                else:
                    for action_row in group['actions']:
                        action_row['after']['types_created'] += 1
                    for action_row in group['actions'][1:]:
                        action_row['before']['types_created'] += 1
                with self.subTest(case=case_id, mutation=mutation), self.assertRaises(ValueError):
                    self.validate(altered)

    def test_declared_cold_lazy_work_and_starting_state_are_enforced(self):
        altered = copy.deepcopy(self.observed)
        group = altered['rows'][6]['groups'][1]
        # Keep state continuity and valid counters while omitting the first call's
        # lazy work. The declared class must reject this independently of compare().
        for action_row in group['actions']:
            action_row['before'] = copy.deepcopy(group['actions'][0]['before'])
            action_row['after'] = copy.deepcopy(group['actions'][0]['before'])
        with self.assertRaises(ValueError):
            self.validate(altered)
        altered = copy.deepcopy(self.observed)
        group = altered['rows'][0]['groups'][1]
        for action_row in group['actions']:
            for endpoint in ('before', 'after'):
                action_row[endpoint]['caches']['identity'] = dict(entries=1, result_flags=[1])
        with self.assertRaises(ValueError):
            self.validate(altered)
        for mutation in ('missing', 'cache_hit', 'lazy_without_work'):
            spec = copy.deepcopy(self.spec)
            if mutation == 'missing':
                del spec['cases'][0]['starting_cache_entries']
            elif mutation == 'cache_hit':
                spec['cases'][14]['first_call']['assignable'] = 'cache_hit'
            else:
                spec['cases'][0]['first_call']['assignable'] = 'cold_lazy_resolution'
            with self.subTest(mutation=mutation), self.assertRaises(ValueError):
                validate(spec, requests(spec), self.text, self.residuals, self.observed)

    def test_complete_diagnostic_payload_and_panic_reason_are_compared(self):
        altered = copy.deepcopy(self.observed)
        diagnostic = altered['rows'][1]['groups'][1]['actions'][0]['diagnostics'][0]
        self.assertTrue(diagnostic['chain'])
        diagnostic['chain'] = []
        with self.assertRaises(ValueError):
            compare(self.observed, altered)
        altered = copy.deepcopy(self.observed)
        altered['supplemental']['residuals']['foreign-checker']['message'] = 'debug overflow'
        with self.assertRaises(ValueError):
            self.validate(altered)
        with self.assertRaises(ValueError):
            diagnostics([dict(diagnostic, category=True)])

    def test_union_shuffle_and_pairwise_outcomes_are_not_sort_claims(self):
        original = self.observed['rows'][0]['groups'][0]['union_ordering']
        self.assertTrue(original)
        altered = copy.deepcopy(original)
        altered[1]['sorted'].pop()
        with self.assertRaises(ValueError):
            ordering(altered)
        altered = copy.deepcopy(original)
        altered[1]['sorted'].reverse()
        with self.assertRaises(ValueError):
            ordering(altered)
        with self.assertRaises(ValueError):
            ordering(original[:-1])

    def test_text_bytes_and_truncation_cannot_be_normalized_away(self):
        text = self.observed['supplemental']['text']
        truncated = next(r for r in text if r['id'] == 'byte-truncation')
        self.assertTrue(truncated['truncated'])
        self.assertGreater(len(truncated['type_full_hex']), len(truncated['type_short_hex']))
        altered = copy.deepcopy(self.observed)
        altered['supplemental']['text'][0]['original_hex'] = 'replacement'.encode().hex()
        with self.assertRaises(ValueError):
            compare(self.observed, altered)
        altered['supplemental']['text'][0]['original_hex'] = 'FF'
        with self.assertRaises(ValueError):
            self.validate(altered)

    def test_node_schedule_requires_coordinates_and_never_uses_unscoped_ids(self):
        query = dict(operation='GetTypeAtLocation', file='/file.ts', kind=80, pos=1, end=2, type_id=10)
        first = action(query, {})
        query['type_id'] = 999
        self.assertEqual(first, action(query, {}))
        for changed in (dict(query, pos=True), dict(query, end=0), dict(query, flags=-1), dict(query, absent=0),
                        {k: v for k, v in query.items() if k != 'pos'}, dict(query, unexpected=1)):
            with self.assertRaises(ValueError):
                action(changed, {})

    def test_baseline_missing_disabled_and_empty_content_stay_distinct(self):
        outcomes = [expected_baseline(row) for row in (
            dict(state='content', text_hex=''), dict(state='no_content'), dict(state='disabled'))]
        self.assertEqual(len({tuple(row) for row in outcomes}), 3)
        for bad in (dict(state='missing'), dict(state='not_implemented'), dict(state='no_content', text_hex='')):
            with self.assertRaises(ValueError):
                expected_baseline(bad)

    def test_request_unknown_fields_and_duplicate_ids_fail(self):
        altered = copy.deepcopy(self.spec)
        altered['cases'].append(altered['cases'][0])
        with self.assertRaises(ValueError):
            requests(altered)
        altered = copy.deepcopy(self.spec)
        altered['options']['strict'] = 1
        with self.assertRaises(ValueError):
            requests(altered)

    def test_measurement_missing_work_and_unusable_denominators_fail(self):
        sample = dict(state='completed', workload_sha256='work', executed_queries=10,
                      elapsed_seconds=2.0, requested_bytes=100, retained_bytes=80)
        samples = [dict(sample) for _ in range(7)]
        ratios = measured_ratios(samples, [dict(sample, elapsed_seconds=1.0) for _ in range(7)], 'work', 10)
        self.assertEqual(ratios['throughput'], .5)
        for key, value in [('elapsed_seconds', 0), ('elapsed_seconds', float('nan')), ('requested_bytes', -1),
                           ('retained_bytes', -1), ('executed_queries', 9), ('state', 'skipped')]:
            bad = copy.deepcopy(samples)
            bad[0][key] = value
            with self.assertRaises(ValueError):
                measured_ratios(bad, samples, 'work', 10)
        with self.assertRaises(ValueError):
            measured_ratios(samples, [dict(sample, retained_bytes=0) for _ in range(7)], 'work', 10)
        with self.assertRaises(ValueError):
            measured_ratios(samples[:-1], samples, 'work', 10)

    def test_census_cannot_omit_unknown_backing_or_inflate_empty_capacity(self):
        complete = dict(state='complete', charged_type_storage_bytes=160, logical_retained_types=2, unassigned_allocations=[])
        self.assertEqual(type_mean_ratio(complete, complete), 1)
        for bad in (dict(complete, logical_retained_types=0), dict(complete, unassigned_allocations=['map']),
                    dict(complete, state='unavailable'), dict(complete, logical_retained_types=True)):
            with self.assertRaises(ValueError):
                type_mean_ratio(bad, complete)

    def test_unknown_typed_boundary_requires_review(self):
        with self.assertRaises(ValueError):
            boundary(dict(file='internal/checker/new.go', signature='func() *NewType', value='newHost.NewType'))


class TypedBoundaryReviewTests(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        cls.closure = load('data/s08/dependency-closure.json.xz')
        cls.review = load('tools/s08/inventory/review.json')
        cls.unresolved = [site for site in sites(cls.closure) if not site['targets']]

    def test_exact_frozen_allowlist_uses_decoded_source_identity(self):
        approved = reviewed_boundaries(self.review)
        self.assertEqual(len(approved), 77)
        self.assertEqual(len(self.unresolved), 77)
        for site in self.unresolved:
            self.assertEqual(boundary(site, approved), boundary(dict(site, index=999999), approved))
            self.assertNotIn('index', site_identity(site))
            self.assertNotIn('targets', site_identity(site))
        self.assertEqual(audit(self.closure)['counts']['reviewed_static_boundaries'], 77)

    def test_new_identity_matching_an_existing_category_pattern_fails(self):
        approved = reviewed_boundaries(self.review)
        for category in ('reflection_runtime', 'source_map_host', 'iterator_continuation', 'context_runtime'):
            site = next(site for site in self.unresolved if boundary(site, approved) == category)
            for field, value in (('line', site['line'] + 1), ('caller', site['caller'] + '.new')):
                with self.subTest(category=category, field=field), self.assertRaises(ValueError):
                    boundary(dict(site, **{field: value}), approved)

    def test_duplicate_or_stale_allowlist_entries_fail(self):
        review = copy.deepcopy(self.review)
        review['unresolved_sites'].append(review['unresolved_sites'][0])
        with self.assertRaises(ValueError):
            reviewed_boundaries(review)
        with patch('s08_audit.sites', return_value=self.unresolved[1:]), self.assertRaises(ValueError):
            audit(self.closure)


@unittest.skipUnless(os.environ.get('S08_NATIVE_TESTS') == '1', 'opt-in pinned Go typed-tool integration')
class NativeInventoryTests(unittest.TestCase):
    def test_direct_generic_interface_and_external_callback_edges(self):
        from s04 import go_environment, verified_upstream
        with tempfile.TemporaryDirectory(prefix='s08-inventory-test-', dir=ROOT / 'target') as tmp:
            directory = Path(tmp)
            (directory / 'go.mod').write_text('module example.test/p0\n\ngo 1.26\n')
            (directory / 'main.go').write_text('''package fixture
import "slices"
type Count = int
type Host interface { Run(int) int }
type worker struct{}
func (worker) Run(n int) int { return n+1 }
var hook = callback
var state int
func init() { register(callback) }
func register(fn func(int) int) { state=fn(2) }
func callback(n int) int { return n*2 }
func invoke(fn func(int) int, n int) int { return fn(n) }
func generic[T any](v T, fn func(T) int) int { return fn(v) }
func throughHost(h Host) int { return h.Run(4) }
func less(a,b int) int { return a-b }
func Root() int { xs:=[]int{2,1};slices.SortFunc(xs,less); return invoke(hook,1)+generic(Count(2),callback)+throughHost(worker{})+xs[0] }
''')
            request = dict(prefix='example.test/', patterns=['.'], roots=['p0::Root'], interfaces=['p0::Host'], goos='linux', goarch='amd64')
            path = directory / 'request.json'
            path.write_text(json.dumps(request))
            cmd = ['go', 'run', '-mod=readonly', str(ROOT / 'tools/s08/inventory/main.go'), '--dir', str(directory), '--request', str(path)]
            env = go_environment()
            cwd = verified_upstream() / 'tsc'
            result = subprocess.run(cmd, cwd=cwd, env=env, capture_output=True, check=True, timeout=120)
            original = strict_json_loads(result.stdout)
            document = compact(original)
            self.assertNotIn('example.test/p0.Count', json.dumps(document))
            self.assertNotIn('func(n int)', json.dumps(document['tables']['signature']))
            selected = {r['selector'] for r in document['functions']}
            self.assertTrue({'p0::Root', 'p0::callback', 'p0::worker.Run', 'p0::less', 'p0::register'} <= selected)
            self.assertEqual([m['name'] for m in document['interfaces']['p0::Host']], ['Run'])
            decoded = list(sites(document))
            self.assertTrue(any(s['kind'] == 'interface' and any('worker).Run' in t for t in s['targets']) for s in decoded))
            self.assertTrue(any(s['kind'] == 'callback_argument' and any('.less@' in t for t in s['targets']) for s in decoded))
            self.assertTrue(any('generic[int]' in name for name in document['identifiers']))
            request['roots'] = ['p0::Missing']
            path.write_text(json.dumps(request))
            failure = subprocess.run(cmd, cwd=cwd, env=env, capture_output=True, timeout=120)
            self.assertNotEqual(failure.returncode, 0)
            self.assertIn(b'missing source root', failure.stderr)


if __name__ == '__main__':
    unittest.main()
