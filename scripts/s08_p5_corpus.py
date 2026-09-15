#!/usr/bin/env python3
"""Resumable frozen-corpus P5 comparison; diagnostics remain separate from bytes.

Native baseline_inputs come from the original runner, not from expected query
or baseline text. The Rust request never includes expected queries/results.
Each completed case is replayable; resume uses the captured executable and sources.
This development command does not emit E2 metrics or claim that emit executed.
"""
import argparse
from collections import Counter
from pathlib import Path
import shutil
import sys

import s08_p4 as p4

from s08_oracle import ROOT, digest
from s08_p4 import canonical, inventory, read, sources as p4_sources, write_new
from s04_common import strict_json_loads
from s08_queries import action, expected_baseline


def sources():
    result = p4_sources()
    for pattern in ('tools/s08/p5/**/*', 'scripts/s08_p5*.py', 'scripts/s08_queries.py', 'scripts/s08_baselines.py'):
        for path in ROOT.glob(pattern):
            if path.is_file():
                result[str(path.relative_to(ROOT))] = digest(path.read_bytes())
    return result


def hex_bytes(value):
    if not isinstance(value, str) or bytes.fromhex(value).hex() != value:
        raise ValueError('noncanonical hex bytes')
    return value


def input_metadata(value):
    if not isinstance(value, list):
        raise ValueError('missing native input metadata')
    for item in value:
        if not isinstance(item, dict) or set(item) != {'name_hex', 'content_hex'}:
            raise ValueError('malformed native input file')
        for raw in item.values(): hex_bytes(raw)
    return value


def diagnostics(values):
    if not isinstance(values, list):
        raise ValueError('missing structured diagnostics')
    fields = {'file_hex', 'pos', 'end', 'code', 'category', 'key_hex', 'text_hex', 'source_hex',
              'args_hex', 'chain', 'related', 'unnecessary', 'deprecated', 'skipped_on_no_emit'}
    for value in values:
        if not isinstance(value, dict) or set(value) != fields:
            raise ValueError('incomplete structured diagnostic')
        for key in ('pos', 'end', 'code', 'category'):
            if type(value[key]) is not int: raise ValueError('invalid diagnostic coordinate/code')
        for key in ('unnecessary', 'deprecated', 'skipped_on_no_emit'):
            if type(value[key]) is not bool: raise ValueError('invalid diagnostic flag')
        if value['file_hex'] is not None: hex_bytes(value['file_hex'])
        for key in ('key_hex', 'text_hex', 'source_hex'): hex_bytes(value[key])
        if not isinstance(value['args_hex'], list): raise ValueError('invalid diagnostic arguments')
        for arg in value['args_hex']: hex_bytes(arg)
        diagnostics(value['chain']); diagnostics(value['related'])
    return values


def native_metadata(request, row):
    if row['acceptance_tier'] != request['acceptance_tier']:
        raise ValueError('native acceptance tier changed')
    if row['state'] != 'executed':
        return
    if request['type_baseline_requested']:
        input_metadata(row['baseline_inputs'])
        if not isinstance(row['baseline_header'], str): raise ValueError('missing native walker header')
    input_metadata(row['error_inputs']); input_metadata(row['error_render_inputs'])
    # Native content-mapped selection may omit files, but may not invent or
    # reorder them. Rust receives the unfiltered inputs and owns its selection.
    remaining = iter(row['error_inputs'])
    if any(not any(item == candidate for candidate in remaining) for item in row['error_render_inputs']):
        raise ValueError('native error selection is not an ordered subsequence')
    if type(row['error_pretty']) is not bool: raise ValueError('missing native pretty setting')
    for key in ('error_pre_diagnostics', 'error_post_diagnostics', 'error_diagnostics'):
        diagnostics(row[key])
    expected_baseline(row['errors'])
    if row['errors']['state'] == 'disabled': raise ValueError('error baseline may not be disabled')


def prepare(native, loading):
    report = read(native / 'report.json')
    if (report.get('walker_inputs') is not True or report.get('walker_input_encoding') != 'hex-v1'
            or report.get('error_inputs') is not True or report.get('error_input_encoding') != 'native-hex-v2'):
        raise ValueError('native capture lacks ordered walker/error inputs and byte-preserving diagnostics')
    # A pre/post-emit difference is a comparison result, never an input waiver.
    if any(m['field'] != 'pre_post_diagnostics' for m in report['mismatches']):
        raise ValueError('native capture has unresolved input/closure differences')
    if report['pin'] != read(ROOT / 'data/upstream.json')['pin']:
        raise ValueError('native pin changed')
    for key, name in (('request_sha256', 'requests.json'), ('observation_sha256', 'observations.ndjson')):
        if digest((native / name).read_bytes()) != report[key]:
            raise ValueError('native capture fingerprint differs: ' + name)
    p4.verify_source_snapshot(native / 'source-snapshot', report['source_inputs'])
    observed = [strict_json_loads(line) for line in (native / 'observations.ndjson').read_bytes().splitlines()]
    ids = [r['id'] for r in read(native / 'requests.json')]
    if [r['id'] for r in observed] != ids:
        raise ValueError('native observation order changed')
    frozen = inventory(loading, tier='all', cases=ids)
    if [r['id'] for r in frozen] != ids:
        raise ValueError('native inputs do not follow the frozen inventory')
    result = []
    for request, row in zip(frozen, observed, strict=True):
        native_metadata(request, row)
        request['error_baseline_requested'] = True
        if row['state'] == 'executed':
            request['error_inputs'] = row['error_inputs']
            if request['type_baseline_requested']:
                request['baseline_inputs'] = row['baseline_inputs']
                request['baseline_header'] = row['baseline_header']
        result.append((request, row))
    return result


