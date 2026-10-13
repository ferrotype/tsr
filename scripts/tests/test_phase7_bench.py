"""The Phase 7 scenario harness: descriptors, run facts, the comparison with the
oracle's frozen work, and the reader behind perf.py's scenario workloads."""
import copy
from contextlib import ExitStack
import json
import os
from pathlib import Path
import shutil
import subprocess
import sys
import tempfile
import threading
import tomllib
import unittest
from unittest import mock

ROOT = Path(__file__).resolve().parents[2]
sys.path.insert(0, str(ROOT))
sys.path.insert(0, str(ROOT / 'scripts'))
from tools.phase7.api import capture as api  # noqa: E402
from tools.phase7.bench import capture, common, pairs  # noqa: E402
import perf  # noqa: E402

SHA = 'a' * 64
ROOT_DIR = '/work/scenario'


def stdout(diagnostics=(), listed=('a.ts', 'b.ts'), check='0.400s', types=1000, files=None):
    lines = list(diagnostics) + [f'{ROOT_DIR}/{name}' for name in listed] + ['bundled:///libs/lib.es5.d.ts']
    lines += [f'Files:          {files if files is not None else len(listed) + 1}', 'Lines:          10',
              f'Types:          {types}', 'Memory used:    1024K', 'Parse time:     0.010s']
    if check is not None:
        lines.append(f'Check time:     {check}')
    lines += ['Emit time:      0.001s', 'Total time:     0.500s']
    return '\n'.join(lines) + '\n'


def frozen_work(facts, mode, **extra):
    work = {'checkers': 4, 'exit': facts['exit'], 'files': facts['files'], 'listed': facts['listed'],
            'inputs_sha256': SHA, 'options_sha256': SHA, 'output_sha256': facts['output_sha256'],
            'diagnostics': facts['diagnostics'], 'types': facts['types']}
    if mode == 'emit':
        work.update(emitted=facts['emitted'], emitted_sha256=facts['emitted_sha256'], oracle_runs=3)
    work.update(extra)
    return work


def emit_facts(files):
    facts = common.observe({'exit': 0, 'stdout': stdout()}, ROOT_DIR)
    facts['emitted_files'] = dict(files)
    facts['emitted'], facts['emitted_sha256'] = common.files_digest(facts['emitted_files'])
    return facts


class DescriptorTests(unittest.TestCase):
    def test_the_five_committed_descriptors_validate_and_are_frozen(self):
        for name in common.NAMES:
            with self.subTest(name=name):
                descriptor = common.load(name)
                self.assertIn('work', descriptor)
                self.assertEqual(descriptor['work']['check']['checkers'], common.ORACLE_CHECKERS)

    def test_invalid_descriptors_are_rejected(self):
        base = common.load('xstate')
        cases = {
            'an image without a digest': lambda d: d['install'].update(image='node:24-bookworm'),
            'an emit mode without @OUT@': lambda d: d['modes'].update(emit=['--noEmit', 'false']),
            'a project outside the checkout': lambda d: d.update(project='../elsewhere'),
            'an undeclared varying oracle': lambda d: d['work']['emit'].update(emitted_in_every_run=1),
            'a declaration without notes': lambda d: (d.pop('notes', None), d.update(oracle_emit_varies=True)),
            'an emit mode that stops checking': lambda d: d['work']['emit'].update(types=10),
            'an emit mode that emitted nothing': lambda d: d['work']['emit'].update(emitted=0),
        }
        for label, change in cases.items():
            with self.subTest(label):
                descriptor = copy.deepcopy(base)
                change(descriptor)
                with self.assertRaises(ValueError):
                    common.validate(descriptor, 'xstate')

    def test_mui_docs_declares_its_varying_oracle(self):
        descriptor = common.load('mui-docs')
        self.assertIs(descriptor['oracle_emit_varies'], True)
        emit = descriptor['work']['emit']
        self.assertLess(emit['emitted_in_every_run'], emit['emitted'])
        self.assertEqual(emit['oracle_runs'], common.ORACLE_EMIT_RUNS[1])


