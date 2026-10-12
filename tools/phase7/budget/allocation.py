#!/usr/bin/env python3
"""Separate native allocation accounting over an authenticated ordinary capture.

Go HeapAlloc and Rust live_usable_bytes count live allocated blocks, including
allocator rounding. Requested Rust layouts remain separate raw diagnostics.
This tool never supplies elapsed, RSS or executable-size acceptance metrics.
"""
from __future__ import annotations

import argparse
from datetime import datetime, timezone
import math
from pathlib import Path
import shutil
import sys

ROOT = Path(__file__).resolve().parents[3]
sys.path.insert(0, str(ROOT))
from tools.phase7.budget import measure, probe, stream  # noqa: E402
from s07_benchmark_stats import ratio_summary  # noqa: E402

PAIRS = 7
AUTHORITIES = {
    'go': 'runtime.MemStats.HeapAlloc after runtime.GC; rounded GC heap bytes',
    'rust': 'mimalloc-0.1.48 mi_usable_size for live allocations; rounded allocated blocks',
}
TRAFFIC_AUTHORITIES = {
    'go': 'runtime.MemStats.TotalAlloc; cumulative rounded GC heap bytes',
    'rust': 'mimalloc-0.1.48 cumulative mi_usable_size per successful alloc/realloc; logical rounded traffic',
}


def instrumented_identity(binary, build):
    manifest = probe.decode(build.read_bytes())
    if not isinstance(manifest, dict):
        raise ValueError('instrumented build manifest must be an object')
    command = manifest.get('build_command')
    if (manifest.get('mode') != 'phase7-budget' or manifest.get('measurement_contract') != 2
            or type(manifest.get('measurement_contract')) is not int
            or manifest.get('performance_eligible') is not False
            or manifest.get('allocation_instrumented') is not True
            or manifest.get('allocator') != measure.ALLOCATOR
            or manifest.get('source_guard') is not True
            or manifest.get('source_changes_during_build') != []
            or type(manifest.get('build_exit')) is not int or manifest['build_exit'] != 0
            or not manifest.get('sources')
            or manifest.get('profile') != {'name': 'release', 'panic': 'unwind', 'lto': 'fat', 'codegen_units': 1}
            or type(manifest['profile']['codegen_units']) is not int
            or not isinstance(command, list) or command[:2] != ['cargo', 'build']
            or '--release' not in command
            or not measure.command_pair(command, '--manifest-path', 'tools/phase7/budget/rust/Cargo.toml')
            or not measure.command_pair(command, '--features', 'allocation-metrics')):
        raise ValueError('allocation accounting requires the guarded instrumented mimalloc release consumer')
    executable = probe.identity(binary)
    measure.bound_asset(binary, {'bytes': manifest.get('size'), 'sha256': manifest.get('sha256')})
    return {'runtime': 'native', 'engine': 'rust', 'executable': executable['path'],
            'manifest': probe.identity(build), 'files': [probe.identity(build), executable],
            'environment': {'GOMEMLIMIT': None, 'GOEXPERIMENT': None, 'NODE_OPTIONS': None},
            'sources': manifest['sources'], 'pin': manifest.get('pin'),
            'revision': measure.artifact_revision(manifest), 'dirty': manifest.get('dirty'),
            'allocation_instrumented': True}


def metrics(checkpoint, engine):
    probe.validate_checkpoint(checkpoint, engine)
    if engine == 'rust' and (checkpoint['allocation_instrumented'] != 'true'
                             or checkpoint.get('allocator') != measure.ALLOCATOR):
        raise ValueError('Rust allocation capture requires instrumented mimalloc')
    if engine == 'rust' and not all(name in checkpoint for name in
            ('live_usable_bytes', 'total_usable_bytes', 'peak_usable_bytes')):
        raise ValueError('allocation acceptance requires allocator usable bytes; requested-only captures are diagnostic')
    retained = checkpoint['heap_alloc' if engine == 'go' else 'live_usable_bytes']
    traffic = checkpoint['total_alloc' if engine == 'go' else 'total_usable_bytes']
    probe.decimal(retained, engine + ' retained allocation bytes', positive=True)
    probe.decimal(traffic, engine + ' total allocation traffic', positive=True)
    if int(traffic) < int(retained):
        raise ValueError('total allocation traffic is smaller than live allocation bytes')
    if engine == 'rust':
        for key in ('tracked_owners', 'tracked_allocations', 'allocation_events', 'peak_requested_bytes'):
            probe.decimal(checkpoint[key], key, positive=True)
        for prefix in ('live', 'total', 'peak'):
            requested = checkpoint[prefix + '_requested_bytes']
            usable = checkpoint[prefix + '_usable_bytes']
            probe.decimal(requested, prefix + ' requested bytes', positive=True)
            probe.decimal(usable, prefix + ' usable bytes', positive=True)
            if int(usable) < int(requested):
                raise ValueError('usable allocation bytes are smaller than requested bytes')
        for authority in ('requested', 'usable'):
            if int(checkpoint['peak_' + authority + '_bytes']) < int(checkpoint['live_' + authority + '_bytes']):
                raise ValueError('allocation peak is smaller than live bytes')
        if int(checkpoint['allocation_events']) < int(checkpoint['deallocation_events']):
            raise ValueError('more deallocations than successful allocations')
    return {'retained_bytes': int(retained), 'total_allocation_traffic_bytes': int(traffic)}


