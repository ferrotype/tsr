"""Small contracts for phase selection, flat weighting and recursive frames."""
import unittest
from compare_bind import phase_for_rust, summarize, suspect_groups


class BindComparisonTests(unittest.TestCase):
    def test_phase_uses_entry_ancestry_and_worker_identity(self):
        bind = {'tsr_binder::bind_parsed_file::{closure#0}'}
        parse = {'tsr_parser::orchestration::parse_source_file_with_counters::{closure#0}'}
        self.assertEqual(phase_for_rust(bind, True), 'bind')
        self.assertEqual(phase_for_rust(parse, True), 'parse')
        self.assertEqual(phase_for_rust(bind, False), 'non_worker')
        self.assertEqual(phase_for_rust({'other::<tsr_binder::bind_parsed_file>'}, True), 'worker_unassigned')
        with self.assertRaisesRegex(ValueError, 'ambiguous'):
            phase_for_rust(bind | parse, True)

    def test_self_partitions_while_inclusive_deduplicates_recursion(self):
        result = summarize([(3, ['leaf', 'bind', 'bind'], {'leaf', 'bind'}),
                            (2, ['bind'], {'bind'}), (1, [], set())])
        self.assertEqual(result['cpu_ns'], 6)
        flat = {r['function']: r['cpu_ns'] for r in result['self']}
        self.assertEqual(flat, {'leaf': 3, 'bind': 2, '<missing stack>': 1})
        inclusive = {r['function']: r['cpu_ns'] for r in result['inclusive']}
        self.assertEqual(inclusive, {'bind': 5, 'leaf': 3})
        self.assertAlmostEqual(sum(r['percent_bind_cpu'] for r in result['self']), 100)
        self.assertEqual(result['self_callers']['leaf'], {'bind': 3})
        self.assertEqual(result['self_callers']['bind'], {'<no caller>': 2})

    def test_physical_names_cannot_replace_displayed_self(self):
        name = '<tsr_ast::storage::AstView>::source_file'
        groups = suspect_groups([(7, ['inlined_display'], {name, 'inlined_display'})])
        self.assertEqual(groups['source_metadata']['inclusive_ns'], 7)
        self.assertEqual(groups['source_metadata']['displayed_self_ns'], 0)
        self.assertEqual(groups['stack_guard']['inclusive_ns'], 0)


if __name__ == '__main__':
    unittest.main()
