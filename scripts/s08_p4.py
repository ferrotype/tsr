#!/usr/bin/env python3
"""Run/replay the entire frozen P4 inventory; never emits E2 pass metrics.

Each variant has its own process, deadline, raw output and atomic completion
record. Resume is tied to the same binary, sources, selected inventory and
options. Unsupported phases remain failures; informational rows stay separate.
"""
import argparse
from collections import Counter, defaultdict
import math
from pathlib import Path
import shutil
import subprocess
import sys

from s04_common import strict_json_loads
from s07_program import validate_requests
from s08_oracle import ROOT, digest
from s07_subset import json_bytes


def canonical(value):
    # S07 options.paths and config_raw are semantic ordered maps. Preserve
    # their source order in both fingerprints and executable requests.
    return json_bytes(value)[:-1]

PHASES = ('config', 'program', 'syntactic', 'semantic', 'global', 'declaration', 'suggestion')
SOURCE_PATTERNS = ('crates/**/*.rs', 'crates/**/Cargo.toml', 'Cargo.toml', 'Cargo.lock',
                   'tools/s08/p4/**', 'tools/s07/program/*.rs', 'scripts/s08_p4.py',
                   'scripts/s04_common.py', 'scripts/s07_program.py', 'scripts/s08_oracle.py',
                   'scripts/s07_subset.py', 'data/upstream.json')
MANIFEST = ROOT / 'data/s08/baseline-requests.json'


def read(path):
    return strict_json_loads(Path(path).read_bytes())


def write_new(path, value):
    with Path(path).open('xb') as stream:
        stream.write(canonical(value) + b'\n')


def atomic(path, value):
    path = Path(path)
    temporary = path.with_suffix(path.suffix + '.partial')
    temporary.write_bytes(canonical(value) + b'\n')
    temporary.replace(path)


def sources():
    paths = {p for pattern in SOURCE_PATTERNS for p in ROOT.glob(pattern) if p.is_file()}
    return {str(p.relative_to(ROOT)): digest(p.read_bytes()) for p in sorted(paths)}


def inventory(loading_path, tier='acceptance', cases=()):
    manifest = read(MANIFEST)
    loading = read(loading_path)
    validate_requests(loading)
    frozen = manifest['requests']
    if [r['id'] for r in loading] != [r['id'] for r in frozen]:
        raise ValueError('loading inventory differs from complete frozen manifest')
    counts = Counter(r['acceptance_tier'] for r in frozen)
    if dict(counts) != {'acceptance': 9369, 'informational': 1359}:
        raise ValueError('frozen acceptance partition changed; explicit review required')
    wanted = set(cases)
    if len(wanted) != len(cases) or wanted - {r['id'] for r in frozen}:
        raise ValueError('duplicate or unknown --case identity')
    result = []
    for phase, request in zip(frozen, loading, strict=True):
        if digest(canonical(request) + b'\n') != phase['loading_request_sha256']:
            raise ValueError('loading request changed: ' + phase['id'])
        if tier != 'all' and phase['acceptance_tier'] != tier:
            continue
        if wanted and phase['id'] not in wanted:
            continue
        result.append({k: phase[k] for k in ('id', 'acceptance_tier', 'diagnostic_phases', 'type_baseline_requested')})
        result[-1]['loading'] = request
    if wanted != {r['id'] for r in result} and wanted:
        raise ValueError('--case identity is outside requested tier')
    return result


def state(value):
    if not isinstance(value, dict) or value.get('state') not in ('executed', 'failed', 'not_implemented', 'not_requested'):
        raise ValueError('unclassified operation result')
    if value['state'] == 'failed' and not ('files' in value or
        isinstance(value.get('class'), str) and isinstance(value.get('reason'), str) and value['reason']):
        raise ValueError('failure lacks reason/class or per-file results')
    if value['state'] == 'not_implemented' and not value.get('reason'):
        raise ValueError('unimplemented operation lacks reason')


