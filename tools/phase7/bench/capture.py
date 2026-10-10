#!/usr/bin/env python3
"""Full checking and checking plus emit of the five scenarios against Go
(docs/PHASE7-plan.md, R0 item 2).

    capture.py build
    capture.py preflight [--scenario NAME ...] [--mode MODE ...]
    capture.py run --output DIR [--scenario NAME ...] [--mode MODE ...] [--pairs 7]

`build` makes the two ordinary release binaries: tsgo from the pin and
tsrust from HEAD (target/phase7/bin). Each sample is one cold process of
either binary on a provisioned scenario (scripts/phase7_scenarios.py),
`-p <project> <mode arguments> --pretty false --checkers N --listFiles
--extendedDiagnostics`, timed from launch to exit, with its peak RSS from its
own rusage. Elapsed is that whole process time; the binary's own `Total time`
(configuration through emit) is kept as the operation time, and the rest as
startup and exit, reported separately.

Every sample is compared with the scenario's frozen oracle work before its
time is kept: exit status, the printed diagnostics and file list, the file
count, the emitted bytes, and evidence that checking ran (a checking time and
at least half the oracle's type count). `preflight` runs one sample of each
runtime per scenario, mode and checker count, checks both runtimes' resolved
options against the oracle's, and plants two controls that must be rejected:
a real Rust run that omits the mode's operation (`--noCheck` or `--noEmit`)
and a valid Rust result with one byte of its output changed. `run` repeats
the preflight, then measures each scenario, mode and checker count in
alternating serial pairs after one discarded warm-up pair: seven pairs,
extended once to fourteen when an elapsed ratio's 95% bootstrap interval
straddles 1.0 or either runtime's relative MAD exceeds 5% (the S07 rule,
s07_benchmark_stats). The host's load is retained with every configuration.

`read_capture(path, scenario, mode)` is the reader behind `perf.py record
check-<scenario>` and `emit-<scenario>`.
"""
from __future__ import annotations

import argparse
from datetime import datetime, timezone
import json
import os
from pathlib import Path
import shutil
import subprocess
import sys
import time

sys.path.insert(0, str(Path(__file__).resolve().parents[3]))
sys.path.insert(0, str(Path(__file__).resolve().parents[3] / 'scripts'))
from tools.phase7.bench import common  # noqa: E402
from s07_benchmark_measure import host_info, reject_concurrent_builds  # noqa: E402
from s07_benchmark_stats import ratio_summary  # noqa: E402

FORMAT = 1
PAIRS = (7, 14)
# Other processes' CPU before a configuration, in percent of one CPU (ps's
# decaying average); above this the configuration ran on a busy host. The load
# average is kept too, but just after a measured compile it reflects that
# compile, not the host.
BUSY_OTHER_CPU = 200.0
OWN_PROCESSES = {'tsgo', 'tsrust'}
THRESHOLDS = {'elapsed': 1.0, 'peak_rss': 0.70}
CONTROLS = {'check': ['--noCheck'], 'emit': ['--noEmit']}


def revision():
    head = subprocess.check_output(['git', 'rev-parse', 'HEAD'], cwd=common.ROOT, text=True).strip()
    dirty = subprocess.check_output(['git', 'status', '--porcelain', '--', 'crates', 'Cargo.toml', 'Cargo.lock'],
                                    cwd=common.ROOT, text=True).strip()
    return head, bool(dirty)


def sample(binary, descriptor, mode, checkers, extra=()):
    """One validated-later run: its raw measurements and comparable facts."""
    root = common.checkout(descriptor)
    out = common.emit_directory() if mode == 'emit' else None
    try:
        result = common.run(binary, root, common.arguments(descriptor, mode, checkers, out, extra))
        facts = common.observe(result, root, out)
    finally:
        if out is not None:
            shutil.rmtree(out, ignore_errors=True)
    operation = facts['total_seconds']
    return {
        'wall_ns': result['wall_ns'], 'peak_rss_bytes': result['peak_rss_bytes'],
        'user_ns': result['user_ns'], 'system_ns': result['system_ns'],
        'operation_ns': None if operation is None else round(operation * 1e9),
        'check_ns': None if facts['check_seconds'] is None else round(facts['check_seconds'] * 1e9),
        'effective_checkers': max(min(checkers, facts['files'], 256), 1),
        'stderr': result['stderr'][:4096],
    }, facts


