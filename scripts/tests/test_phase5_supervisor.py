"""Supervision witnesses using an independent scripted event producer."""
import importlib.util
import json
import os
from pathlib import Path
import sys
import tempfile
import unittest

PATH = Path(__file__).resolve().parents[2] / 'tools/phase5/harness/supervisor.py'
spec = importlib.util.spec_from_file_location('phase5_supervisor', PATH)
supervisor = importlib.util.module_from_spec(spec)
spec.loader.exec_module(supervisor)

FAKE = r'''
import json, os, sys, time
from pathlib import Path
counter = Path(os.environ['COUNTER'])
n = int(counter.read_text()) if counter.exists() else 0
counter.write_text(str(n+1))
script = json.loads(os.environ['SCRIPT'])
for event in script[min(n, len(script)-1)]:
    if 'sleep' in event:
        time.sleep(event['sleep'])
    elif 'exit' in event:
        sys.exit(event['exit'])
    elif 'raw' in event:
        print(event['raw'], flush=True)
    else:
        print(json.dumps(event), flush=True)
'''

def event(action, test=None, **extra):
    return dict(Action=action, **({'Test': test} if test else {}), **extra)


class SupervisorTests(unittest.TestCase):
    def run_script(self, scripts, tests=('TestA',), timeout=.5):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            script = root / 'fake.py'
            script.write_text(FAKE)
            calls = []
            rows, times = supervisor.run_batch([sys.executable, str(script)], tests,
                cwd=root, env={**os.environ, 'SCRIPT': json.dumps(scripts), 'COUNTER': str(root/'count')},
                timeout=timeout, local=root/'logs', on_complete=calls.append)
            return rows, times, calls, int((root/'count').read_text())

    def test_nested_and_deleted_baseline(self):
        baseline = {'test':'TestA/child/name','path':'fourslash/state/a.baseline','state':'fail','reason':'deleted'}
        rows, times, calls, count = self.run_script([[event('run','TestA'), event('run','TestA/child/name'),
            event('output','TestA/child/name', Output='TSR_BASELINE '+json.dumps(baseline)+'\n'),
            event('pass','TestA/child/name'), event('pass','TestA'), event('pass')]])
        self.assertEqual(len(rows),3)
        self.assertEqual({r['parent'] for r in rows},{'fourslash/TestA'})
        self.assertIn('child%2Fname', rows[1]['id'])
        self.assertEqual(rows[0]['reason'],'deleted')
        self.assertEqual(len(calls),1)
        self.assertIn('fourslash/TestA',times)
        self.assertEqual(count,1)

    def test_paused_test_retried_after_crash(self):
        rows, _, _, count = self.run_script([
            [event('run','TestA'),event('pause','TestA'),event('run','TestB'),{'exit':2}],
            [event('run','TestA'),event('pass','TestA'),event('pass')]], ('TestA','TestB'))
        self.assertEqual({r['id']:r['state'] for r in rows}, {'fourslash/TestB':'fail','fourslash/TestA':'pass'})
        self.assertEqual(count,2)

    def test_deadline_restarts_unfinished(self):
        rows, _, _, count = self.run_script([
            [event('run','TestA'),{'sleep':2}],
            [event('run','TestB'),event('pass','TestB'),event('pass')]], ('TestA','TestB'), timeout=.15)
        self.assertEqual(rows[0]['reason'],'test deadline exceeded')
        self.assertEqual(rows[1]['state'],'pass')
        self.assertEqual(count,2)

    def test_pause_suspends_active_time(self):
        rows, times, _, _ = self.run_script([[event('run','TestA'),{'sleep':.05},event('pause','TestA'),
            {'sleep':.05},event('cont','TestA'),{'sleep':.05},event('pass','TestA'),event('pass')]], timeout=.2)
        self.assertEqual(rows[0]['state'],'pass')
        self.assertLess(times['fourslash/TestA'],.17)

    def test_duplicate_and_malformed_events_fail_closed(self):
        for script in ([event('run','TestA'),event('run','TestA')], [{'raw':'no json'}],
                       [event('run','TestOther')], [event('pass','TestA')]):
            with self.subTest(script=script), self.assertRaises(supervisor.HarnessError):
                self.run_script([script])

    def test_duplicate_baseline_identity(self):
        output = event('output','TestA',Output='TSR_BASELINE '+json.dumps({'test':'TestA','path':'a','state':'pass'})+'\n')
        with self.assertRaises(supervisor.HarnessError):
            self.run_script([[event('run','TestA'),output,output]])

    def test_package_failure_not_silently_passed(self):
        with self.assertRaisesRegex(supervisor.HarnessError,'package failed'):
            self.run_script([[event('run','TestA'),event('pass','TestA'),event('fail'),{'exit':1}]])

    def test_completed_results_survive_restart(self):
        rows, _, calls, count = self.run_script([
            [event('run','TestA'),event('pass','TestA'),event('run','TestB'),{'exit':2}],
            [event('run','TestC'),event('output','TestC',Output='native skip reason\n'),event('skip','TestC'),event('pass')]],
            ('TestA','TestB','TestC'))
        self.assertEqual([r['state'] for r in rows],['pass','fail','skip'])
        self.assertEqual(rows[2]['reason'],'native skip reason')
        self.assertEqual(len(calls),3)
        self.assertEqual(count,2)

    def test_fragmented_baseline_and_bounded_detail(self):
        parser = supervisor.Events(['TestA'], 'fourslash')
        parser.feed(event('run','TestA'),0)
        baseline = 'TSR_BASELINE '+json.dumps({'test':'TestA','path':'a','state':'fail','reason':'deleted'})+'\n'
        parser.feed(event('output','TestA',Output=baseline[:7]),.1)
        parser.feed(event('output','TestA',Output=baseline[7:]),.2)
        for _ in range(10):
            parser.feed(event('output','TestA',Output='x'*10000+'\n'),.3)
        parser.feed(event('fail','TestA'),.4)
        rows = list(parser.rows['TestA'].values())
        self.assertEqual(rows[0]['reason'],'deleted')
        self.assertLessEqual(len(rows[1]['detail']),65536)

    def test_giant_ordinary_output_discarded_before_control(self):
        parser = supervisor.Events(['TestA'],'fourslash')
        parser.feed(event('run','TestA'),0)
        for _ in range(1500):
            parser.feed(event('output','TestA',Output='x'*1024),.1)
        self.assertIsNone(parser.partial_output['TestA'])
        parser.feed(event('output','TestA',Output='\nTSR_BASE'),.2)
        parser.feed(event('output','TestA',Output='LINE '+json.dumps(
            {'test':'TestA','path':'a','state':'pass'})+'\n'),.3)
        parser.feed(event('pass','TestA'),.4)
        self.assertEqual(len(parser.rows['TestA']),2)
        self.assertLessEqual(len(parser.output['TestA']),65536)

    def test_giant_control_output_rejected(self):
        parser = supervisor.Events(['TestA'],'fourslash')
        with self.assertRaisesRegex(supervisor.HarnessError,'baseline control'):
            parser.feed(event('output','TestA',Output='TSR_BASELINE '+'x'*1048577),0)

    def test_waiting_parallel_parents_interleave_children(self):
        parser = supervisor.Events(['TestA','TestB'], 'fourslash')
        sequence = [event('run','TestA'),event('pause','TestA'),event('run','TestB'),event('pause','TestB'),
            event('cont','TestA'),event('run','TestA/child'),event('pause','TestA/child'),
            event('cont','TestB'),event('run','TestB/child'),event('pause','TestB/child'),
            event('cont','TestA/child'),event('pass','TestA/child'),event('pass','TestA'),
            event('cont','TestB/child'),event('pass','TestB/child'),event('pass','TestB')]
        for tick, item in enumerate(sequence):
            parser.feed(item,tick)
        self.assertEqual(parser.completed,{'TestA','TestB'})
        self.assertEqual(parser.elapsed['TestA'],6)
        self.assertEqual(parser.elapsed['TestB'],6)

    def test_deadline_preserves_time_across_waiting_parent_resume(self):
        rows, _, _, count = self.run_script([
            [event('run','TestA'),event('pause','TestA'),event('run','TestB'),event('pause','TestB'),
             event('cont','TestA'),{'sleep':.08},event('run','TestA/child'),event('pause','TestA/child'),
             event('cont','TestB'),event('run','TestB/child'),event('pause','TestB/child'),
             event('cont','TestA/child'),{'sleep':.2}],
            [event('run','TestB'),event('pass','TestB'),event('pass')]], ('TestA','TestB'), timeout=.14)
        self.assertEqual(rows[0]['state'],'fail')
        self.assertEqual(rows[0]['reason'],'test deadline exceeded')
        self.assertEqual(rows[1]['state'],'pass')
        self.assertEqual(count,2)

    def test_concurrent_leaves_still_rejected(self):
        parser = supervisor.Events(['TestA','TestB'],'fourslash')
        parser.feed(event('run','TestA'),0)
        parser.feed(event('run','TestA/child'),1)
        with self.assertRaises(supervisor.HarnessError):
            parser.feed(event('run','TestB'),2)

    def test_cont_can_precede_departing_tests_pass_frame(self):
        parser = supervisor.Events(['TestA','TestB'],'fourslash')
        for tick, item in enumerate([event('run','TestA'),event('pause','TestA'),event('run','TestB'),
                event('pause','TestB'),event('cont','TestA'),event('cont','TestB'),
                event('output','TestA',Output='--- PASS: TestA (0.01s)\n'),
                event('pass','TestA'),event('pass','TestB')]):
            parser.feed(item,tick)
        self.assertEqual(parser.completed,{'TestA','TestB'})
        self.assertEqual(parser.elapsed['TestA'],2)
        self.assertEqual(parser.elapsed['TestB'],4)

    def test_baseline_explicit_identity_overrides_lagging_output_test(self):
        parser = supervisor.Events(['TestA','TestB'],'fourslash')
        for tick,item in enumerate([event('run','TestA'),event('pause','TestA'),event('run','TestB'),
                event('pause','TestB'),event('cont','TestA'),event('cont','TestB')]):
            parser.feed(item,tick)
        parser.feed(event('output','TestA',Output='TSR_BASELINE '+json.dumps(
            {'test':'TestB','path':'state/b.baseline','state':'pass'})+'\n'),6)
        self.assertEqual(len(parser.rows['TestB']),1)
        self.assertEqual(len(parser.rows['TestA']),0)

    def test_synthetic_terminal_failure_on_panic_restarts_remaining(self):
        rows, _, calls, count = self.run_script([
            [event('run','TestA'),event('output','TestA',Output='panic: broken transport\n'),event('fail','TestA'),{'exit':2}],
            [event('run','TestB'),event('output','TestB',Output='panic: another fault\n'),event('fail','TestB'),{'exit':2}],
            [event('run','TestC'),event('pass','TestC'),event('pass')]], ('TestA','TestB','TestC'))
        self.assertEqual([r['state'] for r in rows],['fail','fail','pass'])
        self.assertEqual(count,3)
        self.assertEqual(len(calls),3)

    def test_final_synthetic_failed_test_needs_no_package_fail(self):
        rows, _, _, count = self.run_script([[event('run','TestA'),event('fail','TestA'),{'exit':2}]])
        self.assertEqual(rows[0]['state'],'fail')
        self.assertEqual(count,1)

    def test_pass_before_process_death_is_not_attributable_failure(self):
        with self.assertRaisesRegex(supervisor.HarnessError,'unsuccessfully'):
            self.run_script([[event('run','TestA'),event('pass','TestA'),{'exit':2}]])

    def test_two_unidentified_failures_abort(self):
        with self.assertRaisesRegex(supervisor.HarnessError,'two worker failures'):
            self.run_script([[{'exit':1}]])


if __name__ == '__main__':
    unittest.main()