class RunFactsTests(unittest.TestCase):
    def test_other_cpu_excludes_only_the_capture_pid(self):
        table = (f'{os.getpid()} 12.0 /usr/bin/python3\n'
                 '1 350.0 /elsewhere/tsrust\n2 280.0 /elsewhere/tsgo\n'
                 '3 90.0 /usr/bin/python3\n4 1.0 /usr/bin/other\n')
        with mock.patch.object(capture.subprocess, 'check_output', return_value=table) as ps:
            observed = capture.other_cpu()
        self.assertEqual(observed['percent'], 721.0)
        self.assertEqual(observed['busiest'][0], ['tsrust', 350.0])
        self.assertIn('pid=,pcpu=,comm=', ps.call_args.args[0])

    def test_output_splits_at_the_statistics_table(self):
        printed, statistics = common.split_output(stdout(diagnostics=['a.ts(1,2): error TS2322: Nope.']))
        self.assertEqual(printed[0], 'a.ts(1,2): error TS2322: Nope.')
        self.assertEqual(statistics['Types'], '1000')
        with self.assertRaisesRegex(ValueError, 'no --extendedDiagnostics'):
            common.split_output('a.ts(1,2): error TS2322: Nope.\n')

    def test_facts_are_relative_to_the_checkout(self):
        facts = common.observe({'exit': 2, 'stdout': stdout(diagnostics=[
            'a.ts(1,2): error TS2322: Nope.', 'error TS5096: Option.'])}, ROOT_DIR)
        self.assertEqual((facts['exit'], facts['diagnostics'], facts['listed'], facts['files']), (2, 2, 3, 3))
        moved = common.observe({'exit': 2, 'stdout': stdout(diagnostics=[
            'a.ts(1,2): error TS2322: Nope.', 'error TS5096: Option.']).replace(ROOT_DIR, '/elsewhere')}, '/elsewhere')
        self.assertEqual(facts['output_sha256'], moved['output_sha256'])
        self.assertEqual((facts['check_seconds'], facts['total_seconds']), (0.4, 0.5))

    def test_emitted_files_are_hashed_by_relative_path(self):
        with tempfile.TemporaryDirectory() as directory:
            (Path(directory) / 'src').mkdir()
            (Path(directory) / 'src/a.js').write_text('a')
            facts = common.observe({'exit': 0, 'stdout': stdout()}, ROOT_DIR, directory)
            self.assertEqual(list(facts['emitted_files']), ['src/a.js'])
            self.assertEqual(facts['emitted'], 1)

    def test_arguments_name_the_checkers_and_the_output_directory(self):
        descriptor = common.load('xstate')
        args = common.arguments(descriptor, 'emit', 8, '/tmp/out')
        self.assertEqual(args[:2], ['-p', '.'])
        self.assertIn('/tmp/out', args)
        self.assertEqual(args[args.index('--checkers') + 1], '8')
        with self.assertRaises(ValueError):
            common.arguments(descriptor, 'emit', 8)


class ProblemTests(unittest.TestCase):
    def test_a_matching_run_has_no_problems(self):
        facts = common.observe({'exit': 0, 'stdout': stdout()}, ROOT_DIR)
        self.assertEqual(common.problems(facts, frozen_work(facts, 'check'), 'check'), [])

    def test_output_exit_and_skipped_checking_are_problems(self):
        facts = common.observe({'exit': 0, 'stdout': stdout()}, ROOT_DIR)
        work = frozen_work(facts, 'check')
        cases = {
            'a different diagnostic': common.observe({'exit': 0, 'stdout': stdout(diagnostics=['x.ts(1,1): error TS1: x.'])}, ROOT_DIR),
            'a missing file': common.observe({'exit': 0, 'stdout': stdout(listed=('a.ts',))}, ROOT_DIR),
            'no checking time': common.observe({'exit': 0, 'stdout': stdout(check=None)}, ROOT_DIR),
            'too few types': common.observe({'exit': 0, 'stdout': stdout(types=100)}, ROOT_DIR),
            'another exit status': {**facts, 'exit': 1},
        }
        for label, other in cases.items():
            with self.subTest(label):
                self.assertTrue(common.problems(other, work, 'check'))

    def test_emitted_files_must_match_exactly(self):
        files = {f'f{index}.js': format(index, '064x') for index in range(200)}
        facts = emit_facts(files)
        work = frozen_work(facts, 'emit')
        self.assertEqual(common.problems(facts, work, 'emit', 'rust'), [])
        changed = emit_facts({**files, 'f0.js': 'b' * 64})
        self.assertTrue(common.problems(changed, work, 'emit', 'rust'))
        fewer = emit_facts({path: sha for path, sha in files.items() if path != 'f0.js'})
        self.assertTrue(common.problems(fewer, work, 'emit', 'go'), 'a deterministic oracle tolerates nothing')

    def test_a_varying_oracle_may_omit_one_percent_and_change_nothing(self):
        files = {f'f{index}.js': format(index, '064x') for index in range(200)}
        union = emit_facts(files)
        work = frozen_work(union, 'emit', emitted_in_every_run=190)
        two = emit_facts({path: sha for path, sha in files.items() if path not in ('f0.js', 'f1.js')})
        three = emit_facts({path: sha for path, sha in files.items() if path not in ('f0.js', 'f1.js', 'f2.js')})
        changed = emit_facts({**{path: sha for path, sha in files.items() if path != 'f0.js'}, 'f1.js': 'c' * 64})
        self.assertEqual(common.problems(two, work, 'emit', 'go', files), [])
        self.assertRegex(' '.join(common.problems(three, work, 'emit', 'go', files)), 'omitted 3')
        self.assertRegex(' '.join(common.problems(changed, work, 'emit', 'go', files)), 'differ')
        self.assertTrue(common.problems(two, work, 'emit', 'go', None), 'no reference, no tolerance')
        self.assertTrue(common.problems(two, work, 'emit', 'rust', files), 'only the oracle may omit')