def validate_row(request, row):
    if not isinstance(row, dict) or row.get('version') != 1 or row.get('id') != request['id'] or row.get('acceptance_tier') != request['acceptance_tier']:
        raise ValueError('missing/extra/reordered observation or changed acceptance tier')
    if 'fatal' in row:
        if set(row) != {'version', 'id', 'acceptance_tier', 'fatal'}:
            raise ValueError('fatal observation carries a fabricated partial success')
        state(row['fatal'])
        if row['fatal']['state'] != 'failed':
            raise ValueError('fatal observation is not a failure')
        return row
    expected = {'version', 'id', 'acceptance_tier', 'load', 'phases', 'type_symbol_baselines'}
    if row.get('load', {}).get('state') == 'executed':
        expected.add('bind_diagnostics')
    if set(row) != expected or set(row['phases']) != set(request['diagnostic_phases']):
        raise ValueError('missing/extra requested phase or result field')
    state(row['load'])
    state(row['type_symbol_baselines'])
    if not request['type_baseline_requested']:
        if row['type_symbol_baselines'] != {'state': 'not_requested'}:
            raise ValueError('unrequested baseline was executed')
    elif row['type_symbol_baselines']['state'] == 'not_requested':
        raise ValueError('requested baseline silently dropped')
    for name, phase in row['phases'].items():
        state(phase)
        if phase['state'] == 'not_requested':
            raise ValueError('requested phase silently dropped: ' + name)
        if row['load']['state'] != 'executed' and phase['state'] == 'executed':
            raise ValueError('phase executed without loaded program')
    if 'bind_diagnostics' in row:
        state(row['bind_diagnostics'])
        graph = row['load'].get('graph')
        if not isinstance(graph, dict) or graph.get('ID') != request['id'] or not isinstance(graph.get('Files'), list):
            raise ValueError('load lacks observed file identity')
        semantic = row['phases']['semantic']
        if 'files' in semantic:
            names = [f['Name'].encode().hex() for f in graph['Files']]
            if [f.get('file_hex') for f in semantic['files']] != names:
                raise ValueError('semantic file inventory missing/extra/reordered')
            for file in semantic['files']:
                if set(file) != {'file_hex', 'result'}:
                    raise ValueError('malformed semantic file result')
                state(file['result'])
                if file['result']['state'] == 'executed' and file['result'].get('selection') not in ('checked', 'native_skip'):
                    raise ValueError('semantic success lacks native selection')
            executed = all(f['result']['state'] == 'executed' for f in semantic['files'])
            if (semantic['state'] == 'executed') != executed:
                raise ValueError('semantic completion hides a failed source')
    return row


def fatal(request, kind, reason):
    return {'version': 1, 'id': request['id'], 'acceptance_tier': request['acceptance_tier'],
            'fatal': {'state': 'failed', 'class': kind, 'reason': reason}}


def build(directory):
    directory.mkdir(parents=True, exist_ok=False)
    before = sources()
    snapshot = directory / 'source-snapshot'
    for name, expected in before.items():
        content = (ROOT / name).read_bytes()
        if digest(content) != expected:
            raise ValueError('sources changed while snapshotting the adapter build')
        target = snapshot / name
        target.parent.mkdir(parents=True, exist_ok=True)
        target.write_bytes(content)
    command = ['cargo', 'build', '--locked', '-p', 'ts_compiler', '--example', 'p4_inventory', '--message-format=json']
    with (directory / 'build.stdout').open('wb') as out, (directory / 'build.stderr').open('wb') as err:
        result = subprocess.run(command, cwd=ROOT, stdout=out, stderr=err, check=False)
    if result.returncode:
        raise ValueError('P4 adapter build failed; raw output retained in ' + str(directory))
    binaries = set()
    for line in (directory / 'build.stdout').read_bytes().splitlines():
        event = strict_json_loads(line)
        if (event.get('reason') == 'compiler-artifact' and event.get('target', {}).get('name') == 'p4_inventory'
                and event['target']['kind'] == ['example'] and event.get('executable')):
            binaries.add(event['executable'])
    if len(binaries) != 1 or sources() != before:
        raise ValueError('ambiguous executable or sources changed during build')
    executable = Path(next(iter(binaries)))
    record = {'version': 1, 'command': command, 'binary': str(executable.resolve()),
              'binary_sha256': digest(executable.read_bytes()), 'sources': before,
              'source_snapshot': str(snapshot.resolve())}
    write_new(directory / 'build.json', record)
    return record


def verify_source_snapshot(directory, expected):
    directory = Path(directory)
    actual = {str(path.relative_to(directory)): digest(path.read_bytes())
              for path in directory.rglob('*') if path.is_file()}
    if actual != expected:
        raise ValueError('adapter build source snapshot differs from its fingerprint')