def validated(binary, descriptor, mode, checkers, runtime, reference=None):
    measured, facts = sample(binary, descriptor, mode, checkers)
    if mode == 'emit':
        measured['emitted'] = facts['emitted']
    found = common.problems(facts, descriptor['work'][mode], mode, runtime, reference)
    if measured['stderr']:
        found.append('wrote to stderr: ' + measured['stderr'][:200])
    return measured, facts, found


def check_scenario(descriptor, binaries):
    """Inputs and options: the checkout still holds the frozen inputs and both
    runtimes resolve the oracle's options."""
    if not common.provisioned(descriptor):
        raise ValueError(f"{descriptor['name']}: not provisioned (scripts/phase7_scenarios.py provision)")
    if 'work' not in descriptor:
        raise ValueError(f"{descriptor['name']}: not frozen (scripts/phase7_scenarios.py freeze)")
    common.require_unchanged(descriptor)
    root = common.checkout(descriptor)
    found = []
    for mode in common.MODES:
        for runtime, binary in binaries.items():
            options, text = common.options_digest(binary, descriptor, mode, root)
            if options != descriptor['work'][mode]['options_sha256']:
                found.append(f'{mode}: {runtime} resolves different options:\n{text[:2000]}')
    return found


def preflight(names, modes, binaries, log=sys.stdout, references=None):
    """One sample per runtime, mode and checker count, the scenario checks and
    the planted controls; returns the report and an estimate in seconds.
    `references` collects, per scenario and emit mode, the emitted files of a
    run whose files are the oracle's whole union (common.problems)."""
    references = {} if references is None else references
    report = {'scenarios': {}, 'controls': {}, 'estimate_seconds': 0, 'passed': True}
    for name in names:
        descriptor = common.load(name)
        entry = report['scenarios'][name] = {'problems': check_scenario(descriptor, binaries), 'samples': {}}
        root = common.checkout(descriptor)
        for mode in modes:
            inputs = None
            reference = None
            if mode == 'emit':
                # The reference: the first Rust run whose files are the whole union.
                _, facts = sample(binaries['rust'], descriptor, mode, common.ORACLE_CHECKERS)
                work = descriptor['work'][mode]
                if (facts['emitted'], facts['emitted_sha256']) == (work['emitted'], work['emitted_sha256']):
                    reference = references[(name, mode)] = facts['emitted_files']
            for checkers in common.CHECKERS:
                walls = {}
                for runtime, binary in binaries.items():
                    measured, facts, found = validated(binary, descriptor, mode, checkers, runtime, reference)
                    if inputs is None:
                        inputs = common.inputs_digest(root, facts['listed_paths'])
                        if inputs != descriptor['work'][mode]['inputs_sha256']:
                            entry['problems'].append(f'{mode}: the checkout no longer holds the frozen inputs')
                    walls[runtime] = measured['wall_ns']
                    entry['samples'][f'{mode}-{checkers}-{runtime}'] = {
                        'wall_ns': measured['wall_ns'], 'peak_rss_bytes': measured['peak_rss_bytes'], 'problems': found}
                    entry['problems'] += [f'{mode} {checkers} {runtime}: {problem}' for problem in found]
                    print(f'preflight {name} {mode} {checkers} {runtime}: {measured["wall_ns"] / 1e9:.2f}s '
                          f'{measured["peak_rss_bytes"] / 2**20:.0f} MB' + (f' PROBLEMS {found}' if found else ''),
                          file=log, flush=True)
                report['estimate_seconds'] += sum(walls.values()) * (PAIRS[0] + 1) / 1e9
            # Planted controls: an omitted operation and a wrong result must fail.
            omitted, omitted_facts = sample(binaries['rust'], descriptor, mode, common.ORACLE_CHECKERS, CONTROLS[mode])
            wrong = dict(validated(binaries['rust'], descriptor, mode, common.ORACLE_CHECKERS, 'rust', reference)[1])
            if mode == 'emit':
                # One byte of one emitted file: the comparison reads the files.
                files = dict(wrong['emitted_files'])
                first = sorted(files)[0]
                files[first] = format(int(files[first], 16) ^ 1, '064x')
                wrong['emitted_files'] = files
                wrong['emitted'], wrong['emitted_sha256'] = common.files_digest(files)
            else:
                wrong['output_sha256'] = format(int(wrong['output_sha256'], 16) ^ 1, '064x')
            work = descriptor['work'][mode]
            controls = {'omitted_operation': common.problems(omitted_facts, work, mode, 'rust', reference),
                        'wrong_result': common.problems(wrong, work, mode, 'rust', reference)}
            if mode == 'emit' and 'emitted_in_every_run' in work:
                # The oracle's own tolerance still rejects a changed file.
                controls['wrong_result_oracle'] = common.problems(wrong, work, mode, 'go', reference)
            report['controls'][f'{name}-{mode}'] = controls
            common.require_unchanged(descriptor)
            for control, found in controls.items():
                if not found:
                    entry['problems'].append(f'{mode}: the planted {control} control was accepted')
                print(f'preflight {name} {mode} control {control}: '
                      + ('rejected: ' + found[0] if found else 'ACCEPTED'), file=log, flush=True)
        if entry['problems']:
            report['passed'] = False
    return report


