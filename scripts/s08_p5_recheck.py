#!/usr/bin/env python3
"""Capture or replay a named P5 subset without changing full-corpus counts.

A control supplies authenticated requests and native observations; it never
supplies expected results to the Rust executable. Historical temporary drivers
are preserved byte-for-byte under tools/s08/results/p5-corpus/producers/.
"""
import argparse
import math
from pathlib import Path
import shutil
import sys

import s08_p5_corpus as p5


def authenticated(directory, name, expected):
    raw = (directory / name).read_bytes()
    if p5.digest(raw) != expected:
        raise ValueError('capture fingerprint differs: ' + name)
    return raw


def expected_rows(directory, metadata):
    if 'expected_sha256' in metadata:
        return p5.strict_json_loads(authenticated(directory, 'expected.json', metadata['expected_sha256']))
    report = p5.strict_json_loads(authenticated(directory, 'native-report.json', metadata['native_report_sha256']))
    raw = authenticated(directory, 'native-observations.ndjson', report['observation_sha256'])
    return [p5.strict_json_loads(line) for line in raw.splitlines()]


def select(control, selection):
    metadata = p5.read(control / 'capture.json')
    raw = authenticated(control, 'requests.json', metadata['requests_sha256'])
    requests = p5.strict_json_loads(raw)
    expected = expected_rows(control, metadata)
    if [r['id'] for r in requests] != [r['id'] for r in expected]:
        raise ValueError('control native inventory differs')
    ids = selection.get('ids')
    if (selection.get('version') != 1 or not isinstance(selection.get('selection'), str)
            or not isinstance(ids, list) or not ids or any(not isinstance(i, str) for i in ids)
            or len(ids) != len(set(ids))):
        raise ValueError('selection needs unique, nonempty ordered IDs and a reason')
    by_id = {r['id']: i for i, r in enumerate(requests)}
    if len(by_id) != len(requests) or any(i not in by_id for i in ids):
        raise ValueError('unknown or ambiguous selection ID')
    indexes = [by_id[i] for i in ids]
    if indexes != sorted(indexes):
        raise ValueError('selection must preserve control order')
    selected_requests = [requests[i] for i in indexes]
    selected_expected = [expected[i] for i in indexes]
    for request, row in zip(selected_requests, selected_expected, strict=True):
        p5.native_metadata(request, row)
    return selected_requests, selected_expected, indexes


def replay(output, allow_partial=False):
    metadata = p5.read(output / 'capture.json')
    authenticated(output, 'producer.py', metadata['producer_sha256'])
    authenticated(output, 'executable', metadata['build']['binary_sha256'])
    expected = expected_rows(output, metadata)
    requests = p5.strict_json_loads(authenticated(output, 'requests.json', metadata['requests_sha256']))
    if [r['id'] for r in requests] != [r['id'] for r in expected]:
        raise ValueError('recheck native inventory differs')
    if 'selection_sha256' in metadata:
        selection = p5.strict_json_loads(authenticated(output, 'selection.json', metadata['selection_sha256']))
        if selection['ids'] != [r['id'] for r in requests]:
            raise ValueError('recheck selection differs')
    for request, row in zip(requests, expected, strict=True):
        p5.native_metadata(request, row)
    return p5.p4.replay(output, allow_partial, validator=p5.validate_row,
                        summarizer=lambda reqs, rows: p5.summarize(reqs, rows, expected))


def run(control, selection, output, timeout, resume=False):
    if not math.isfinite(timeout) or timeout <= 0:
        raise ValueError('timeout must be positive and finite')
    requests, expected, indexes = select(control, selection)
    control_hash = p5.digest((control / 'capture.json').read_bytes())
    selection_raw = p5.canonical(selection) + b'\n'
    if output.exists():
        if not resume:
            raise ValueError('existing recheck requires --resume')
        metadata = p5.read(output / 'capture.json')
        if (metadata['control_capture_sha256'] != control_hash or metadata['timeout_seconds'] != timeout
                or metadata.get('selection_sha256') != p5.digest(selection_raw)):
            raise ValueError('resume requires identical control, selection and timeout')
        stored, completed, _ = replay(output, True)
        if stored != requests or expected_rows(output, metadata) != expected:
            raise ValueError('resume inputs changed')
        start = len(completed)
    else:
        output.mkdir(parents=True)
        record = p5.p4.build(output / 'build', example='p5_inventory', source_fn=p5.sources, optimize=True)
        (output / 'source-snapshot').symlink_to('build/source-snapshot', target_is_directory=True)
        shutil.copy2(record['binary'], output / 'executable')
        metadata = {'version': 1, 'selection': selection['selection'],
                    'selection_sha256': p5.digest(selection_raw),
                    'control_capture_sha256': control_hash, 'control_indexes': indexes,
                    'requests_sha256': p5.digest(p5.canonical(requests) + b'\n'),
                    'expected_sha256': p5.digest(p5.canonical(expected) + b'\n'),
                    'producer_sha256': p5.digest(Path(__file__).read_bytes()),
                    'build': record, 'timeout_seconds': timeout}
        p5.write_new(output / 'capture.json', metadata)
        p5.write_new(output / 'selection.json', selection)
        p5.write_new(output / 'requests.json', requests)
        p5.write_new(output / 'expected.json', expected)
        shutil.copy2(__file__, output / 'producer.py')
        (output / 'cases').mkdir()
        start = 0
    for index in range(start, len(requests)):
        directory = output / 'cases' / f'{index:05d}'
        request = requests[index]
        if expected[index]['state'] != 'executed':
            p5.p4.begin_case(directory, request)
            row = p5.p4.fatal(request, 'native_unavailable', expected[index]['state'])
        else:
            row = p5.p4.execute_case(output / 'executable', directory, request, timeout, validator=p5.validate_row)
        p5.p4.complete_case(directory, request, metadata, row)
        if (index + 1) % 10 == 0 or index + 1 == len(requests):
            print(f'P5 recheck {index + 1}/{len(requests)}', file=sys.stderr, flush=True)
    _, _, report = replay(output)
    report['source_stable'] = p5.sources() == metadata['build']['sources']
    p5.p4.atomic(output / 'report.json', report)
    print(report['counts_by_tier'], 'source_stable', report['source_stable'])


if __name__ == '__main__':
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--control', type=Path)
    parser.add_argument('--selection', type=Path)
    parser.add_argument('--output', type=Path, required=True)
    parser.add_argument('--timeout', type=float, default=60)
    parser.add_argument('--resume', action='store_true')
    parser.add_argument('--replay', action='store_true')
    args = parser.parse_args()
    if args.replay:
        _, _, report = replay(args.output.resolve())
        p5.p4.atomic(args.output / 'replayed.json', report)
        print(report['counts_by_tier'])
    elif args.control and args.selection:
        run(args.control.resolve(), p5.read(args.selection), args.output.resolve(), args.timeout, args.resume)
    else:
        parser.error('--control and --selection are required unless replaying')
