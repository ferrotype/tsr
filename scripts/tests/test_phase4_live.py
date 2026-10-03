"""The live CLI witness must observe completed native cycles and actual bytes."""
import copy
from pathlib import Path
import shutil
import sys
import unittest

sys.path.insert(0, str(Path(__file__).resolve().parents[1]))
import phase4_live as live
import phase4_native as native
from test_phase4_native import NativeFixture


class LiveComparison(unittest.TestCase):
    def test_build_watch_clock_after_clear_does_not_become_a_diagnostic(self):
        clear = b'\x1b[2J\x1b[3J\x1b[H'
        raw = (clear + b'03:46:42 PM - File change detected. Starting incremental compilation...\n'
               b'src/a.ts(1,2): error TS2304: Missing name\n'
               b'03:46:43 PM - Found 1 error. Watching for file changes.\n')
        self.assertTrue(live.normalized(raw, Path('/root')).startswith(clear))
        self.assertEqual(live.settled_output(raw, Path('/root'))['diagnostics'],
                         'src/a.ts(1,2): error TS2304: Missing name')

    def test_only_last_completed_cycle_is_compared_but_its_diagnostics_survive(self):
        raw = (b'[1:02:03 PM] File change detected. Starting incremental compilation...\n'
               b'src/a.ts(1,2): error TS2304: Missing name\n'
               b'[1:02:04 PM] Found 1 error. Watching for file changes.\n'
               b'[1:02:05 PM] File change detected. Starting incremental compilation...\n'
               b'src/b.ts(4,2): error TS2322: Actual error\n'
               b'[1:02:06 PM] Found 1 error. Watching for file changes.\n')
        self.assertEqual(live.settled_output(raw, Path('/root')),
                         {'errors': 1, 'diagnostics': 'src/b.ts(4,2): error TS2322: Actual error',
                          'status': 'Found 1 error. Watching for file changes.'})

    def test_an_unfinished_or_empty_cycle_cannot_pass(self):
        for raw in (b'', b'Starting compilation in watch mode...\n'):
            with self.assertRaisesRegex(ValueError, 'no completed cycle'):
                live.settled_output(raw, Path('/root'))

    def test_completed_cycle_followed_by_unfinished_rebuild_is_rejected(self):
        with self.assertRaisesRegex(ValueError, 'unfinished cycle'):
            live.settled_output(b'Found 0 errors. Watching for file changes.\n'
                                b'File change detected. Starting incremental compilation...\n', Path('/root'))

    def test_failure_partial_capture_output_and_forced_shutdown_never_pass(self):
        complete = {'rows': [{'step': str(i), 'files': {'out/a.js': '31'}, 'output': {'errors': 0}}
                             for i in range(10)],
                    'error': None, 'termination': {'status': 0, 'forced': False}}
        self.assertTrue(live.same_watch(complete, copy.deepcopy(complete)))
        for change in (lambda row: row.update(error='timeout'),
                       lambda row: row['rows'].pop(),
                       lambda row: row['rows'][3]['files'].update({'out/a.js': '32'}),
                       lambda row: row['rows'][3]['output'].update(errors=1),
                       lambda row: row['termination'].update(forced=True),
                       lambda row: row['termination'].update(status=2)):
            altered = copy.deepcopy(complete)
            change(altered)
            self.assertFalse(live.same_watch(complete, altered))


