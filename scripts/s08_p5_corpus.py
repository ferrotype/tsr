#!/usr/bin/env python3
"""Focused frozen-corpus P5 comparison; diagnostics remain separate from bytes.

Native baseline_inputs come from the original runner, not from expected query
or baseline text. The Rust request never includes expected queries/results.
This development command does not emit E2 metrics or support resumed captures.
"""
import argparse
from collections import Counter
from pathlib import Path
import subprocess

from s08_oracle import ROOT, digest
from s08_p4 import canonical, inventory, read, sources as p4_sources, validate_row, write_new
from s08_queries import action, expected_baseline


def sources():
    result = p4_sources()
    for pattern in ('tools/s08/p5/**', 'scripts/s08_p5*.py', 'scripts/s08_queries.py'):
        for path in ROOT.glob(pattern):
            if path.is_file():
                result[str(path.relative_to(ROOT))] = digest(path.read_bytes())
    return result


def prepare(native, loading):
    report = read(native / 'report.json')
    if report.get('walker_inputs') is not True or report.get('walker_input_encoding') != 'hex-v1' or report['mismatches']:
        raise ValueError('native capture lacks ordered walker inputs or has unresolved contract differences')
    if report['pin'] != read(ROOT / 'data/upstream.json')['pin']:
        raise ValueError('native pin changed')
    for key, name in (('request_sha256', 'requests.json'), ('observation_sha256', 'observations.ndjson')):
        if digest((native / name).read_bytes()) != report[key]:
            raise ValueError('native capture fingerprint differs: ' + name)
    for name, fingerprint in report['source_inputs'].items():
        if digest((native / 'source-snapshot' / name).read_bytes()) != fingerprint:
            raise ValueError('native snapshot differs: ' + name)
    from s04_common import strict_json_loads
    observed = [strict_json_loads(line) for line in (native / 'observations.ndjson').read_bytes().splitlines()]
    ids = [r['id'] for r in read(native / 'requests.json')]
    if [r['id'] for r in observed] != ids:
        raise ValueError('native observation order changed')
    frozen = inventory(loading, tier='all', cases=ids)
    if [r['id'] for r in frozen] != ids:
        raise ValueError('native inputs do not follow the frozen inventory')
    result = []
    for request, row in zip(frozen, observed, strict=True):
        if request['acceptance_tier'] != row['acceptance_tier']:
            raise ValueError('native acceptance tier changed')
        if row['state'] == 'executed' and request['type_baseline_requested']:
            # Preserve duplicate filenames, source bytes, and input/aux order.
            if not isinstance(row['baseline_inputs'], list) or not isinstance(row['baseline_header'], str):
                raise ValueError('missing native walker input metadata')
            if any(set(f) != {'name_hex', 'content_hex'} or not all(isinstance(v, str) and bytes.fromhex(v).hex() == v for v in f.values()) for f in row['baseline_inputs']):
                raise ValueError('malformed native walker input')
            request['baseline_inputs'] = row['baseline_inputs']
            request['baseline_header'] = row['baseline_header']
        result.append((request, row))
    return result


def compare_row(expected, row):
    if 'fatal' in row:
        return {'state': 'failed', 'reason': row['fatal']}
    actual = row['type_symbol_baselines']
    if actual['state'] == 'not_requested':
        if any(expected[k] != {'state': 'disabled'} for k in ('types', 'symbols')):
            raise ValueError('disabled baseline differs from native request')
        return {'state': 'disabled'}
    if actual['state'] != 'executed':
        return {'state': 'failed', 'reason': actual}
    for key in ('types', 'symbols'):
        expected_baseline(actual[key])
    native_files, rust_files = {}, {}
    native_queries = [action(q, native_files) for q in expected['queries']]
    rust_queries = [action(q, rust_files) for q in actual['queries']]
    differences = [key for key in ('types', 'symbols') if expected[key] != actual[key]]
    if native_files != rust_files or native_queries != rust_queries:
        differences.append('queries')
    return {'state': 'different' if differences else 'match', 'differences': differences,
            'native_queries': len(native_queries), 'rust_queries': len(rust_queries)}