class ConcurrentPairTests(unittest.TestCase):
    def setUp(self):
        stack = self.enterContext(ExitStack())
        self.root = Path(stack.enter_context(tempfile.TemporaryDirectory())).resolve()
        self.control, self.candidate = self.root / 'control', self.root / 'candidate'
        self.control.write_bytes(b'control executable')
        self.candidate.write_bytes(b'candidate executable')
        stack.enter_context(mock.patch.object(common, 'CACHE', self.root / 'cache'))
        stack.enter_context(mock.patch.object(common, 'checkout', return_value=Path(ROOT_DIR)))
        stack.enter_context(mock.patch.object(common, 'provisioned', return_value=True))
        self.unchanged = stack.enter_context(mock.patch.object(common, 'require_unchanged'))
        stack.enter_context(mock.patch('builtins.print'))
        self.descriptor = copy.deepcopy(common.load('xstate'))
        emitted = self.root / 'expected'
        emitted.mkdir()
        (emitted / 'a.js').write_text('compiled output')
        for mode in common.MODES:
            facts = common.observe({'exit': 0, 'stdout': stdout()}, ROOT_DIR, emitted if mode == 'emit' else None)
            self.descriptor['work'][mode] = frozen_work(facts, mode)

    def result(self, wall=100):
        return {'exit': 0, 'stdout': stdout(), 'stderr': '', 'wall_ns': wall,
                'peak_rss_bytes': wall, 'user_ns': wall, 'system_ns': 1}

    def screen(self, output, mode='check', count=1, **options):
        return pairs.screen(self.descriptor, self.control, self.candidate, output,
                            mode=mode, checkers=4, pairs=count, timeout=17, **options)

    def test_pairs_overlap_and_hash_after_both_finish_with_paired_medians(self):
        rendezvous = threading.Barrier(2)
        lock = threading.Lock()
        calls, counts, finished = [], {'control': 0, 'candidate': 0}, 0
        timings = {'control': [1, 2, 100], 'candidate': [3, 100, 200]}
        observe = common.observe

        def run(binary, cwd, args, timeout):
            nonlocal finished
            with lock:
                index = counts[binary.name]
                counts[binary.name] += 1
                calls.append((binary, cwd, args, timeout))
            # A sequential runner fails here instead of making this a timing
            # assertion sensitive to the test host's scheduler.
            rendezvous.wait(timeout=5)
            out = Path(args[args.index('--outDir') + 1])
            (out / 'a.js').write_text('compiled output')
            with lock:
                finished += 1
            return self.result(timings[binary.name][index])

        def observed(*args):
            with lock:
                self.assertEqual(finished, len(calls))
                self.assertEqual(finished % 2, 0, 'both children must finish before observing either')
            return observe(*args)

        with mock.patch.object(common, 'run', side_effect=run), mock.patch.object(common, 'observe', side_effect=observed):
            report = self.screen(self.root / 'screen', mode='emit', count=3)
        self.assertTrue(report['complete'])
        self.assertEqual(report['method'], 'concurrent pairs; alternating submission order; no profiler')
        self.assertEqual(report['type_count_policy'], 'equal')
        self.assertEqual(report['median_ratios'], dict.fromkeys(pairs.METRICS, 3.0))
        self.assertEqual([row['submission_order'] for row in report['pairs']],
                         [['control', 'candidate'], ['candidate', 'control'], ['control', 'candidate']])
        self.assertEqual(counts, {'control': 3, 'candidate': 3})
        self.assertEqual(report['binaries']['candidate'], common.identity(self.candidate))
        directories = []
        for binary, cwd, args, timeout in calls:
            out = Path(args[args.index('--outDir') + 1])
            directories.append(out)
            self.assertEqual(args, common.arguments(self.descriptor, 'emit', 4, out))
            self.assertEqual((cwd, timeout), (Path(ROOT_DIR), 17))
            self.assertTrue(binary.is_absolute())
        self.assertEqual(len(set(directories)), 6)
        self.assertTrue(all(not directory.exists() for directory in directories))
        self.assertEqual(self.unchanged.call_count, 2)

    def test_serial_pairs_use_one_worker_without_barrier_and_alternate_execution(self):
        events = []
        executor = pairs.concurrent.futures.ThreadPoolExecutor
        observe = common.observe

        def run(binary, cwd, args, timeout):
            # A new child cannot enter while the previous child is unfinished.
            self.assertEqual(len(events) % 2, 0)
            events.append(('start', binary.name))
            result = self.result(100 if binary.name == 'control' else 200)
            events.append(('finish', binary.name))
            return result

        def observed(*args):
            self.assertEqual(len(events) % 4, 0, 'validation must wait for both children')
            return observe(*args)

        with mock.patch.object(common, 'run', side_effect=run), \
                mock.patch.object(common, 'observe', side_effect=observed), \
                mock.patch.object(pairs.concurrent.futures, 'ThreadPoolExecutor', wraps=executor) as pool, \
                mock.patch.object(pairs.threading, 'Barrier', side_effect=AssertionError('serial barrier')):
            report = self.screen(self.root / 'serial', count=3, serial=True)
        self.assertEqual(pool.call_args_list, [mock.call(1)] * 3)
        order = ['control', 'candidate', 'candidate', 'control', 'control', 'candidate']
        self.assertEqual(events, [(event, name) for name in order for event in ('start', 'finish')])
        self.assertTrue(report['complete'])
        self.assertEqual(report['method'], 'serial pairs; alternating execution order; no profiler')
        self.assertEqual(report['median_ratios'], dict.fromkeys(pairs.METRICS, 2.0))
        self.assertEqual([row['submission_order'] for row in report['pairs']],
                         [['control', 'candidate'], ['candidate', 'control'], ['control', 'candidate']])

    def test_serial_pairs_keep_exact_type_validation_and_failure_evidence(self):
        for explicit in (False, True):
            with self.subTest(explicit=explicit):
                output = self.root / f'serial-types-{explicit}'
                options = ({'control_types': 1000, 'candidate_types': 801,
                            'type_count_reason': 'Avoid unused types.'} if explicit else {})

                def run(binary, *args, **kwargs):
                    return {**self.result(), 'stdout': stdout(types=1000 if binary.name == 'control' else 802)}

                message = 'differs from expected' if explicit else 'type counts differ'
                with mock.patch.object(common, 'run', side_effect=run), self.assertRaisesRegex(ValueError, message):
                    self.screen(output, serial=True, **options)
                report = json.loads((output / 'summary.json').read_text())
                self.assertFalse(report['complete'])
                self.assertNotIn('median_ratios', report)
                for role in ('control', 'candidate'):
                    self.assertTrue((output / f'0-{role}.stdout').is_file())

    def test_invalid_pairs_keep_raw_results_and_never_publish_ratios(self):
        cases = {
            'bad-control': ('control', {'exit': 99}, 'exit'),
            'bad-candidate': ('candidate', {'exit': 99}, 'exit'),
            'malformed': ('control', {'stdout': 'compiler failed\n'}, 'statistics'),
            'unequal-types': ('candidate', {'stdout': stdout(types=1001)}, 'type counts differ'),
            'stderr': ('candidate', {'stderr': 'internal error'}, 'stderr'),
            'zero-metric': ('candidate', {'wall_ns': 0}, 'wall_ns must be positive'),
            'raised': ('control', None, 'timed out'),
        }
        for name, (bad, changes, message) in cases.items():
            with self.subTest(name=name):
                directories = []

                def run(binary, cwd, args, timeout):
                    out = Path(args[args.index('--outDir') + 1])
                    directories.append(out)
                    (out / 'a.js').write_text('compiled output')
                    if binary.name == bad and changes is None:
                        raise ValueError('timed out')
                    return {**self.result(), **(changes if binary.name == bad else {})}

                output = self.root / name
                with mock.patch.object(common, 'run', side_effect=run), self.assertRaisesRegex(ValueError, message):
                    self.screen(output, mode='emit')
                report = json.loads((output / 'summary.json').read_text())
                self.assertFalse(report['complete'])
                self.assertEqual(report['pairs'], [])
                self.assertNotIn('median_ratios', report)
                for role in ('control', 'candidate'):
                    self.assertTrue((output / f'0-{role}.json').is_file())
                    if name != 'raised' or role != bad:
                        self.assertTrue((output / f'0-{role}.stdout').is_file())
                        self.assertTrue((output / f'0-{role}.stderr').is_file())
                self.assertTrue(all(not directory.exists() for directory in directories))

    def test_changed_inputs_at_the_end_leave_an_incomplete_summary(self):
        self.unchanged.side_effect = [None, ValueError('inputs changed')]
        output = self.root / 'changed'
        with mock.patch.object(common, 'run', side_effect=lambda *a, **k: self.result()), \
                self.assertRaisesRegex(ValueError, 'inputs changed'):
            self.screen(output)
        report = json.loads((output / 'summary.json').read_text())
        self.assertEqual(len(report['pairs']), 1)
        self.assertFalse(report['complete'])
        self.assertNotIn('median_ratios', report)

    def test_explicit_type_counts_record_the_reason_and_each_pairs_counts(self):
        def run(binary, *args, **kwargs):
            count = 1000 if binary.name == 'control' else 801
            return {**self.result(), 'stdout': stdout(types=count)}

        output = self.root / 'explicit-types'
        with mock.patch.object(common, 'run', side_effect=run):
            report = self.screen(output, count=3, control_types=1000, candidate_types=801,
                                 type_count_reason='  Avoid constructing unused overload types.  ')
        self.assertTrue(report['complete'])
        self.assertEqual(report['type_count_policy'], 'explicit')
        self.assertEqual(report['type_count_expectations'], {'control': 1000, 'candidate': 801})
        self.assertEqual(report['type_count_reason'], 'Avoid constructing unused overload types.')
        self.assertEqual([pair['types'] for pair in report['pairs']],
                         [{'control': 1000, 'candidate': 801}] * 3)
        self.assertEqual(json.loads((output / 'summary.json').read_text()), report)

    def test_explicit_types_reject_either_sides_count_drift_after_a_valid_pair(self):
        for bad, observed in [('control', 999), ('candidate', 802), ('candidate', 1000)]:
            with self.subTest(bad=bad, observed=observed):
                calls = {'control': 0, 'candidate': 0}

                def run(binary, *args, **kwargs):
                    calls[binary.name] += 1
                    count = 1000 if binary.name == 'control' else 801
                    if binary.name == bad and calls[bad] == 2:
                        count = observed
                    return {**self.result(), 'stdout': stdout(types=count)}

                output = self.root / f'drift-{bad}-{observed}'
                with mock.patch.object(common, 'run', side_effect=run), \
                        self.assertRaisesRegex(ValueError, f'{bad}: type count {observed} differs from expected'):
                    self.screen(output, count=3, control_types=1000, candidate_types=801,
                                type_count_reason='Avoid unused type construction.')
                report = json.loads((output / 'summary.json').read_text())
                self.assertFalse(report['complete'])
                self.assertEqual(len(report['pairs']), 1)
                self.assertEqual(calls, {'control': 2, 'candidate': 2})
                self.assertNotIn('median_ratios', report)
                for role in calls:
                    self.assertTrue((output / f'1-{role}.json').is_file())
                raw = json.loads((output / f'1-{bad}.json').read_text())
                self.assertTrue(any('differs from expected' in issue for issue in raw['issues']))

    def test_explicit_types_preserve_the_oracle_type_floor_and_work_checks(self):
        cases = [('type-floor', 100, {'stdout': stdout(types=100)}, 'types'),
                 ('changed-output', 801, {'stdout': stdout(types=801, diagnostics=['x.ts(1,1): error TS1: changed.'])}, 'output'),
                 ('missing-file', 801, {'stdout': stdout(types=801, listed=('a.ts',))}, 'files')]
        for label, candidate_types, changes, message in cases:
            with self.subTest(label=label):
                def run(binary, *args, **kwargs):
                    return {**self.result(), **(changes if binary.name == 'candidate' else {})}

                with mock.patch.object(common, 'run', side_effect=run), self.assertRaisesRegex(ValueError, message):
                    self.screen(self.root / label, control_types=1000, candidate_types=candidate_types,
                                type_count_reason='Exact type counts do not excuse changed work.')

    def test_invalid_explicit_type_policy_cannot_start_a_compiler(self):
        cases = [
            {'control_types': 1000},
            {'candidate_types': 801},
            {'type_count_reason': 'Reason without counts.'},
            {'control_types': 1000, 'candidate_types': 801},
            {'control_types': 1000, 'candidate_types': 801, 'type_count_reason': ' \t '},
            *[{'control_types': control, 'candidate_types': candidate,
               'type_count_reason': 'Invalid count.'}
              for control, candidate in [(0, 801), (1000, 0), (-1, 801), (1000, -1),
                                         (True, 801), (1000, False), (1000.0, 801), (1000, '801')]],
        ]
        with mock.patch.object(common, 'run') as run:
            for index, options in enumerate(cases):
                with self.subTest(options=options), self.assertRaises(ValueError):
                    self.screen(self.root / f'invalid-policy-{index}', **options)
                self.assertFalse((self.root / f'invalid-policy-{index}').exists())
            run.assert_not_called()

    def test_explicit_type_counts_do_not_allow_changed_emitted_bytes(self):
        def run(binary, cwd, args, timeout):
            output = Path(args[args.index('--outDir') + 1])
            (output / 'a.js').write_text('changed output' if binary.name == 'candidate' else 'compiled output')
            return {**self.result(), 'stdout': stdout(types=1000 if binary.name == 'control' else 801)}

        with mock.patch.object(common, 'run', side_effect=run), self.assertRaisesRegex(ValueError, 'emitted'):
            self.screen(self.root / 'explicit-changed-emit', mode='emit',
                        control_types=1000, candidate_types=801,
                        type_count_reason='Avoid unused types while preserving emitted bytes.')

    def test_invalid_count_or_existing_output_cannot_start_a_compiler(self):
        with mock.patch.object(common, 'run') as run:
            with self.assertRaisesRegex(ValueError, 'positive'):
                self.screen(self.root / 'zero', count=0)
            with self.assertRaises(FileExistsError):
                self.screen(self.root)
        run.assert_not_called()

    def test_cli_uses_explicit_paths_and_restores_the_cache(self):
        previous = common.CACHE
        cache = self.root / 'another-cache'
        output = self.root / 'cli-output'
        real_screen = pairs.screen

        def screen(*args, **kwargs):
            self.assertEqual(common.CACHE, cache)
            return real_screen(*args, **kwargs)

        with mock.patch.object(common, 'load', return_value=self.descriptor), \
                mock.patch.object(pairs, 'screen', side_effect=screen), \
                mock.patch.object(common, 'run', side_effect=lambda *a, **k: self.result()):
            pairs.main(['--scenario', 'xstate', '--mode', 'check', '--pairs', '1',
                        '--control', str(self.control), '--candidate', str(self.candidate),
                        '--cache', str(cache), '--output', str(output)])
        self.assertEqual(common.CACHE, previous)
        report = json.loads((output / 'summary.json').read_text())
        self.assertEqual(report['cache'], str(cache))
        self.assertTrue(report['complete'])

    def test_cli_forwards_exact_type_counts_and_reason(self):
        previous = common.CACHE
        output = self.root / 'cli-explicit-types'

        def run(binary, *args, **kwargs):
            return {**self.result(), 'stdout': stdout(types=1000 if binary.name == 'control' else 801)}

        with mock.patch.object(common, 'load', return_value=self.descriptor), \
                mock.patch.object(common, 'run', side_effect=run):
            pairs.main(['--scenario', 'xstate', '--mode', 'check', '--pairs', '1',
                        '--control', str(self.control), '--candidate', str(self.candidate),
                        '--output', str(output), '--control-types', '1000', '--candidate-types', '801',
                        '--type-count-reason', 'Short-circuit avoids constructing unused types.'])
        self.assertEqual(common.CACHE, previous)
        report = json.loads((output / 'summary.json').read_text())
        self.assertTrue(report['complete'])
        self.assertEqual(report['type_count_expectations'], {'control': 1000, 'candidate': 801})
        self.assertEqual(report['type_count_reason'], 'Short-circuit avoids constructing unused types.')

    def test_cli_forwards_serial_mode(self):
        output = self.root / 'cli-serial'
        with mock.patch.object(common, 'load', return_value=self.descriptor), \
                mock.patch.object(common, 'run', side_effect=lambda *a, **k: self.result()):
            pairs.main(['--scenario', 'xstate', '--mode', 'check', '--pairs', '1', '--serial',
                        '--control', str(self.control), '--candidate', str(self.candidate),
                        '--output', str(output)])
        report = json.loads((output / 'summary.json').read_text())
        self.assertTrue(report['complete'])
        self.assertEqual(report['method'], 'serial pairs; alternating execution order; no profiler')