def load_ordinary_reference(directory, scenario=None, mode=None):
    directory = directory.resolve(strict=True)
    report = probe.decode((directory / 'measurement.json').read_bytes())
    binding = report.get('scenario_acceptance_binding')
    if not isinstance(binding, dict):
        raise ValueError('allocation reference requires an ordinary selected native capture')
    selected = binding['selection']
    scenario = selected['scenario'] if scenario is None else scenario
    mode = selected['mode'] if mode is None else mode
    measure.read_capture(directory, scenario, mode, 'native', require_qualification=False)
    archive = stream.Archive(Path(report['archive']['metadata']), Path(report['archive']['contents']),
                             directory / 'input.json')
    observations = {engine: probe.read_observation(directory / ('warmup-' + engine)) for engine in ('go', 'rust')}
    return report, archive, observations, probe.identity(directory / 'measurement.json')


def same_asset(left, right):
    return all(left[key] == right[key] for key in ('bytes', 'sha256'))


def pair_identity(identities, reference, ordinary_rust_build):
    go, rust = identities['go'], identities['rust']
    reference_go = reference['artifacts']['go']
    if not same_asset(probe.identity(go['executable']), probe.identity(reference_go['executable'])):
        raise ValueError('allocation Go executable differs from ordinary reference')
    if not same_asset(go['manifest'], reference_go['manifest']):
        raise ValueError('allocation Go manifest differs from ordinary reference')
    if not same_asset(probe.identity(ordinary_rust_build), reference['artifacts']['rust']['manifest']):
        raise ValueError('archived ordinary Rust build manifest changed')
    ordinary_rust = probe.decode(ordinary_rust_build.read_bytes())
    if rust['sources'] != ordinary_rust['sources'] or rust['pin'] != reference['pin']:
        raise ValueError('instrumented Rust source/pin differs from the ordinary reference')


def validate_work(reference, observation, engine, binding):
    probe.validate_observation(observation, 'check' if binding['selection']['mode'] == 'check' else 'check_emit')
    baseline = reference[engine]
    measure.checking_work(observation, binding, int(baseline['types']))
    return measure.work_comparison(reference['rust'], observation, engine, binding)