def load_average():
    return [round(value, 2) for value in os.getloadavg()]


def other_cpu():
    """The CPU the host's other processes use, in percent of one CPU, with
    the five busiest by name (names only, never arguments)."""
    rows = []
    for line in subprocess.check_output(['ps', '-Ao', 'pcpu=,comm='], text=True).splitlines():
        cpu, _, command = line.strip().partition(' ')
        name = Path(command.strip()).name
        if name not in OWN_PROCESSES and name != Path(sys.executable).name:
            rows.append((float(cpu), name))
    rows.sort(reverse=True)
    return {'percent': round(sum(cpu for cpu, _ in rows), 1), 'busiest': [[name, cpu] for cpu, name in rows[:5]]}


def summaries(pairs):
    """Per metric: the S07 ratio summary over complete pairs."""
    result = {}
    for metric, field in (('elapsed', 'wall_ns'), ('peak_rss', 'peak_rss_bytes')):
        go = [pair['go'][field] for pair in pairs]
        rust = [pair['rust'][field] for pair in pairs]
        result[metric] = ratio_summary(go, rust, timing=metric == 'elapsed', threshold=THRESHOLDS[metric],
                                       accepted_counts=PAIRS)
    return result


def measure(descriptor, mode, checkers, binaries, pairs, log, reference=None):
    """One configuration: a warm-up pair, then `pairs` alternating pairs, once
    extended to the larger count when the elapsed summary asks for it."""
    config = {'scenario': descriptor['name'], 'mode': mode, 'checkers': checkers,
              'load_before': load_average(), 'other_cpu_before': other_cpu(),
              'warmup': {}, 'pairs': [], 'failures': []}
    for runtime in ('go', 'rust'):
        measured, _, found = validated(binaries[runtime], descriptor, mode, checkers, runtime, reference)
        config['warmup'][runtime] = {'wall_ns': measured['wall_ns'], 'problems': found}
        config['failures'] += [f'warm-up {runtime}: {problem}' for problem in found]
    target = pairs
    while not config['failures'] and len(config['pairs']) < target:
        index = len(config['pairs'])
        order = ('go', 'rust') if index % 2 == 0 else ('rust', 'go')
        pair = {'index': index, 'order': list(order)}
        for runtime in order:
            measured, _, found = validated(binaries[runtime], descriptor, mode, checkers, runtime, reference)
            pair[runtime] = measured
            config['failures'] += [f'pair {index} {runtime}: {problem}' for problem in found]
        config['pairs'].append(pair)
        if len(config['pairs']) == target and target == PAIRS[0] and not config['failures']:
            if summaries(config['pairs'])['elapsed']['needs_more']:
                target = PAIRS[1]
    config['load_after'] = load_average()
    if not config['failures']:
        config['summaries'] = summaries(config['pairs'])
        elapsed = config['summaries']['elapsed']
        print(f"{descriptor['name']} {mode} {checkers}: elapsed {elapsed['ratio']:.3f} "
              f"[{elapsed['bootstrap']['lower']:.3f}, {elapsed['bootstrap']['upper']:.3f}], "
              f"peak RSS {config['summaries']['peak_rss']['ratio']:.3f}, {len(config['pairs'])} pairs, "
              f"other CPU {config['other_cpu_before']['percent']:.0f}%", file=log, flush=True)
    else:
        print(f"{descriptor['name']} {mode} {checkers}: FAILED {config['failures'][:3]}", file=log, flush=True)
    return config