def pair(index, go_wall, rust_wall):
    def sample(wall):
        return {'wall_ns': wall, 'peak_rss_bytes': 100 * 2**20, 'operation_ns': wall - 10, 'check_ns': wall // 2,
                'user_ns': wall, 'system_ns': 1, 'effective_checkers': 4, 'stderr': ''}
    return {'index': index, 'order': ['go', 'rust'] if index % 2 == 0 else ['rust', 'go'],
            'go': sample(go_wall), 'rust': sample(rust_wall)}


def synthetic_capture(directory, busy=150.0, **changes):
    configurations = []
    for checkers in common.CHECKERS:
        configurations.append({'scenario': 'xstate', 'mode': 'check', 'checkers': checkers, 'failures': [],
                               'load_before': [9.0, 9.0, 9.0], 'load_after': [9.0, 9.0, 9.0],
                               'other_cpu_before': {'percent': 150.0 if checkers != 8 else busy, 'busiest': []},
                               'pairs': [pair(index, 1_000_000_000 + index, 2_000_000_000 + index) for index in range(7)]})
    report = {'format': capture.FORMAT, 'complete': True, 'dirty': False, 'revision': 'f' * 40, 'pin': 'e' * 40,
              'finished': '2026-10-10T00:00:00Z', 'modes': ['check'], 'host': {'os': 'darwin', 'architecture': 'arm64', 'cpu_capacity': 18},
              'binaries': {}, 'scenarios': {'xstate': common.load('xstate')}, 'configurations': configurations}
    report.update(changes)
    (Path(directory) / 'capture.json').write_text(json.dumps(report))
    return Path(directory)