class LiveAcceptance(NativeFixture):
    def live_capture(self):
        groups = {}
        for runtime in ('go', 'rust'):
            shutil.copy2(self.build / (runtime + '-executable'), self.capture / (runtime + '-executable'))
        for mode in live.MODES:
            group = {'pass': False}
            for runtime in ('go', 'rust'):
                base = self.capture / mode / runtime
                project = base / 'project'
                live.prepare(project)
                physical = Path('/tmp/tsr-phase4-live-original/real')
                cwd = physical.parent / 'linked' if mode.endswith('-symlink') else physical
                arguments = (['-b', '--watch'] if mode.startswith('build-') else ['--watch']) + ['--pretty', 'false']
                self.save(base / 'process/invocation.json', {
                    'command': [str(self.recorded_capture / (runtime + '-executable')), *arguments],
                    'cwd': str(cwd), 'physical_cwd': str(physical),
                    'environment': {'NO_COLOR': '1', 'FORCE_COLOR': None},
                    'settle_timeout_seconds': 30, 'quiet_seconds': 1.2, 'termination_timeout_seconds': 10})
                rows, stdout = [], b''
                for index in range(10):
                    name = live.apply_step(project, index)
                    live.put(project, 'out/state.tsbuildinfo', f'build state {index}\n')
                    raw = (b'[1:02:03 PM] Starting compilation in watch mode...\n'
                           b'[1:02:03 PM] Found 0 errors. Watching for file changes.\n')
                    step = base / f'step-{index}'
                    step.mkdir()
                    (step / 'stdout').write_bytes(raw)
                    (step / 'stderr').write_bytes(b'')
                    self.save(step / 'observation.json', {'index': index, 'step': name,
                              'stdout_start': len(stdout), 'stdout_end': len(stdout) + len(raw)})
                    stdout += raw
                    shutil.copytree(project, step / 'project')
                    rows.append({'step': name, 'output': live.settled_output(raw, cwd), 'files': live.outputs(project)})
                (base / 'process/stdout').write_bytes(stdout)
                (base / 'process/stderr').write_bytes(b'')
                termination = {'status': 0, 'forced': False}
                self.save(base / 'process/termination.json', termination)
                group[runtime] = {'rows': rows, 'error': None, 'termination': termination}
                self.save(base / 'rows.json', rows)
                self.save(base / 'result.json', group[runtime])
            groups[mode] = group
        report = self.report(groups)
        self.bind(self.capture, report)
        return report

    def test_full_live_replay_ignores_false_flags_and_accepts_relocated_proofs(self):
        self.live_capture()
        moved_build, moved_capture = self.root / 'imported-build', self.root / 'imported-live'
        shutil.move(self.build, moved_build)
        shutil.move(self.capture, moved_capture)
        result = live.verify_witnesses(moved_build, moved_capture, expected_host='linux')
        self.assertEqual(result['metrics'], {'live_watch_parity': True})
        self.assertEqual(result['identities']['live_watch_parity'], native.digest((moved_capture / 'report.json').read_bytes()))

    def test_development_capture_cannot_be_promoted_without_build_proof(self):
        report = self.live_capture()
        report['build_sha256'] = None
        self.bind(self.capture, report)
        with self.assertRaisesRegex(ValueError, 'unauthenticated'):
            live.verify_witnesses(self.build, self.capture)

    def test_missing_mode_and_missing_step_cannot_pass(self):
        report = self.live_capture()
        missing = report['witnesses'].pop('build-watch-symlink')
        self.bind(self.capture, report)
        with self.assertRaisesRegex(ValueError, 'mode inventory'):
            live.verify_witnesses(self.build, self.capture)
        report['witnesses']['build-watch-symlink'] = missing
        row = report['witnesses']['watch']['go']
        row['rows'].pop()
        self.save(self.capture / 'watch/go/result.json', row)
        self.save(self.capture / 'watch/go/rows.json', row['rows'])
        self.bind(self.capture, report)
        with self.assertRaisesRegex(ValueError, 'edit schedule'):
            live.verify_witnesses(self.build, self.capture)

    def test_same_fatal_termination_does_not_pass(self):
        report = self.live_capture()
        for runtime in ('go', 'rust'):
            row = report['witnesses']['watch'][runtime]
            row['termination']['status'] = 2
            self.save(self.capture / 'watch' / runtime / 'process/termination.json', row['termination'])
            self.save(self.capture / 'watch' / runtime / 'result.json', row)
        report['pass'] = report['witnesses']['watch']['pass'] = True
        self.bind(self.capture, report)
        self.assertFalse(live.verify_witnesses(self.build, self.capture)['metrics']['live_watch_parity'])

    def test_modified_step_bytes_cannot_be_hidden_by_rehashing(self):
        report = self.live_capture()
        path = self.capture / 'watch/rust/step-3/stdout'
        path.write_bytes(path.read_bytes().replace(b'0 errors', b'1 errors'))
        self.bind(self.capture, report)
        with self.assertRaisesRegex(ValueError, 'stream or ordering'):
            live.verify_witnesses(self.build, self.capture)

    def test_actual_input_edits_and_copied_image_are_checked(self):
        report = self.live_capture()
        path = self.capture / 'watch/go/step-3/project/src/future/extra.ts'
        original = path.read_bytes()
        path.write_text('wrong scripted edit')
        self.bind(self.capture, report)
        with self.assertRaisesRegex(ValueError, 'scripted edit'):
            live.verify_witnesses(self.build, self.capture)
        path.write_bytes(original)
        (self.capture / 'rust-executable').write_bytes(b'another image')
        self.bind(self.capture, report)
        with self.assertRaisesRegex(ValueError, 'copied executable'):
            live.verify_witnesses(self.build, self.capture)

    def test_other_host_and_unexpected_step_are_rejected(self):
        report = self.live_capture()
        with self.assertRaisesRegex(ValueError, 'another host'):
            live.verify_witnesses(self.build, self.capture, expected_host='macos')
        self.save(self.capture / 'watch/go/step-10/observation.json', {})
        self.bind(self.capture, report)
        with self.assertRaisesRegex(ValueError, 'unexpected artifact'):
            live.verify_witnesses(self.build, self.capture)