def capture(names, modes, output, pairs=PAIRS[0], log=sys.stdout):
    if output.exists() and any(output.iterdir()):
        raise ValueError(f'{output} is not empty; a capture is never overwritten')
    output.mkdir(parents=True, exist_ok=True)
    binaries = common.binaries()
    head, dirty = revision()
    host = host_info(minimum_cpus=max(common.CHECKERS))
    reject_concurrent_builds()
    report = {'format': FORMAT, 'revision': head, 'dirty': dirty, 'pin': json.loads((common.ROOT / 'data/upstream.json').read_text())['pin'],
              'started': datetime.now(timezone.utc).strftime('%Y-%m-%dT%H:%M:%SZ'), 'host': host,
              'binaries': {runtime: common.identity(path) for runtime, path in binaries.items()},
              'scenarios': {name: common.load(name) for name in names}, 'modes': list(modes),
              'pairs': pairs, 'configurations': [], 'complete': False}
    path = output / 'capture.json'
    try:
        references = {}
        report['preflight'] = preflight(names, modes, binaries, log, references)
        if not report['preflight']['passed']:
            raise ValueError('preflight failed; nothing was measured')
        print(f"preflight passed; estimated {report['preflight']['estimate_seconds'] / 60:.0f} minutes", file=log, flush=True)
        for name in names:
            descriptor = report['scenarios'][name]
            for mode in modes:
                for checkers in common.CHECKERS:
                    report['configurations'].append(measure(descriptor, mode, checkers, binaries, pairs, log,
                                                            references.get((name, mode))))
                    path.write_text(json.dumps(report, indent=1) + '\n')
        for name in names:
            descriptor = report['scenarios'][name]
            root = common.checkout(descriptor)
            listed = common.observe(common.run(binaries['go'], root, common.arguments(
                descriptor, 'check', common.ORACLE_CHECKERS)), root)['listed_paths']
            if common.inputs_digest(root, listed) != descriptor['work']['check']['inputs_sha256']:
                raise ValueError(f'{name}: the inputs changed during the capture')
            common.require_unchanged(descriptor)
        report['complete'] = all(not config['failures'] for config in report['configurations'])
    finally:
        report['finished'] = datetime.now(timezone.utc).strftime('%Y-%m-%dT%H:%M:%SZ')
        report['load_after'] = load_average()
        path.write_text(json.dumps(report, indent=1) + '\n')
    return report


# ---------------------------------------------------------------- reader

