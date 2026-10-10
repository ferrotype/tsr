"""The Phase 7 scenario harness: descriptors, run facts, the comparison with the
oracle's frozen work, and the reader behind perf.py's scenario workloads."""
import copy
import json
from pathlib import Path
import sys
import tempfile
import tomllib
import unittest

ROOT = Path(__file__).resolve().parents[2]
sys.path.insert(0, str(ROOT))
sys.path.insert(0, str(ROOT / 'scripts'))
from tools.phase7.api import capture as api  # noqa: E402
from tools.phase7.bench import capture, common  # noqa: E402
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


def pair(index, go_wall, rust_wall):
    def sample(wall):
        return {'wall_ns': wall, 'peak_rss_bytes': 100 * 2**20, 'operation_ns': wall - 10, 'check_ns': wall // 2,
                'user_ns': wall, 'system_ns': 1, 'effective_checkers': 4, 'stderr': ''}
    return {'index': index, 'order': ['go', 'rust'] if index % 2 == 0 else ['rust', 'go'],
            'go': sample(go_wall), 'rust': sample(rust_wall)}


def synthetic_capture(directory, **changes):
    configurations = []
    for checkers in common.CHECKERS:
        configurations.append({'scenario': 'xstate', 'mode': 'check', 'checkers': checkers, 'failures': [],
                               'load_before': [1.0, 1.0, 1.0], 'load_after': [1.0, 1.0, 1.0],
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
        self.assertFalse(measurement['metadata']['host_busy'])

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
              'controls': {'nodelist': [{'name': 'decode', 'result': {'latency': {'p50': 3.0, 'samplesCount': 5}}}]}}
    report.update(changes)
    return report, tasks


class ApiReaderTests(unittest.TestCase):
    def test_task_keys_drop_the_identifier_count(self):
        self.assertEqual(api.key('getSymbolAtPosition - 2894 identifiers (batched)'),
                         'getsymbolatposition_identifiers_batched')
        self.assertEqual(api.key('transfer program.ts'), 'transfer_program_ts')
        self.assertTrue(api.is_control('TS - load project') and api.is_control('materialize checker.ts'))
        self.assertFalse(api.is_control('spawn API'))

    def test_server_tasks_get_ratios_and_controls_do_not(self):
        report, _ = api_report()
        with tempfile.TemporaryDirectory() as directory:
            (Path(directory) / 'capture.json').write_text(json.dumps(report))
            measurement = api.read_capture(directory)
        self.assertEqual(sorted(measurement['ratios']), sorted(
            f'{prefix}_{mode}{suffix}' for mode in ('sync', 'async') for prefix, suffix in
            (('elapsed', '_spawn_api'), ('elapsed', '_getsymbolatposition_identifiers_batched'), ('peak_rss', ''))))
        self.assertEqual(measurement['ratios']['elapsed_sync_spawn_api'], 2.0)
        self.assertEqual(measurement['ratios']['peak_rss_async'], 0.75)
        self.assertEqual(measurement['samples']['rust']['elapsed_sync_spawn_api'], [2.0, 2.0, 2.0])
        self.assertIn('sync: TS - load project', measurement['metadata']['controls_median_ms'])

    def test_runtimes_that_ran_different_tasks_are_refused(self):
        report, tasks = api_report()
        report['runs'][1]['tasks'] = tasks(2.0, rename='getSymbolAtPosition - 2893 identifiers (batched)')
        with tempfile.TemporaryDirectory() as directory:
            (Path(directory) / 'capture.json').write_text(json.dumps(report))
            with self.assertRaisesRegex(ValueError, 'different tasks'):
                api.read_capture(directory)


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
