"""The live CLI witness must observe completed native cycles and actual bytes."""
import copy
from pathlib import Path
import sys
import unittest

sys.path.insert(0, str(Path(__file__).resolve().parents[1]))
import phase4_live as live


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