def run(reference_directory, identities, output, timeout):
    reference, archive, observations, reference_id = load_ordinary_reference(reference_directory)
    pair_identity(identities, reference, reference_directory / 'rust-build.json')
    if type(timeout) not in (int, float) or not math.isfinite(timeout) or timeout <= 0:
        raise ValueError('allocation capture requires a finite positive timeout')
    measure.require_output(archive, output)
    output.mkdir(parents=True)
    for engine, identity in identities.items():
        shutil.copyfile(identity['manifest']['path'], output / (engine + '-build.json'))
    controller_paths = [Path(__file__), Path(measure.__file__), Path(probe.__file__), Path(stream.__file__),
                        ROOT / 'tools/phase7/budget/snapshot.py', ROOT / 'tools/phase7/bench/common.py',
                        ROOT / 'scripts/s07_benchmark_stats.py']
    controllers = [probe.identity(path) for path in controller_paths]
    helpers = measure.archive_helpers(output, controllers, identities)
    report = {'schema': 1, 'kind': 'native-allocation-accounting', 'measurement_contract': 2,
              'started_utc': datetime.now(timezone.utc).isoformat(), 'complete': False,
              'revision': measure.paired_revision(identities),
              'performance_acceptance': False, 'allocation_eligible': False,
              'scope': 'separate allocation accounting; no elapsed/RSS/size acceptance',
              'ordinary_reference': reference_id, 'archive': archive.identity,
              'scenario': reference['scenario_acceptance_binding']['selection']['scenario'],
              'mode': reference['scenario_acceptance_binding']['selection']['mode'],
              'authorities': AUTHORITIES, 'traffic_authorities': TRAFFIC_AUTHORITIES,
              'artifacts': identities, 'controllers': controllers, 'helper_manifest': 'helper-manifest.json',
              'pairs': [], 'failures': []}

    def save():
        probe.write(output / 'allocation.json', report)

    def guard():
        if probe.identity(reference_id['path']) != reference_id:
            raise ValueError('ordinary reference changed during allocation capture')
        measure.verify_contents(archive)
        for identity in identities.values():
            measure.unchanged(identity)
        for item in controllers:
            if probe.identity(item['path']) != item:
                raise ValueError('allocation controller changed during capture')
        for item in helpers:
            if not same_asset(probe.identity(output / item['archived']), item):
                raise ValueError('archived allocation controller changed during capture')

    save()
    try:
        for index in range(PAIRS):
            order = ['go', 'rust'] if index % 2 == 0 else ['rust', 'go']
            pair = {'index': index, 'order': order}
            report['pairs'].append(pair)
            for engine in order:
                guard()
                identity = identities[engine]
                directory = output / f'pair-{index:02d}-{engine}'
                with measure.launch_environment(identity['environment']):
                    captured = probe.capture(Path(identity['executable']), archive.input, directory, timeout,
                                             backend=engine, preparation=archive.preparation(), protocol_version=2)
                if captured.get('complete') is not True or captured.get('exit') != 0:
                    raise ValueError('allocation child did not complete')
                row = {'record': str(directory.relative_to(output) / 'record.json'),
                       'raw_files': [dict(probe.identity(path), file=path.name)
                                     for path in sorted(directory.iterdir()) if path.is_file()],
                       'metrics': metrics(captured['checkpoint'], engine),
                       'work': validate_work(observations, captured['observation'], engine,
                                             reference['scenario_acceptance_binding'])}
                pair[engine] = row
                guard()
                save()
        report['summary'] = {key: ratio_summary([pair['go']['metrics'][key] for pair in report['pairs']],
                                                [pair['rust']['metrics'][key] for pair in report['pairs']],
                                                accepted_counts=(PAIRS,))
                             for key in ('retained_bytes', 'total_allocation_traffic_bytes')}
        report.update(complete=True, allocation_eligible=True, finished_utc=datetime.now(timezone.utc).isoformat())
    except Exception as error:
        report['failures'].append(str(error))
    finally:
        save()
    return report