class ReaderTests(unittest.TestCase):
    def test_a_complete_capture_yields_the_six_ratios(self):
        with tempfile.TemporaryDirectory() as directory:
            measurement = capture.read_capture(synthetic_capture(directory), 'xstate', 'check')
        self.assertEqual(sorted(measurement['ratios']),
                         ['elapsed_2', 'elapsed_4', 'elapsed_8', 'peak_rss_2', 'peak_rss_4', 'peak_rss_8'])
        self.assertAlmostEqual(measurement['ratios']['elapsed_4'], 2.0, places=2)
        self.assertEqual(measurement['ratios']['peak_rss_8'], 1.0)
        self.assertEqual(len(measurement['samples']['rust']['wall_ns_2']), 7)
        self.assertFalse(measurement['metadata']['host_busy'], 'a high load average alone is the capture itself')
        with tempfile.TemporaryDirectory() as directory:
            busy = capture.read_capture(synthetic_capture(directory, busy=450.0), 'xstate', 'check')
        self.assertTrue(busy['metadata']['host_busy'])

    def test_incomplete_dirty_or_disordered_captures_are_refused(self):
        def disorder(report):
            report['configurations'][0]['pairs'][1]['order'] = ['go', 'rust']
        cases = {'incomplete': {'complete': False}, 'dirty': {'dirty': True}}
        for label, changes in cases.items():
            with self.subTest(label), tempfile.TemporaryDirectory() as directory:
                with self.assertRaises(ValueError):
                    capture.read_capture(synthetic_capture(directory, **changes), 'xstate', 'check')
        with tempfile.TemporaryDirectory() as directory:
            path = synthetic_capture(directory)
            report = json.loads((path / 'capture.json').read_text())
            disorder(report)
            (path / 'capture.json').write_text(json.dumps(report))
            with self.assertRaisesRegex(ValueError, 'out of order'):
                capture.read_capture(path, 'xstate', 'check')
            with self.assertRaisesRegex(ValueError, 'did not measure'):
                capture.read_capture(path, 'vscode', 'check')