def read_capture(path, scenario, mode):
    """The measurement of one workload (check-<scenario> or emit-<scenario>)
    from a complete capture; perf.py writes it as a run file."""
    path = Path(path)
    report = json.loads((path / 'capture.json' if path.is_dir() else path).read_text())
    if report.get('format') != FORMAT or report.get('complete') is not True:
        raise ValueError('a run needs a complete capture (every configuration matched the oracle)')
    if report.get('dirty'):
        raise ValueError('the capture measured a tsrust built from uncommitted sources')
    if scenario not in report['scenarios'] or mode not in report['modes']:
        raise ValueError(f'the capture did not measure {mode}-{scenario}')
    configs = {config['checkers']: config for config in report['configurations']
               if config['scenario'] == scenario and config['mode'] == mode}
    if set(configs) != set(common.CHECKERS):
        raise ValueError(f'{mode}-{scenario}: the capture lacks a checker count')
    ratios, samples, summary, loads = {}, {'go': {}, 'rust': {}}, {}, {}
    for checkers, config in sorted(configs.items()):
        pairs = config['pairs']
        if config['failures'] or len(pairs) not in PAIRS:
            raise ValueError(f'{mode}-{scenario} {checkers}: incomplete configuration')
        for index, pair in enumerate(pairs):
            if pair['index'] != index or pair['order'] != (['go', 'rust'] if index % 2 == 0 else ['rust', 'go']):
                raise ValueError(f'{mode}-{scenario} {checkers}: pairs out of order')
        recomputed = summaries(pairs)
        for metric in ('elapsed', 'peak_rss'):
            ratios[f'{metric}_{checkers}'] = recomputed[metric]['ratio']
            summary[f'{metric}_{checkers}'] = recomputed[metric]
        for runtime in ('go', 'rust'):
            for field in ('wall_ns', 'peak_rss_bytes', 'operation_ns', 'check_ns', 'user_ns', 'system_ns',
                          'effective_checkers'):
                samples[runtime][f'{field}_{checkers}'] = [pair[runtime][field] for pair in pairs]
        loads[str(checkers)] = {'before': config['load_before'], 'after': config['load_after'],
                                'other_cpu_before': config.get('other_cpu_before')}
    work = report['scenarios'][scenario]['work'][mode]
    busy = any((load['other_cpu_before'] or {}).get('percent', 0) > BUSY_OTHER_CPU for load in loads.values())
    metadata = {'scenario': scenario, 'mode': mode, 'source': report['scenarios'][scenario]['source'],
                'project': report['scenarios'][scenario]['project'],
                'arguments': report['scenarios'][scenario]['modes'][mode], 'work': work,
                'binaries': report['binaries'], 'pin': report['pin'], 'load': loads, 'host_busy': busy,
                'host_busy_rule': f'other processes above {BUSY_OTHER_CPU:.0f}% of one CPU before a configuration',
                'sampling': f'one warm-up pair, then {PAIRS[0]} alternating pairs, once extended to {PAIRS[1]}',
                'elapsed': 'process launch to exit; operation_ns is the binary\'s own Total time'}
    return {'ratios': ratios, 'samples': samples, 'summaries': summary, 'metadata': metadata,
            'host': report['host'], 'revision': report['revision'], 'recorded_at': report['finished']}


def main():
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument('command', choices=('build', 'preflight', 'run'))
    parser.add_argument('--scenario', action='append', choices=common.NAMES, help='default: all five')
    parser.add_argument('--mode', action='append', choices=common.MODES, help='default: both')
    parser.add_argument('--pairs', type=int, default=PAIRS[0], choices=PAIRS)
    parser.add_argument('--output', type=Path, help='run: an empty capture directory')
    args = parser.parse_args()
    names, modes = tuple(args.scenario or common.NAMES), tuple(args.mode or common.MODES)
    try:
        if args.command == 'build':
            for runtime, path in common.build().items():
                print(runtime, json.dumps(common.identity(path)))
        elif args.command == 'preflight':
            report = preflight(names, modes, common.binaries())
            print(f"preflight {'passed' if report['passed'] else 'FAILED'}; "
                  f"a run of seven pairs takes about {report['estimate_seconds'] / 60:.0f} minutes")
            if not report['passed']:
                sys.exit(1)
        else:
            if args.output is None:
                parser.error('run needs --output')
            report = capture(names, modes, args.output, args.pairs)
            print(f"capture {'complete' if report['complete'] else 'INCOMPLETE'}: {args.output / 'capture.json'}")
            if not report['complete']:
                sys.exit(1)
    except (OSError, ValueError, subprocess.SubprocessError) as error:
        sys.exit(f'capture.py {args.command}: {error}')


if __name__ == '__main__':
    main()
