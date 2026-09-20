"""Small contracts for phase selection, flat weighting and recursive frames."""
import unittest
import analyze_xctrace as rust
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

    def test_old_and_new_archives_use_the_same_selectors(self):
        for prefix in ('ts_', 'tsr_'):
            with self.subTest(prefix=prefix):
                bind = {prefix + 'binder::bind_parsed_file::{closure#0}'}
                parse = {prefix + 'parser::orchestration::parse_source_file_with_counters::{closure#0}'}
                self.assertEqual(phase_for_rust(bind, True), 'bind')
                self.assertEqual(phase_for_rust(parse, True), 'parse')
                self.assertEqual(phase_for_rust(bind, False), 'non_worker')
                with self.assertRaisesRegex(ValueError, 'ambiguous'):
                    phase_for_rust(bind | parse, True)
                wrapper = prefix + 'cpu_profile::profile_bind::{closure#0}'
                self.assertEqual(rust.phase_for([{'name': wrapper}]), 'bind')
                self.assertEqual(rust.phase_for([{'name': prefix + 'cpu_profile::main'}]), 'driver')
                self.assertFalse(rust.marker('foreign::' + wrapper, 'profile_bind'))
                self.assertFalse(rust.marker('other::<' + wrapper + '>', 'profile_bind'))
                lookup = '<' + prefix + 'arena::file::StorageView<' + prefix + 'ast::Node>>::node'
                self.assertTrue(rust.query_flags([{'name': lookup}])['ast_lookup_union'])
                self.assertFalse(rust.query_flags([
                    {'name': 'other::Vec<' + prefix + 'ast::storage::AstView>::node'}
                ])['ast_lookup_union'])

    def test_legacy_matching_keeps_original_report_names(self):
        name = '<ts_ast::storage::AstView>::source_file'
        samples = [(7, [name], {name})]
        groups = suspect_groups(samples)
        self.assertEqual(groups['source_metadata']['displayed_self_ns'], 7)
        self.assertEqual(groups['source_metadata']['leaves'], {name: 7})
        self.assertEqual(summarize(samples)['self'][0]['function'], name)
        frame = {'name': name}
        rust.query_flags([frame])
        self.assertEqual(frame['name'], name)
        self.assertEqual(rust.selector_name('foreign::ts_ast::Node'), 'foreign::ts_ast::Node')

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