def api_report(**changes):
    names = ['spawn API', 'TS - load project', 'materialize program.ts', 'getSymbolAtPosition - 2894 identifiers (batched)']
    def tasks(scale, rename=None):
        return [{'name': rename if rename and name == names[-1] else name,
                 'result': {'state': 'completed', 'latency': {'p50': scale * (index + 1), 'samplesCount': 10}}}
                for index, name in enumerate(names)]
    runs = [{'mode': mode, 'round': index, 'runtime': runtime, 'tasks': tasks(2.0 if runtime == 'rust' else 1.0)}
            for mode in ('sync', 'async') for index in range(3) for runtime in ('go', 'rust')]
    memory = {mode: {'go': {'peak_rss_bytes': 200, 'server_peaks': [100, 200]},
                     'rust': {'peak_rss_bytes': 150, 'server_peaks': [150, 120]}} for mode in ('sync', 'async')}
    report = {'format': api.FORMAT, 'complete': True, 'dirty': False, 'revision': 'f' * 40, 'pin': 'e' * 40,
              'node': '24.20.0', 'finished': '2026-10-10T00:00:00Z', 'host': {'os': 'darwin'}, 'binaries': {},
              'load_before': [1, 1, 1], 'load_after': [1, 1, 1], 'runs': runs, 'memory': memory,
              'work': {mode: {runtime: {name: {'count': 1, 'sha256': SHA} for name in api.WORK}
                              for runtime in ('go', 'rust')} for mode in api.FILES},
              'controls': {'nodelist': [{'name': 'decode', 'result': {'latency': {'p50': 3.0, 'samplesCount': 5}}}]}}
    report.update(changes)
    return report, tasks


