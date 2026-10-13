"""Temporary-source recovery and mocked count captures; no compiler execution."""
from contextlib import ExitStack
import json
from pathlib import Path
import subprocess
import tempfile
import unittest
from unittest import mock

from tools.phase7.bench import common, work_counts, work_counts_rust


class BuildInputTests(unittest.TestCase):
    class PreflightPassed(Exception):
        """Stop after the source-status gate, before any instrumentation/build."""

    def setUp(self):
        self.base = Path(self.enterContext(tempfile.TemporaryDirectory()))
        self.root = self.base / 'source'
        self.root.mkdir()
        subprocess.run(['git', 'init', '--quiet', str(self.root)], check=True)
        self.enterContext(mock.patch.object(work_counts, 'ROOT', self.root))
        self.enterContext(mock.patch.object(work_counts.pgo, 'environment', return_value={}))
        self.identity = self.enterContext(mock.patch.object(
            work_counts.pgo, 'source_identity', side_effect=self.PreflightPassed))
        self.execute = self.enterContext(mock.patch.object(work_counts.pgo, 'execute'))
        self.rust_edits = self.enterContext(mock.patch.object(work_counts.work_counts_rust, 'edits'))
        self.go_edits = self.enterContext(mock.patch.object(work_counts.work_counts_go, 'edits'))

    def test_untracked_build_inputs_are_rejected_even_when_dirty_is_allowed(self):
        source = self.root / 'crates/checker/src/new_module.rs'
        source.parent.mkdir(parents=True)
        source.write_text('untracked candidate module\n')
        for allow_dirty in (False, True):
            with self.subTest(allow_dirty=allow_dirty):
                with self.assertRaisesRegex(ValueError, 'untracked build inputs.*new_module.rs'):
                    work_counts.build(self.base / str(allow_dirty), 'unused-go', allow_dirty=allow_dirty)
                self.assertEqual(source.read_text(), 'untracked candidate module\n')
        self.identity.assert_not_called()
        self.execute.assert_not_called()
        self.rust_edits.assert_not_called()
        self.go_edits.assert_not_called()

    def test_untracked_bench_tools_outside_build_inputs_are_allowed(self):
        tool = self.root / 'tools/phase7/bench/new_tool.py'
        tool.parent.mkdir(parents=True)
        tool.write_text('untracked diagnostic tool\n')
        for allow_dirty in (False, True):
            with self.subTest(allow_dirty=allow_dirty):
                with self.assertRaises(self.PreflightPassed):
                    work_counts.build(self.base / str(allow_dirty), 'unused-go', allow_dirty=allow_dirty)
        self.assertEqual(self.identity.call_count, 2)
        self.execute.assert_not_called()

    def test_tracked_candidate_still_requires_allow_dirty(self):
        source = self.root / 'crates/checker/src/lib.rs'
        source.parent.mkdir(parents=True)
        source.write_text('tracked candidate\n')
        subprocess.run(['git', 'add', 'crates'], cwd=self.root, check=True)
        with self.assertRaisesRegex(ValueError, 'commit or preserve source edits'):
            work_counts.build(self.base / 'refused', 'unused-go')
        self.identity.assert_not_called()
        with self.assertRaises(self.PreflightPassed):
            work_counts.build(self.base / 'allowed', 'unused-go', allow_dirty=True)
        self.identity.assert_called_once()
        self.execute.assert_not_called()