def run(native, loading, output, timeout):
    pairs = prepare(native, loading)
    output.mkdir(parents=True, exist_ok=False)
    before = sources()
    command = ['cargo', 'build', '--locked', '-p', 'ts_compiler', '--example', 'p5_inventory', '--message-format=json']
    with (output / 'build.stdout').open('wb') as out, (output / 'build.stderr').open('wb') as err:
        completed = subprocess.run(command, cwd=ROOT, stdout=out, stderr=err, check=False)
    if completed.returncode:
        raise ValueError('P5 build failed; logs retained')
    from s04_common import strict_json_loads
    events = [strict_json_loads(line) for line in (output / 'build.stdout').read_bytes().splitlines()]
    executables = {e['executable'] for e in events if e.get('reason') == 'compiler-artifact' and e.get('target', {}).get('name') == 'p5_inventory' and e.get('executable')}
    if len(executables) != 1 or sources() != before:
        raise ValueError('source changed during build or executable is ambiguous')
    binary = Path(next(iter(executables)))
    fingerprint = digest(binary.read_bytes())
    write_new(output / 'build.json', {'command': command, 'binary': str(binary), 'binary_sha256': fingerprint, 'sources': before})
    results = []
    for index, (request, expected) in enumerate(pairs):
        prefix = output / f'{index:05d}'
        write_new(prefix.with_suffix('.request.json'), request)
        write_new(prefix.with_suffix('.native.json'), expected)
        result = {'id': request['id'], 'acceptance_tier': request['acceptance_tier']}
        if expected['state'] != 'executed':
            result.update(state='native_unavailable', native_state=expected['state'])
        else:
            with prefix.with_suffix('.stdout').open('wb') as out, prefix.with_suffix('.stderr').open('wb') as err:
                try:
                    process = subprocess.run([str(binary), str(prefix.with_suffix('.request.json')), str(prefix.with_suffix('.rust.json'))], cwd=ROOT, stdout=out, stderr=err, timeout=timeout, check=False)
                    if process.returncode:
                        result.update(state='failed', reason=f'process exit {process.returncode}')
                    else:
                        row = read(prefix.with_suffix('.rust.json'))
                        # P5 retains panic query context outside the retired owner.
                        validation = {k:v for k,v in row.items() if 'fatal' not in row or k not in ('queries', 'active_query')}
                        validate_row(request, validation)
                        result.update(compare_row(expected, row))
                except subprocess.TimeoutExpired:
                    result.update(state='failed', reason='timeout')
        results.append(result)
        write_new(prefix.with_suffix('.comparison.json'), result)
    stable = sources() == before and digest(binary.read_bytes()) == fingerprint
    report = {'version': 1, 'scope': 'Focused native corpus walker comparison; no E2 metric',
              'native_report_sha256': digest((native / 'report.json').read_bytes()),
              'source_stable': stable, 'rows': results,
              'counts_by_tier': {tier: dict(Counter(r['state'] for r in results if r['acceptance_tier'] == tier)) for tier in ('acceptance', 'informational')}}
    write_new(output / 'report.json', report)
    print(report['counts_by_tier'])
    if not stable:
        raise ValueError('sources or executable changed during capture')


if __name__ == '__main__':
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--native', type=Path, required=True)
    parser.add_argument('--loading-requests', type=Path, default=ROOT / 'target/s07-subset/review/loading-requests.candidate.json')
    parser.add_argument('--output', type=Path, required=True)
    parser.add_argument('--timeout', type=float, default=60)
    args = parser.parse_args()
    import math
    if not math.isfinite(args.timeout) or args.timeout <= 0:
        parser.error('--timeout must be positive and finite')
    run(args.native.resolve(), args.loading_requests.resolve(), args.output.resolve(), args.timeout)