def summarize(requests, rows):
    tiers = {}
    for tier in ('acceptance', 'informational'):
        selected = [(request, row) for request, row in zip(requests, rows, strict=True) if request['acceptance_tier'] == tier]
        states = Counter()
        buckets = defaultdict(list)
        completed = 0
        for request, row in selected:
            if 'fatal' in row:
                f = row['fatal']
                buckets[('process', f['class'], f['reason'])].append(request['id'])
                continue
            okay = row['load']['state'] == 'executed'
            operations = [('load', row['load']), *row['phases'].items(), ('type_symbol_baselines', row['type_symbol_baselines'])]
            for name, value in operations:
                states[(name, value['state'])] += 1
                if name == 'semantic' and 'files' in value:
                    errors = [f['result'] for f in value['files'] if f['result']['state'] != 'executed']
                else:
                    errors = [value] if value['state'] in ('failed', 'not_implemented') else []
                for error in errors:
                    buckets[(name, error.get('class', error['state']), error.get('reason', 'unclassified'))].append(request['id'])
                if name not in ('load', 'type_symbol_baselines') and value['state'] != 'executed':
                    okay = False
            completed += okay
        tiers[tier] = {'variants': len(selected), 'all_requested_diagnostic_phases_completed': completed,
                       'states': [{'operation': k[0], 'state': k[1], 'variants': v} for k, v in sorted(states.items())],
                       'failure_buckets': [{'operation': k[0], 'class': k[1], 'reason': k[2],
                         'occurrences': len(v), 'variants': list(dict.fromkeys(v))} for k, v in sorted(buckets.items(), key=lambda kv: (-len(kv[1]), kv[0]))]}
    return {'version': 1, 'scope': 'P4 execution/failure inventory; no native diagnostic or baseline parity claim; not E2 evidence',
            'requested': len(requests), 'observed': len(rows), 'tiers': tiers}


def replay(directory, allow_partial=False):
    metadata = read(directory / 'capture.json')
    if 'source_snapshot' in metadata.get('build', {}):
        verify_source_snapshot(directory / 'source-snapshot', metadata['build']['sources'])
    requests_raw = (directory / 'requests.json').read_bytes()
    if digest(requests_raw) != metadata['requests_sha256']:
        raise ValueError('capture request inventory changed')
    requests = strict_json_loads(requests_raw)
    rows = []
    for entry in (directory / 'cases').iterdir():
        if entry.name.isdigit() and (entry / 'result.json').exists() and (len(entry.name) != 5 or int(entry.name) >= len(requests)):
            raise ValueError('extra completion record outside request inventory')
    for index, request in enumerate(requests):
        path = directory / 'cases' / f'{index:05}' / 'result.json'
        if not path.exists():
            if allow_partial:
                break
            raise ValueError('capture incomplete at ' + request['id'])
        envelope = read(path)
        if envelope.get('request_sha256') != digest(canonical(request) + b'\n') or envelope.get('capture_sha256') != digest(canonical(metadata) + b'\n'):
            raise ValueError('case completion belongs to different input/binary capture')
        for name, expected in envelope['artifacts'].items():
            if name not in ('stdout', 'stderr', 'observation.json') or digest((path.parent / name).read_bytes()) != expected:
                raise ValueError('case raw artifact changed')
        rows.append(validate_row(request, envelope['row']))
    if any((directory / 'cases' / f'{i:05}' / 'result.json').exists() for i in range(len(rows), len(requests))):
        raise ValueError('noncontiguous completion records')
    report = summarize(requests[:len(rows)], rows)
    report.update(requested=len(requests), complete=len(rows) == len(requests), capture_sha256=digest(canonical(metadata) + b'\n'))
    return requests, rows, report