class RustOverlayTests(unittest.TestCase):
    def test_missing_and_duplicate_anchors_are_rejected_without_writes(self):
        for matches in (0, 2):
            with self.subTest(matches=matches), tempfile.TemporaryDirectory() as directory:
                root = Path(directory)
                source = root / 'crates/tsr_checker/src/calls.rs'
                source.parent.mkdir(parents=True)
                before = '\n        let node = state.node;\n' * matches
                source.write_text(before)
                with self.assertRaisesRegex(ValueError, f'expected one exact anchor, found {matches}'):
                    work_counts_rust.edits(root)
                self.assertEqual(source.read_text(), before)
                self.assertFalse((source.parent / 'work_counts.rs').exists())

    def test_existing_helper_is_rejected_before_reading_or_replacing_sources(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            helper = root / 'crates/tsr_checker/src/work_counts.rs'
            helper.parent.mkdir(parents=True)
            helper.write_text('existing helper\n')
            with self.assertRaisesRegex(ValueError, 'diagnostic module already exists'):
                work_counts_rust.edits(root)
            self.assertEqual(helper.read_text(), 'existing helper\n')


class RestorationTests(unittest.TestCase):
    def setUp(self):
        self.base = Path(self.enterContext(tempfile.TemporaryDirectory()))
        self.root = self.base / 'source'
        self.root.mkdir()
        self.original = self.root / 'existing.rs'
        self.original.write_bytes(b'original\r\nbytes\n')
        self.directory = self.base / 'overlay'
        self.edits = {'existing.rs': 'instrumented existing\n', 'new.rs': 'instrumented new\n'}

    def test_restores_exact_existing_bytes_and_removes_new_helper(self):
        before = self.original.read_bytes()
        with work_counts.temporary_sources(self.edits, self.directory, self.root):
            self.assertEqual(self.original.read_text(), self.edits['existing.rs'])
            self.assertEqual((self.root / 'new.rs').read_text(), self.edits['new.rs'])
            self.assertEqual((self.directory / 'original/existing.rs').read_bytes(), before)
            self.assertFalse((self.directory / 'original/new.rs').exists())
            record = json.loads((self.directory / 'restore.json').read_text())
            self.assertEqual(record[1]['before_sha256'], None)
        self.assertEqual(self.original.read_bytes(), before)
        self.assertFalse((self.root / 'new.rs').exists())
        work_counts.restore(self.directory, self.root)  # Recovery is idempotent.
        self.assertEqual(self.original.read_bytes(), before)

    def test_exception_and_interruption_restore_before_propagating(self):
        for exception in (RuntimeError('build failed'), KeyboardInterrupt()):
            with self.subTest(exception=type(exception).__name__):
                directory = self.base / type(exception).__name__
                before = self.original.read_bytes()
                with self.assertRaises(type(exception)):
                    with work_counts.temporary_sources(self.edits, directory, self.root):
                        raise exception
                self.assertEqual(self.original.read_bytes(), before)
                self.assertFalse((self.root / 'new.rs').exists())

    def test_intervening_existing_edit_is_preserved_and_other_file_recovers(self):
        with self.assertRaisesRegex(ValueError, 'existing.rs'):
            with work_counts.temporary_sources(self.edits, self.directory, self.root):
                self.original.write_text('concurrent existing edit')
        self.assertEqual(self.original.read_text(), 'concurrent existing edit')
        self.assertFalse((self.root / 'new.rs').exists())
        self.assertEqual((self.directory / 'original/existing.rs').read_bytes(), b'original\r\nbytes\n')

    def test_intervening_new_helper_edit_is_preserved(self):
        with self.assertRaisesRegex(ValueError, 'new.rs'):
            with work_counts.temporary_sources(self.edits, self.directory, self.root):
                (self.root / 'new.rs').write_text('concurrent new helper edit')
        self.assertEqual(self.original.read_bytes(), b'original\r\nbytes\n')
        self.assertEqual((self.root / 'new.rs').read_text(), 'concurrent new helper edit')

    def test_deleted_existing_source_is_not_recreated(self):
        with self.assertRaisesRegex(ValueError, 'existing.rs'):
            with work_counts.temporary_sources(self.edits, self.directory, self.root):
                self.original.unlink()
        self.assertFalse(self.original.exists())
        self.assertFalse((self.root / 'new.rs').exists())

    def test_changed_backup_is_not_used_to_overwrite_source(self):
        with self.assertRaisesRegex(ValueError, 'backup changed'):
            with work_counts.temporary_sources(self.edits, self.directory, self.root):
                (self.directory / 'original/existing.rs').write_text('corrupt backup')
        self.assertEqual(self.original.read_text(), self.edits['existing.rs'])
        self.assertEqual((self.directory / 'original/existing.rs').read_text(), 'corrupt backup')
        self.assertFalse((self.root / 'new.rs').exists())

    def test_existing_recovery_state_is_not_overwritten(self):
        self.directory.mkdir()
        saved = self.directory / 'original/existing.rs'
        saved.parent.mkdir()
        saved.write_text('only surviving original')
        record = self.directory / 'restore.json'
        record.write_text('existing recovery record')
        with self.assertRaises((ValueError, FileExistsError)):
            with work_counts.temporary_sources(self.edits, self.directory, self.root):
                self.fail('existing recovery state must be rejected before yielding')
        self.assertEqual(saved.read_text(), 'only surviving original')
        self.assertEqual(record.read_text(), 'existing recovery record')
        self.assertEqual(self.original.read_bytes(), b'original\r\nbytes\n')
        self.assertFalse((self.root / 'new.rs').exists())

    def test_edit_after_backup_before_instrumentation_is_not_overwritten(self):
        write = work_counts.pgo.write
        def intervene(path, record):
            write(path, record)
            self.original.write_text('edit after backup')
        with mock.patch.object(work_counts.pgo, 'write', side_effect=intervene), \
                self.assertRaisesRegex(ValueError, 'existing.rs'):
            with work_counts.temporary_sources(self.edits, self.directory, self.root):
                self.fail('intervening edit must be rejected before yielding')
        self.assertEqual(self.original.read_text(), 'edit after backup')
        self.assertEqual((self.directory / 'original/existing.rs').read_bytes(), b'original\r\nbytes\n')
        self.assertFalse((self.root / 'new.rs').exists())

    def test_escaping_edit_does_not_modify_source(self):
        outside = self.base / 'outside.rs'
        outside.write_text('outside')
        with self.assertRaisesRegex(ValueError, 'escapes'):
            with work_counts.temporary_sources({'../outside.rs': 'overwrite'}, self.directory, self.root):
                self.fail('escaping edit must not yield')
        self.assertEqual(outside.read_text(), 'outside')


class CaptureTests(unittest.TestCase):
    def setUp(self):
        stack = self.enterContext(ExitStack())
        self.root = Path(stack.enter_context(tempfile.TemporaryDirectory())).resolve()
        self.build = self.root / 'build'
        self.build.mkdir()
        for name in ('tsgo', 'tsrust'):
            (self.build / name).write_bytes(name.encode())
        (self.build / 'build.json').write_text(json.dumps({
            'complete': True, 'counter_order': ['boundary'],
            'binaries': {name: common.identity(self.build / name) for name in ('tsgo', 'tsrust')},
        }))
        self.counts = {'format': 1, 'counters': {'boundary': 0}}
        self.executions = []
        stack.enter_context(mock.patch.object(work_counts.pgo, 'environment', return_value={}))
        stack.enter_context(mock.patch.object(work_counts.pgo, 'preflight', return_value={
            'listed_paths': ['/fixture/a.ts'], 'content_sha256': 'inputs', 'options_sha256': 'options'}))
        stack.enter_context(mock.patch.object(common, 'load', return_value={'work': {'check': {}}}))
        stack.enter_context(mock.patch.object(common, 'checkout', return_value=Path('/fixture')))
        stack.enter_context(mock.patch.object(common, 'arguments', return_value=['--checkers', '1']))
        stack.enter_context(mock.patch.object(common, 'observe', return_value={'listed_paths': ['/fixture/a.ts']}))
        stack.enter_context(mock.patch.object(common, 'problems', return_value=[]))
        stack.enter_context(mock.patch.object(work_counts.pgo, 'execute', side_effect=self.execute))
        self.previous_cache = common.CACHE
        self.addCleanup(setattr, common, 'CACHE', self.previous_cache)

    def execute(self, command, cwd, env, prefix, timeout=600):
        self.executions.append(Path(command[0]).name)
        Path(env['TSR_WORK_COUNTS']).write_text(json.dumps(self.counts))
        return {'exit': 0, 'stdout': '', 'stderr': ''}

    def test_zero_counts_are_valid_and_both_runtime_inputs_match(self):
        report = work_counts.capture(self.build, self.root / 'capture', self.root / 'cache', ['xstate'])
        self.assertTrue(report['complete'])
        self.assertEqual(self.executions, ['tsgo', 'tsrust'])
        self.assertEqual(report['scenarios']['xstate']['comparison']['boundary'],
                         {'go': 0, 'rust': 0, 'delta': 0})

    def test_invalid_counter_values_or_inventory_stop_before_second_runtime(self):
        for label, counts in (
                ('boolean', {'format': 1, 'counters': {'boundary': True}}),
                ('negative', {'format': 1, 'counters': {'boundary': -1}}),
                ('float', {'format': 1, 'counters': {'boundary': 1.0}}),
                ('inventory', {'format': 1, 'counters': {'unexpected': 1}}),
                ('format', {'format': 2, 'counters': {'boundary': 1}})):
            with self.subTest(label=label):
                self.counts = counts
                self.executions.clear()
                output = self.root / label
                with self.assertRaises(ValueError):
                    work_counts.capture(self.build, output, self.root / 'cache', ['xstate'])
                self.assertEqual(self.executions, ['tsgo'])
                report = json.loads((output / 'report.json').read_text())
                self.assertFalse(report['complete'])
                self.assertTrue(report['scenarios']['xstate']['go']['issues'])


if __name__ == '__main__':
    unittest.main()