def phase_complete(phase):
    return phase['state'] == 'executed' and all(phase_complete(f['result']) for f in phase.get('files', []))


def validate_row(request, row):
    if request.get('error_baseline_requested') is not True:
        raise ValueError('P5 error baseline request silently dropped')
    if 'fatal' in row:
        if set(row) - {'version', 'id', 'acceptance_tier', 'fatal', 'queries', 'active_query'}:
            raise ValueError('fatal observation carries fabricated partial success')
        p4.validate_row(request, {k: v for k, v in row.items() if k not in ('queries', 'active_query')})
        # Query context is diagnostic evidence, not a successful walker result.
        if ('queries' in row) != ('active_query' in row) or ('queries' in row and not isinstance(row['queries'], list)):
            raise ValueError('incomplete panic query context')
        return row
    if 'error_baseline' not in row:
        raise ValueError('requested error baseline silently dropped')
    p4.validate_row(request, {k: v for k, v in row.items() if k != 'error_baseline'})
    errors = row['error_baseline']
    p4.state(errors)
    if errors['state'] == 'not_requested': raise ValueError('requested error baseline silently dropped')
    if errors['state'] == 'executed':
        if set(errors) != {'state', 'diagnostics', 'baseline', 'emit', 'pretty', 'inputs'}:
            raise ValueError('incomplete error baseline observation')
        if row['load']['state'] != 'executed' or not all(phase_complete(p) for p in row['phases'].values()):
            raise ValueError('error baseline hides an incomplete diagnostic phase')
        diagnostics(errors['diagnostics'])
        expected_baseline(errors['baseline'])
        if errors['baseline']['state'] == 'disabled' or (errors['baseline']['state'] == 'no_content') != (not errors['diagnostics']):
            raise ValueError('error baseline lost diagnostics or fabricated content')
        if errors['emit'] != 'not_executed': raise ValueError('P5 adapter does not execute emit')
        if type(errors['pretty']) is not bool: raise ValueError('missing observed pretty setting')
        input_metadata(errors['inputs'])
    return row


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


def compare_errors(expected, row):
    if 'fatal' in row: return {'state': 'failed', 'reason': row['fatal']}
    actual = row['error_baseline']
    if actual['state'] != 'executed': return {'state': 'failed', 'reason': actual}
    differences = []
    for label, left, right in (
        ('native_pre_post_diagnostics', expected['error_pre_diagnostics'], expected['error_post_diagnostics']),
        ('pre_diagnostics', expected['error_pre_diagnostics'], actual['diagnostics']),
        ('post_diagnostics', expected['error_post_diagnostics'], actual['diagnostics']),
        ('render_diagnostics', expected['error_diagnostics'], actual['diagnostics']),
        ('inputs', expected['error_render_inputs'], actual['inputs']),
        ('pretty', expected['error_pretty'], actual['pretty']),
        ('bytes', expected['errors'], actual['baseline']),
    ):
        if left != right: differences.append(label)
    return {'state': 'different' if differences else 'match', 'differences': differences,
            'native_diagnostics': len(expected['error_diagnostics']), 'rust_diagnostics': len(actual['diagnostics']),
            'emit': 'not_executed'}


def summarize(requests, rows, expected):
    results = []
    for request, row, native in zip(requests, rows, expected[:len(rows)], strict=True):
        result = {'id': request['id'], 'acceptance_tier': request['acceptance_tier']}
        if native['state'] != 'executed':
            result.update(state='native_unavailable', native_state=native['state'])
        else:
            types = compare_row(native, row)
            errors = compare_errors(native, row)
            state = ('failed' if 'failed' in (types['state'], errors['state']) else
                     'different' if 'different' in (types['state'], errors['state']) else 'match')
            result.update(state=state, type_symbols=types, errors=errors)
        results.append(result)
    return {'version': 2, 'scope': 'Native corpus baseline comparison; emit not executed; no E2 metric',
            'observed': len(rows), 'rows': results,
            'counts_by_tier': {tier: dict(Counter(r['state'] for r in results if r['acceptance_tier'] == tier))
                               for tier in ('acceptance', 'informational')}}