def read_capture(directory, scenario, mode, *, ordinary_reference):
    directory = directory.resolve(strict=True)
    report = probe.decode((directory / 'allocation.json').read_bytes())
    if (report.get('schema') != 1 or report.get('kind') != 'native-allocation-accounting'
            or report.get('measurement_contract') != 2 or report.get('complete') is not True
            or report.get('allocation_eligible') is not True or report.get('performance_acceptance') is not False
            or report.get('authorities') != AUTHORITIES or report.get('traffic_authorities') != TRAFFIC_AUTHORITIES
            or report.get('scenario') != scenario or report.get('mode') != mode):
        raise ValueError('requires a complete separate native allocation capture for the selected operation')
    reference, archive, observations, reference_id = load_ordinary_reference(ordinary_reference, scenario, mode)
    if reference_id != report['ordinary_reference'] or archive.identity != report['archive']:
        raise ValueError('allocation capture is not bound to this ordinary reference and frozen archive')
    identities = report['artifacts']
    for engine in ('go', 'rust'):
        build = directory / (engine + '-build.json')
        if not same_asset(probe.identity(build), identities[engine]['manifest']):
            raise ValueError('allocation build manifest changed')
    # The ordinary reader already authenticates Go's immutable executable and
    # archived GC implementation. Do not require today's source files to match.
    restored = {'go': identities['go'],
                'rust': instrumented_identity(Path(identities['rust']['executable']), directory / 'rust-build.json')}
    pair_identity(restored, reference, ordinary_reference / 'rust-build.json')
    if report.get('revision') != measure.paired_revision(restored):
        raise ValueError('allocation revision differs from measured artifacts')
    helpers = probe.decode((directory / 'helper-manifest.json').read_bytes())
    for helper in helpers:
        if not same_asset(probe.identity(measure.captured_file(directory, helper['archived'])), helper):
            raise ValueError('allocation helper archive changed')
    pairs = report['pairs']
    if len(pairs) != PAIRS:
        raise ValueError('allocation capture requires seven complete alternating pairs')
    samples = {'go': [], 'rust': []}
    traffic = {'go': [], 'rust': []}
    for index, pair in enumerate(pairs):
        order = ['go', 'rust'] if index % 2 == 0 else ['rust', 'go']
        if type(pair.get('index')) is not int or pair['index'] != index or pair.get('order') != order:
            raise ValueError('allocation pair order differs')
        for engine in order:
            saved = pair[engine]
            expected = f'pair-{index:02d}-{engine}/record.json'
            if saved['record'] != expected:
                raise ValueError('allocation record path/order differs')
            sample = measure.captured_file(directory, expected).parent
            files = saved['raw_files']
            if len({item['file'] for item in files}) != len(files) or not {'record.json', 'responses.ndjson.gz', 'stderr'} <= {item['file'] for item in files}:
                raise ValueError('allocation raw files are missing or duplicated')
            for item in files:
                if Path(item['file']).name != item['file'] or not same_asset(probe.identity(measure.captured_file(sample, item['file'])), item):
                    raise ValueError('allocation raw sample changed')
            raw = probe.decode((sample / 'record.json').read_bytes())
            if raw.get('complete') is not True or raw.get('exit') != 0 or raw.get('observation_record', {}).get('format') != 'rows-v2':
                raise ValueError('allocation child lacks complete paged observations')
            if not same_asset(raw['binary'], probe.identity(restored[engine]['executable'])):
                raise ValueError('allocation child did not execute the bound artifact')
            observed = probe.read_observation(sample)
            validate_work(observations, observed, engine, reference['scenario_acceptance_binding'])
            actual = metrics(raw['checkpoint'], engine)
            if actual != saved['metrics']:
                raise ValueError('saved allocation metric differs from raw checkpoint')
            commands = [row['command'] for row in raw['commands']]
            if any(commands.count(name) != 1 for name in ('prepare_begin', 'prepare_end', 'execute', 'checkpoint', 'observe_begin', 'release')) or not commands.index('prepare_begin') < commands.index('prepare_end') < commands.index('execute') < commands.index('checkpoint') < commands.index('observe_begin') < commands.index('release'):
                raise ValueError('allocation capture violates retained checkpoint schedule')
            samples[engine].append(actual['retained_bytes'])
            traffic[engine].append(actual['total_allocation_traffic_bytes'])
    measure.verify_contents(archive)
    summary = ratio_summary(samples['go'], samples['rust'], accepted_counts=(PAIRS,))
    total = ratio_summary(traffic['go'], traffic['rust'], accepted_counts=(PAIRS,))
    if report['summary'] != {'retained_bytes': summary, 'total_allocation_traffic_bytes': total}:
        raise ValueError('saved allocation summary differs from raw checkpoints')
    return {'revision': report.get('revision'), 'ratio': summary['ratio'], 'samples': samples, 'authorities': AUTHORITIES,
            'summary': summary, 'traffic': {'samples': traffic, 'authorities': TRAFFIC_AUTHORITIES, 'summary': total},
            'ordinary_reference': reference_id, 'capture': probe.identity(directory / 'allocation.json')}


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    for name in ('go', 'rust', 'go-build', 'rust-build', 'reference', 'output'):
        parser.add_argument('--' + name, type=Path, required=True)
    parser.add_argument('--timeout', type=float, default=180)
    args = parser.parse_args()
    try:
        identities = {'go': measure.build_identity('native', 'go', args.go, args.go_build),
                      'rust': instrumented_identity(args.rust, args.rust_build)}
        report = run(args.reference, identities, args.output.resolve(), args.timeout)
        print(args.output.resolve() / 'allocation.json')
        return 0 if report['complete'] else 2
    except Exception as error:
        print(error, file=sys.stderr)
        return 2


if __name__ == '__main__':
    raise SystemExit(main())