class ApiReaderTests(unittest.TestCase):
    def test_preflight_refuses_wrong_answers_before_timing(self):
        report, _ = api_report()
        observation = {'work': report['work']['sync']['go']}
        wrong = copy.deepcopy(observation)
        wrong['work']['getsymbolatlocation_identifiers']['sha256'] = 'b' * 64
        with tempfile.TemporaryDirectory() as directory, \
                mock.patch.object(api, 'prerequisites', return_value=('node', '24.20.0')), \
                mock.patch.object(api.common, 'binaries', return_value={'go': Path('/go'), 'rust': Path('/rust')}), \
                mock.patch.object(api.common, 'identity', return_value={}), \
                mock.patch.object(api, 'host_info', return_value={}), \
                mock.patch.object(api, 'reject_concurrent_builds'), \
                mock.patch.object(api.jsapi, '_place_binary'), \
                mock.patch.object(api, 'run_node', side_effect=[observation, wrong, observation, observation]), \
                mock.patch.object(api, 'run_file') as timed:
            with self.assertRaisesRegex(ValueError, 'API answers differ'):
                api.capture(directory)
            timed.assert_not_called()
            saved = json.loads((Path(directory) / 'capture.json').read_text())
            self.assertFalse(saved['complete'])

    def test_matching_names_do_not_hide_wrong_or_missing_answers(self):
        for mode in api.FILES:
            for name in api.WORK:
                with self.subTest(mode=mode, name=name):
                    report, _ = api_report()
                    report['work'][mode]['rust'][name]['sha256'] = 'b' * 64
                    with self.assertRaisesRegex(ValueError, 'API answers differ'):
                        api.validate(report)
                    report['work'][mode]['rust'].pop(name)
                    with self.assertRaisesRegex(ValueError, 'missing or extra'):
                        api.validate(report)

    def test_legacy_capture_without_answer_check_is_not_accepted(self):
        report, _ = api_report()
        report.pop('work')
        with self.assertRaisesRegex(ValueError, 'no API answer comparison'):
            api.validate(report)

    def test_task_keys_drop_the_identifier_count(self):
        self.assertEqual(api.key('getSymbolAtPosition - 2894 identifiers (batched)'),
                         'getsymbolatposition_identifiers_batched')
        self.assertEqual(api.key('transfer program.ts'), 'transfer_program_ts')
        self.assertTrue(api.is_control('sync', 'TS - load project') and api.is_control('sync', 'materialize checker.ts'))
        self.assertFalse(api.is_control('sync', 'spawn API'))
        self.assertTrue(api.is_control('async', 'spawn API'))

    def test_server_tasks_get_ratios_and_controls_do_not(self):
        report, _ = api_report()
        with tempfile.TemporaryDirectory() as directory:
            (Path(directory) / 'capture.json').write_text(json.dumps(report))
            measurement = api.read_capture(directory)
        self.assertEqual(sorted(measurement['ratios']), sorted(
            ['elapsed_sync_spawn_api'] + [f'{prefix}_{mode}{suffix}' for mode in ('sync', 'async') for prefix, suffix in
                                          (('elapsed', '_getsymbolatposition_identifiers_batched'), ('peak_rss', ''))]))
        self.assertEqual(measurement['ratios']['elapsed_sync_spawn_api'], 2.0)
        self.assertEqual(measurement['ratios']['peak_rss_async'], 0.75)
        self.assertEqual(measurement['samples']['rust']['elapsed_sync_spawn_api'], [2.0, 2.0, 2.0])
        self.assertIn('sync: TS - load project', measurement['metadata']['controls_median_ms'])
        self.assertIn('async: spawn API', measurement['metadata']['controls_median_ms'])

    def test_runtimes_that_ran_different_tasks_are_refused(self):
        report, tasks = api_report()
        report['runs'][1]['tasks'] = tasks(2.0, rename='getSymbolAtPosition - 2893 identifiers (batched)')
        with tempfile.TemporaryDirectory() as directory:
            (Path(directory) / 'capture.json').write_text(json.dumps(report))
            with self.assertRaisesRegex(ValueError, 'different tasks'):
                api.read_capture(directory)