def replay(output, allow_partial=False):
    metadata = read(output / 'capture.json')
    native_report = read(output / 'native-report.json')
    if digest((output / 'native-report.json').read_bytes()) != metadata['native_report_sha256']:
        raise ValueError('native report changed')
    raw = (output / 'native-observations.ndjson').read_bytes()
    if digest(raw) != native_report['observation_sha256']:
        raise ValueError('native observations changed')
    expected = [strict_json_loads(line) for line in raw.splitlines()]
    requests = read(output / 'requests.json')
    if [r['id'] for r in expected] != [r['id'] for r in requests]:
        raise ValueError('native inventory changed')
    for request, row in zip(requests, expected, strict=True): native_metadata(request, row)
    if digest((output / 'executable').read_bytes()) != metadata['build']['binary_sha256']:
        raise ValueError('captured executable changed')
    return p4.replay(output, allow_partial, validator=validate_row,
                     summarizer=lambda reqs, rows: summarize(reqs, rows, expected))


def run(native, loading, output, timeout, resume=False, *, public_type_strings=False, source_fn=None):
    source_fn = source_fn or sources
    if public_type_strings and read(native / 'report.json').get('public_type_strings') is not True:
        raise ValueError('E2 needs a native --public-type-strings capture before building Rust')
    if output.exists():
        if not resume: raise ValueError('existing capture requires --resume')
        metadata = read(output / 'capture.json')
        if metadata['timeout_seconds'] != timeout or digest((native / 'report.json').read_bytes()) != metadata['native_report_sha256']:
            raise ValueError('resume requires identical native capture and timeout')
        requests, completed, _ = replay(output, True)
        if any(r.get('public_type_strings', False) != public_type_strings for r in requests):
            raise ValueError('resume changed public TypeToString observation mode')
        expected = [strict_json_loads(line) for line in (output / 'native-observations.ndjson').read_bytes().splitlines()]
        start = len(completed)
    else:
        pairs = prepare(native, loading)
        requests, expected = map(list, zip(*pairs, strict=True))
        if public_type_strings:
            for request in requests: request['public_type_strings'] = True
        output.mkdir(parents=True)
        (output / 'cases').mkdir()
        record = p4.build(output / 'build', example='p5_inventory', source_fn=source_fn, optimize=True)
        # Keep a single source snapshot in the capture, not a second copy.
        Path(record['source_snapshot']).rename(output / 'source-snapshot')
        record['source_snapshot'] = str(output / 'source-snapshot')
        p4.atomic(output / 'build/build.json', record)
        metadata = {'version': 2, 'requests_sha256': digest(canonical(requests) + b'\n'),
                    'build': record, 'timeout_seconds': timeout,
                    'native_report_sha256': digest((native / 'report.json').read_bytes())}
        shutil.copy2(record['binary'], output / 'executable')
        shutil.copy2(native / 'report.json', output / 'native-report.json')
        shutil.copy2(native / 'observations.ndjson', output / 'native-observations.ndjson')
        write_new(output / 'requests.json', requests)
        write_new(output / 'capture.json', metadata)
        start = 0
    binary = output / 'executable'
    for index in range(start, len(requests)):
        request = requests[index]
        case_dir = output / 'cases' / f'{index:05d}'
        if expected[index]['state'] != 'executed':
            p4.begin_case(case_dir, request)
            row = p4.fatal(request, 'native_unavailable', expected[index]['state'])
        else:
            row = p4.execute_case(binary, case_dir, request, timeout, validator=validate_row)
        p4.complete_case(case_dir, request, metadata, row)
        if (index + 1) % 100 == 0 or index + 1 == len(requests):
            print(f'P5 inventory {index + 1}/{len(requests)}', file=sys.stderr, flush=True)
    _, _, report = replay(output)
    report['source_stable'] = source_fn() == metadata['build']['sources']
    p4.atomic(output / 'report.json', report)
    print(report['counts_by_tier'])
    if not report['source_stable']:
        print('Capture uses its historical source snapshot; current workspace differs.', file=sys.stderr)


if __name__ == '__main__':
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--native', type=Path)
    parser.add_argument('--loading-requests', type=Path, default=ROOT / 'target/s07-subset/review/loading-requests.candidate.json')
    parser.add_argument('--output', type=Path, required=True)
    parser.add_argument('--timeout', type=float, default=60)
    parser.add_argument('--resume', action='store_true')
    parser.add_argument('--replay', action='store_true')
    args = parser.parse_args()
    import math
    if not math.isfinite(args.timeout) or args.timeout <= 0:
        parser.error('--timeout must be positive and finite')
    if args.replay:
        _, _, report = replay(args.output.resolve())
        p4.atomic(args.output / 'replayed.json', report)
        print(report['counts_by_tier'])
    elif args.native:
        run(args.native.resolve(), args.loading_requests.resolve(), args.output.resolve(), args.timeout, args.resume)
    else:
        parser.error('--native is required unless replaying')