def run(args):
    requests = inventory(args.loading_requests, args.tier, args.case)
    build_record = read(args.build_record)
    directory = args.output.resolve()
    authenticated_resume = args.resume and directory.exists() and 'source_snapshot' in build_record
    if build_record['sources'] != sources() and not authenticated_resume:
        raise ValueError('source tree differs from the adapter build; rebuild before capture')
    binary = Path(build_record['binary'])
    if digest(binary.read_bytes()) != build_record['binary_sha256']:
        raise ValueError('adapter binary changed since build')
    if 'source_snapshot' in build_record:
        verify_source_snapshot(build_record['source_snapshot'], build_record['sources'])
    metadata = {'version': 1, 'requests_sha256': digest(canonical(requests) + b'\n'),
                'manifest_sha256': digest(MANIFEST.read_bytes()), 'build': build_record, 'timeout_seconds': args.timeout,
                'tier': args.tier, 'cases': args.case, 'semantics': 'diagnostic execution only; no E2 or performance gate'}
    if directory.exists():
        if not args.resume or read(directory / 'capture.json') != metadata:
            raise ValueError('existing capture requires --resume with identical source/binary/input/options')
        _, rows, _ = replay(directory, True)
        start = len(rows)
    else:
        directory.mkdir(parents=True)
        (directory / 'cases').mkdir()
        write_new(directory / 'requests.json', requests)
        write_new(directory / 'capture.json', metadata)
        shutil.copy2(binary, directory / 'executable')
        if 'source_snapshot' in build_record:
            shutil.copytree(build_record['source_snapshot'], directory / 'source-snapshot')
        start = 0
    captured_binary = directory / 'executable'
    if digest(captured_binary.read_bytes()) != build_record['binary_sha256']:
        raise ValueError('captured executable changed')
    for index in range(start, len(requests)):
        request = requests[index]
        case_dir = directory / 'cases' / f'{index:05}'
        # A killed producer may have raw files but no committed result. Preserve
        # that attempt before retrying; never overwrite completed observations.
        if case_dir.exists():
            attempt = 0
            while case_dir.with_name(case_dir.name + f'.interrupted-{attempt}').exists(): attempt += 1
            case_dir.rename(case_dir.with_name(case_dir.name + f'.interrupted-{attempt}'))
        case_dir.mkdir()
        write_new(case_dir / 'request.json', request)
        observation = case_dir / 'observation.json'
        with (case_dir / 'stdout').open('wb') as out, (case_dir / 'stderr').open('wb') as err:
            try:
                proc = subprocess.run([str(captured_binary), str(case_dir / 'request.json'), str(observation)],
                                      cwd=ROOT, stdout=out, stderr=err, timeout=args.timeout, check=False)
                row = fatal(request, 'process_exit', str(proc.returncode)) if proc.returncode else validate_row(request, read(observation))
            except subprocess.TimeoutExpired:
                row = fatal(request, 'timeout', f'variant exceeded {args.timeout:g} seconds')
            except (OSError, ValueError, KeyError, TypeError) as error:
                row = fatal(request, 'harness_protocol', str(error))
        artifacts = {name: digest((case_dir / name).read_bytes()) for name in ('stdout', 'stderr', 'observation.json') if (case_dir / name).exists()}
        write_new(case_dir / 'result.json', {'request_sha256': digest(canonical(request) + b'\n'),
                  'capture_sha256': digest(canonical(metadata) + b'\n'), 'artifacts': artifacts, 'row': row})
        if (index + 1) % 100 == 0 or index + 1 == len(requests):
            print(f'P4 inventory {index + 1}/{len(requests)}', file=sys.stderr, flush=True)
    _, _, report = replay(directory)
    report['source_stable'] = sources() == build_record['sources']
    atomic(directory / 'report.json', report)
    print(canonical({k: report[k] for k in ('requested', 'observed', 'complete', 'source_stable')}).decode())


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    commands = parser.add_subparsers(dest='command', required=True)
    b = commands.add_parser('build'); b.add_argument('--output', type=Path, required=True)
    r = commands.add_parser('run')
    r.add_argument('--loading-requests', type=Path, default=ROOT / 'target/s07-subset/review/loading-requests.candidate.json')
    r.add_argument('--build-record', type=Path, required=True)
    r.add_argument('--output', type=Path, required=True)
    r.add_argument('--tier', choices=('acceptance', 'informational', 'all'), default='acceptance')
    r.add_argument('--case', action='append', default=[])
    r.add_argument('--timeout', type=float, default=60)
    r.add_argument('--resume', action='store_true')
    v = commands.add_parser('replay'); v.add_argument('directory', type=Path); v.add_argument('--allow-partial', action='store_true')
    args = parser.parse_args()
    if args.command == 'build': build(args.output.resolve())
    elif args.command == 'run':
        if not math.isfinite(args.timeout) or args.timeout <= 0: raise ValueError('timeout must be positive and finite')
        run(args)
    else:
        _, _, report = replay(args.directory.resolve(), args.allow_partial)
        atomic(args.directory / ('partial-report.json' if args.allow_partial else 'report.json'), report)
        print(canonical({'requested': report['requested'], 'observed': report['observed'], 'complete': report['complete']}).decode())


if __name__ == '__main__':
    try: main()
    except (OSError, ValueError, KeyError, TypeError) as error:
        print('P4 inventory failed: ' + str(error), file=sys.stderr)
        raise SystemExit(1) from error
