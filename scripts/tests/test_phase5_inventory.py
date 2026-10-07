"""Tests for the source audit; no server or corpus execution."""
import importlib.util
from pathlib import Path
import tempfile
import unittest

from tools.phase5.harness import runner

ROOT = Path(__file__).resolve().parents[2]
spec = importlib.util.spec_from_file_location('phase5_inventory', ROOT / 'tools/phase5/harness/inventory.py')
inventory = importlib.util.module_from_spec(spec)
spec.loader.exec_module(inventory)


class InventoryTests(unittest.TestCase):
    def test_compiled_roster_rejects_missing_and_unlisted_tests(self):
        rows = [{'package': 'lsp', 'name': 'TestOne'}]
        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory) / 'list'
            path.write_text('TestOne\nPASS\n')
            inventory.verify_lists(rows, [f'lsp={path}'])
            path.write_text('TestTwo\n')
            with self.assertRaisesRegex(ValueError, 'source-only=.*TestOne.*compiled-only=.*TestTwo'):
                inventory.verify_lists(rows, [f'lsp={path}'])

    def test_pinned_audit_separates_main_skips_and_helper_client_routes(self):
        rows = inventory.collect(ROOT)
        self.assertFalse(any(row['name'] == 'TestMain' for row in rows))
        fs = [row for row in rows if row['package'] == 'fourslash/tests']
        self.assertEqual(len(fs), 4547)
        self.assertEqual(sum(row['first_statement_skip'] for row in fs), 386)
        self.assertEqual(sum(row['tsc_prebuild'] for row in fs), 8)
        completion = next(row for row in rows if row['name'] == 'TestCompletionAfterFileClose')
        self.assertEqual(completion['route'], 'lsp/TestCompletionAfterFileClose')
        self.assertEqual(runner.LSP_TESTS, {row['name'] for row in rows
                         if row['package'] == 'lsp' and row['route'] == f"lsp/{row['name']}"})
        self.assertIn('Compiled rosters checked in this generation: **none**', inventory.markdown(rows, []))

    def test_bounded_direct_audit_keeps_partial_observations_as_work(self):
        rows = {(row['package'], row['name']): row for row in inventory.collect(ROOT)}
        self.assertTrue(set(inventory.DIRECT_AUDIT) <= rows.keys())
        self.assertTrue(rows['lsp', 'TestDynamicQueueFIFO']['route'].startswith('Exact observation assignment:'))
        self.assertTrue(rows['lsp/lspwatcher', 'TestRootFromGlob']['route'].startswith('Exact observation assignment:'))
        for name in ('TestDynamicQueueGetCancellation', 'TestDynamicQueuePutCancellationWhileStateUnavailable'):
            self.assertTrue(rows['lsp', name]['route'].startswith('Exact observation assignment:'))
            self.assertIn('source audit only, execution not certified here', rows['lsp', name]['route'])
        self.assertIn('Rust Result::Err carries no item', rows['lsp', 'TestDynamicQueueGetCancellation']['route'])
        self.assertIn('subsequent put/get returns 2', rows['lsp', 'TestDynamicQueuePutCancellationWhileStateUnavailable']['route'])
        self.assertTrue(rows['lsp/lspwatcher', 'TestWatcher_CreateChangeDelete']['route'].startswith('WORK:'))
        self.assertIn('changed/deleted', rows['lsp/lspwatcher', 'TestWatcher_CreateChangeDelete']['route'])


if __name__ == '__main__':
    unittest.main()
