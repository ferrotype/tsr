"""Perf adapter registration and missing native heap controls; no measurements."""
import argparse
import importlib
from pathlib import Path
import unittest
from unittest.mock import patch

from tools.phase7.budget import measure

perf = importlib.import_module('scripts.perf')


class BudgetPerfControls(unittest.TestCase):
    def test_budget_record_requires_an_explicit_durable_capture(self):
        for name in ('embed-check-xstate', 'wasm-emit-vscode', 'embed-size', 'wasm-size'):
            with self.subTest(workload=name), self.assertRaisesRegex(ValueError, '--capture must name'):
                perf.record(argparse.Namespace(workload=name, capture=None))

    def test_all_scenario_mode_runtime_adapters_use_the_readonly_reader(self):
        for prefix, runtime in (('embed', 'native'), ('wasm', 'wasm')):
            for scenario in perf.SCENARIOS:
                for mode in perf.SCENARIO_MODES:
                    name = f'{prefix}-{mode}-{scenario}'
                    self.assertIn(name, perf.WORKLOADS)
                    capture = Path('/private/tmp/readonly-budget')
                    with patch.object(measure, 'read_capture', return_value={'verified': True}) as reader:
                        self.assertEqual(perf.WORKLOADS[name].read(capture, argparse.Namespace()), {'verified': True})
                    reader.assert_called_once_with(capture.resolve(), scenario, mode, runtime)

    def test_size_adapters_use_the_readonly_size_reader(self):
        from tools.phase7.budget import size
        for name, runtime in (('embed-size', 'native'), ('wasm-size', 'wasm')):
            with patch.object(size, 'read_capture', return_value={'ratios': {'artifact_bytes': 0.9}}) as reader:
                result = perf.WORKLOADS[name].read(Path('/private/tmp/size'), argparse.Namespace())
                self.assertEqual(result['ratios'], {'artifact_bytes': 0.9})
                reader.assert_called_once_with(Path('/private/tmp/size'), runtime)

    def test_explicit_native_allocation_is_composed_without_rss_substitution(self):
        import types
        measurement = {'ratios': {'elapsed': 0.5, 'peak_rss': 0.6}, 'samples': {'go': {}, 'rust': {}},
                       'metadata': {'unavailable': {'retained_bytes': 'missing'}}}
        allocation = {'ratio': 0.8, 'samples': {'go': [100] * 7, 'rust': [80] * 7},
                      'authorities': {'go': 'HeapAlloc', 'rust': 'live_requested_bytes'}}
        reader = unittest.mock.Mock(return_value=allocation)
        module = types.SimpleNamespace(read_capture=reader)
        real_import = importlib.import_module
        def imported(name):
            return module if name == 'tools.phase7.budget.allocation' else real_import(name)
        with patch.object(measure, 'read_capture', return_value=measurement), patch.object(perf.importlib, 'import_module', side_effect=imported):
            result = perf.WORKLOADS['embed-check-xstate'].read(Path('/private/tmp/ordinary'),
                            argparse.Namespace(allocation_capture=Path('/private/tmp/allocation')))
        reader.assert_called_once_with(Path('/private/tmp/allocation'), 'xstate', 'check',
                                       ordinary_reference=Path('/private/tmp/ordinary'))
        self.assertEqual(result['ratios']['retained_bytes'], 0.8)
        self.assertNotIn('retained_rss', result['ratios'])
        self.assertEqual(result['samples']['rust']['retained_bytes'], [80] * 7)
        self.assertEqual(result['metadata']['unavailable'], {})

    def test_native_retained_heap_missing_is_still_a_failed_threshold(self):
        run = {'ratios': {'elapsed': 0.5, 'peak_rss': 0.6}}
        rows = perf.compare(run, {'elapsed': 1.0, 'peak_rss': 0.70, 'retained_bytes': 1.0})
        self.assertEqual(next(row for row in rows if row[0] == 'retained_bytes'),
                         ('retained_bytes', None, 1.0, 'missing'))
        self.assertNotIn('retained_rss', run['ratios'])

    def test_threshold_misses_do_not_make_the_capture_ineligible(self):
        run = {'ratios': {'elapsed': 2.0, 'peak_rss': 1.2, 'retained_rss': 1.1}}
        rows = perf.compare(run, dict.fromkeys(run['ratios'], 1.0))
        self.assertEqual([row[3] for row in rows], ['miss', 'miss', 'miss'])


if __name__ == '__main__':
    unittest.main()