@unittest.skipUnless(shutil.which('node') and (ROOT / 'upstream/node_modules/tinybench').is_dir(),
                     'pinned Node/tinybench dependencies are needed for the lifecycle integration test')
class ApiLifecycleTests(unittest.TestCase):
    def test_symbol_normalization_keeps_identity_names_and_declarations(self):
        module = (api.HERE / 'symbols.mjs').as_uri()
        code = '''
import assert from 'node:assert/strict';
import { symbolObserver } from MODULE;
const make = (name, index = 10) => ({name, escapedName: name, flags: 1, checkFlags: 0,
    declarations: [{path: '/root/file.ts', kind: 1, index}]});
const go = symbolObserver('/root/'), rust = symbolObserver('/root/');
assert.deepEqual(go(make('__@hasInstance@239')), rust(make('__@hasInstance@164')));
assert.deepEqual(go(make('__@hasInstance@239')), rust(make('__@hasInstance@164')));
assert.notDeepEqual(go(make('__@hasInstance@240')), rust(make('__@hasInstance@164')));
assert.notDeepEqual(go(make('x')), rust(make('y')));
assert.notDeepEqual(go(make('x', 10)), rust(make('x', 11)));
assert.notDeepEqual(go(make('___@hasInstance@239')), rust(make('___@hasInstance@164')));
assert.notDeepEqual(go(make('x')), rust({...make('x'), flags: 2}));
assert.notDeepEqual(go(make('x')), rust(null));
'''.replace('MODULE', json.dumps(module))
        result = subprocess.run([shutil.which('node'), '--input-type=module', '-e', code],
                                capture_output=True, text=True, timeout=10)
        self.assertEqual(result.returncode, 0, result.stderr)

    def test_spawn_iterations_reap_children_before_the_next_task(self):
        # Actual tinybench hooks and actual children; both run()/runSync()
        # entry points use the same task timer with untimed async cleanup.
        for entry in ('run', 'runSync'):
            with self.subTest(entry=entry), tempfile.TemporaryDirectory() as directory:
                root = Path(directory)
                (root / 'node_modules').symlink_to(ROOT / 'upstream/node_modules')
                fixture = root / 'lifecycle.mjs'
                fixture.write_text('''
import { Bench } from 'tinybench';
import { spawn } from 'node:child_process';
export async function runBenchmarks() {
    let child;
    const bench = new Bench({time: 0, iterations: 4, warmup: true,
        warmupIterations: 2, warmupTime: 0, teardown() {
            child?.stdin.destroy(); child?.stdout.destroy(); child?.kill(); child = undefined;
        }});
    bench.add('spawn API', () => {
        child = spawn(process.execPath, ['-e', 'setInterval(() => {}, 1000)']);
    }, {async: false});
    bench.add('later query', () => {}, {async: false});
    await bench.ENTRY();
}
'''.replace('ENTRY', entry))
                output = root / 'result.json'
                result = subprocess.run([shutil.which('node'), '--experimental-import-meta-resolve',
                                         str(api.HERE / 'capture.mjs'), str(fixture), str(output)],
                                        text=True, capture_output=True, timeout=30)
                self.assertEqual(result.returncode, 0, result.stderr)
                report = json.loads(output.read_text())
                self.assertEqual(report['lifecycle'], {'spawned': 6, 'reaped': 6, 'peak': 1, 'remaining': 0})
                self.assertTrue(all(task['result']['state'] == 'completed' for task in report['benches'][0]['tasks']))


class PerfWiringTests(unittest.TestCase):
    def test_every_scenario_workload_has_its_thresholds(self):
        thresholds = tomllib.loads((ROOT / 'status/perf/thresholds.toml').read_text())
        for name in common.NAMES:
            for mode in common.MODES:
                workload = f'{mode}-{name}'
                with self.subTest(workload):
                    self.assertIn(workload, perf.WORKLOADS)
                    self.assertEqual(thresholds[workload], {**{f'elapsed_{n}': 1.0 for n in common.CHECKERS},
                                                            **{f'peak_rss_{n}': 0.70 for n in common.CHECKERS}})


if __name__ == '__main__':
    unittest.main()
